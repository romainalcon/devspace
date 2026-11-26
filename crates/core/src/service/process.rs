//! Command lines of the servers and initialisation of their data
//! directories.
//!
//! A data directory is created in a staging directory next to it (initdb,
//! or `mysqld --initialize-insecure`) and renamed into place once the tool
//! succeeded: an instance whose data directory exists is initialised. The
//! servers then run in the foreground, as the current user, with every
//! setting on their command line (MySQL ignores the option files of the
//! machine and of the user), listening on the loopback address of the
//! sites and, on Unix, on a socket of the private runtime directory.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use devspace_platform::fs::{FileMode, write_atomic};
use devspace_platform::paths::OsFamily;

use super::{InstanceKey, ServiceLayout, ServiceName};
use crate::runtime::InstalledRuntime;
use crate::{Error, Result};

/// Time limit of the initialisation of a data directory.
pub const INIT_TIMEOUT: Duration = Duration::from_secs(300);

/// Statements run by MySQL right after creating its data directory.
/// `--initialize-insecure` creates `root@localhost` (socket connections)
/// without a password; TCP clients connect from a loopback address.
pub const MYSQL_INIT_SQL: &str = "\
CREATE USER 'root'@'127.%' IDENTIFIED BY '';
GRANT ALL PRIVILEGES ON *.* TO 'root'@'127.%' WITH GRANT OPTION;
";

/// Client authentication of PostgreSQL, written by DevSpace at every
/// start (`hba_file`): the socket and every loopback address, without a
/// password. The file `initdb` writes only accepts `127.0.0.1`.
pub const PG_HBA_CONF: &str = "\
# Managed by DevSpace: rewritten at every start.
local all all trust
host  all all 127.0.0.0/8 trust
";

/// An instance about to run.
#[derive(Debug, Clone, Copy)]
pub struct Instance<'a> {
    /// Service and series.
    pub key: InstanceKey,
    /// Installation it runs.
    pub runtime: &'a InstalledRuntime,
    /// Loopback address of the sites.
    pub address: Ipv4Addr,
    /// TCP port.
    pub port: u16,
    /// Files of the instances.
    pub layout: &'a ServiceLayout,
}

/// A program to run with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Absolute path of the program.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<OsString>,
}

/// `--name=value` with a path value.
fn option(name: &str, value: &Path) -> OsString {
    let mut text = OsString::from(format!("--{name}="));
    text.push(value);
    text
}

impl Instance<'_> {
    fn failure(&self, detail: String) -> Error {
        Error::ServiceInit {
            instance: self.key.to_string(),
            detail,
        }
    }

    fn program(&self, base: &str) -> Result<PathBuf> {
        self.runtime.program(base).ok_or_else(|| {
            self.failure(format!(
                "{} {} does not provide `{base}`",
                self.runtime.name, self.runtime.version
            ))
        })
    }

    /// Client authentication file of a PostgreSQL server.
    fn pg_hba_file(&self) -> PathBuf {
        self.layout.socket_dir(self.key).join("pg_hba.conf")
    }

    /// Socket of a MySQL server.
    fn mysql_socket(&self) -> PathBuf {
        self.layout.socket_dir(self.key).join("mysqld.sock")
    }

    /// Command line of the server, for the platform `os`.
    ///
    /// # Errors
    ///
    /// [`Error::ServiceInit`] if the installation lacks the server.
    pub fn server(&self, os: OsFamily) -> Result<Invocation> {
        let data = self.layout.data_dir(self.key);
        let sockets = self.layout.socket_dir(self.key);
        let mut args: Vec<OsString> = Vec::new();
        match self.key.service {
            ServiceName::Postgresql => {
                args.extend(["-D".into(), data.into_os_string()]);
                args.extend([
                    "-h".into(),
                    self.address.to_string().into(),
                    "-p".into(),
                    self.port.to_string().into(),
                    "-c".into(),
                ]);
                let mut hba = OsString::from("hba_file=");
                hba.push(self.pg_hba_file());
                args.push(hba);
                if os == OsFamily::Windows {
                    args.extend(["-c".into(), "unix_socket_directories=".into()]);
                } else {
                    args.extend([
                        "-k".into(),
                        sockets.into_os_string(),
                        "-c".into(),
                        "unix_socket_permissions=0700".into(),
                    ]);
                }
            }
            ServiceName::Valkey => {
                // No configuration file: the defaults of the program, then
                // these settings.
                args.extend([
                    "--bind".into(),
                    self.address.to_string().into(),
                    "--port".into(),
                    self.port.to_string().into(),
                    "--dir".into(),
                    data.into_os_string(),
                    "--daemonize".into(),
                    "no".into(),
                    // Listening on loopback only, without a password.
                    "--protected-mode".into(),
                    "no".into(),
                    // Output on the standard output, sent to the log file.
                    "--logfile".into(),
                    "".into(),
                    "--pidfile".into(),
                    "".into(),
                ]);
                if os != OsFamily::Windows {
                    args.extend([
                        "--unixsocket".into(),
                        sockets.join("valkey.sock").into_os_string(),
                        "--unixsocketperm".into(),
                        "700".into(),
                    ]);
                }
            }
            ServiceName::Mysql => {
                args.push("--no-defaults".into());
                args.push(option("basedir", &self.runtime.dir));
                args.push(option("datadir", &data));
                args.push(format!("--bind-address={}", self.address).into());
                args.push(format!("--port={}", self.port).into());
                args.push("--mysqlx=OFF".into());
                args.push("--skip-name-resolve".into());
                // No file import or export through SQL.
                args.push("--secure-file-priv=NULL".into());
                args.push(option("pid-file", &sockets.join("mysqld.pid")));
                if os == OsFamily::Windows {
                    // The error log goes to a file of the data directory
                    // otherwise.
                    args.push("--console".into());
                } else {
                    args.push(option("socket", &self.mysql_socket()));
                }
            }
        }
        Ok(Invocation {
            program: self.program(self.key.service.server_program())?,
            args,
        })
    }

    /// Command asking the server to stop gracefully, for platforms without
    /// stop signals (Windows); `None` where a signal is sent instead
    /// ([`ServiceName::stop_request`]).
    ///
    /// # Errors
    ///
    /// [`Error::ServiceInit`] if the installation lacks the tool.
    pub fn stop_command(&self, os: OsFamily) -> Result<Option<Invocation>> {
        if os != OsFamily::Windows {
            return Ok(None);
        }
        Ok(match self.key.service {
            // "fast": disconnects the clients, like `SIGINT` on Unix.
            ServiceName::Postgresql => Some(Invocation {
                program: self.program("pg_ctl")?,
                args: vec![
                    "stop".into(),
                    "-D".into(),
                    self.layout.data_dir(self.key).into_os_string(),
                    "-m".into(),
                    "fast".into(),
                    "-W".into(),
                ],
            }),
            ServiceName::Mysql => Some(Invocation {
                program: self.program("mysqladmin")?,
                args: vec![
                    "--no-defaults".into(),
                    format!("--host={}", self.address).into(),
                    format!("--port={}", self.port).into(),
                    format!("--user={}", ServiceName::Mysql.user()).into(),
                    "shutdown".into(),
                ],
            }),
            // Not published for Windows.
            ServiceName::Valkey => None,
        })
    }

    /// Commands creating the data directory in `staging` (an existing,
    /// empty directory).
    fn init_steps(&self, staging: &Path, init_sql: &Path, os: OsFamily) -> Result<Vec<Invocation>> {
        Ok(match self.key.service {
            ServiceName::Postgresql => vec![Invocation {
                program: self.program("initdb")?,
                args: vec![
                    option("pgdata", staging),
                    format!("--username={}", ServiceName::Postgresql.user()).into(),
                    "--auth=trust".into(),
                    "--encoding=UTF8".into(),
                    "--no-locale".into(),
                    "--no-instructions".into(),
                ],
            }],
            ServiceName::Mysql => {
                let mut args = vec![
                    "--no-defaults".into(),
                    "--initialize-insecure".into(),
                    option("basedir", &self.runtime.dir),
                    option("datadir", staging),
                    option("init-file", init_sql),
                ];
                if os == OsFamily::Windows {
                    args.push("--console".into());
                }
                vec![Invocation {
                    program: self.program("mysqld")?,
                    args,
                }]
            }
            // An empty data directory: Valkey creates its files.
            ServiceName::Valkey => vec![],
        })
    }

    /// Creates the data directory of the instance unless it exists, the
    /// private directory of its sockets and its generated configuration.
    /// Returns whether the data directory was created. The output of the tools goes to the log of the
    /// instance. Blocking; takes seconds.
    ///
    /// # Errors
    ///
    /// [`Error::ServiceInit`] when a tool fails or times out; nothing is
    /// left in place of the data directory then.
    pub fn initialize(&self) -> Result<bool> {
        let file_error = |path: &Path| {
            let path = path.to_path_buf();
            move |source: io::Error| Error::InstallFile { path, source }
        };
        let sockets = self.layout.socket_dir(self.key);
        fs::create_dir_all(&self.layout.run_dir).map_err(file_error(&self.layout.run_dir))?;
        match devspace_platform::fs::create_private_dir(&sockets) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(file_error(&sockets)(err)),
        }
        if self.key.service == ServiceName::Postgresql {
            let hba = self.pg_hba_file();
            write_atomic(&hba, PG_HBA_CONF.as_bytes(), FileMode::Private)
                .map_err(file_error(&hba))?;
        }
        let data = self.layout.data_dir(self.key);
        if data.is_dir() {
            return Ok(false);
        }
        let staging = self.layout.staging_dir(self.key);
        let parent = staging.parent().unwrap_or(&self.layout.data_root);
        fs::create_dir_all(parent).map_err(file_error(parent))?;
        remove_dir_if_exists(&staging)?;
        devspace_platform::fs::create_private_dir(&staging).map_err(file_error(&staging))?;
        let init_sql = parent.join(format!(".init-{}.sql", self.key.series));
        let result = write_atomic(&init_sql, MYSQL_INIT_SQL.as_bytes(), FileMode::Private)
            .map_err(file_error(&init_sql))
            .and_then(|()| self.init_steps(&staging, &init_sql, OsFamily::current()))
            .and_then(|steps| {
                let log_path = self.layout.log_file(self.key);
                for step in steps {
                    self.run_step(&step, &log_path)?;
                }
                fs::rename(&staging, &data).map_err(file_error(&data))
            });
        let _ = fs::remove_file(&init_sql);
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        tracing::info!(instance = %self.key, dir = %data.display(), "service data directory created");
        Ok(true)
    }

    /// Runs `step` with its output appended to `log_path`.
    fn run_step(&self, step: &Invocation, log_path: &Path) -> Result<()> {
        let name = step
            .program
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let log = open_log(log_path).map_err(|source| Error::InstallFile {
            path: log_path.to_path_buf(),
            source,
        })?;
        let log_err = log.try_clone().map_err(|source| Error::InstallFile {
            path: log_path.to_path_buf(),
            source,
        })?;
        tracing::info!(instance = %self.key, program = %step.program.display(), "initialising the service data");
        let mut child = Command::new(&step.program)
            .args(&step.args)
            .current_dir(&self.layout.data_root)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .spawn()
            .map_err(|err| self.failure(format!("cannot run `{name}`: {err}")))?;
        let deadline = Instant::now() + INIT_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(self.failure(format!(
                        "`{name}` did not finish within {}s; see `{}`",
                        INIT_TIMEOUT.as_secs(),
                        log_path.display()
                    )));
                }
                Err(err) => return Err(self.failure(format!("lost track of `{name}`: {err}"))),
            }
        };
        if status.success() {
            Ok(())
        } else {
            Err(self.failure(format!(
                "`{name}` failed ({status}); see `{}`",
                log_path.display()
            )))
        }
    }
}

/// Opens a log file for appending (several programs write to it in turn).
///
/// # Errors
///
/// The I/O error.
pub fn open_log(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn remove_dir_if_exists(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::InstallFile {
            path: dir.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppPaths;
    use crate::runtime::{RuntimeName, Version};
    use crate::service::Series;

    fn setup(name: RuntimeName, version: &str, root: &Path) -> (InstalledRuntime, ServiceLayout) {
        let version = Version::parse(version).unwrap();
        let dir = root
            .join("runtimes")
            .join(name.as_str())
            .join(version.to_string());
        let runtime = InstalledRuntime {
            name,
            version,
            bin_dirs: vec![dir.join("bin")],
            dir,
        };
        (runtime, ServiceLayout::new(&AppPaths::from_root(root)))
    }

    fn args(invocation: &Invocation) -> Vec<String> {
        invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[cfg(unix)]
    fn script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        devspace_platform::fs::set_mode(path, FileMode::Executable).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn server_command_lines_stay_on_the_loopback() {
        let tmp = tempfile::tempdir().unwrap();
        let (pg, layout) = setup(RuntimeName::Postgresql, "18.7.0", tmp.path());
        script(&pg.dir.join("bin/postgres"), "exit 0");
        let key = InstanceKey::of(ServiceName::Postgresql, pg.version);
        let instance = Instance {
            key,
            runtime: &pg,
            address: Ipv4Addr::new(127, 0, 0, 2),
            port: 5432,
            layout: &layout,
        };
        let unix = instance.server(OsFamily::MacOs).unwrap();
        assert_eq!(unix.program, pg.dir.join("bin/postgres"));
        let data = layout.data_dir(key).display().to_string();
        let sockets = layout.socket_dir(key).display().to_string();
        let hba = format!(
            "hba_file={}",
            layout.socket_dir(key).join("pg_hba.conf").display()
        );
        assert_eq!(
            args(&unix),
            [
                "-D",
                &data,
                "-h",
                "127.0.0.2",
                "-p",
                "5432",
                "-c",
                &hba,
                "-k",
                &sockets,
                "-c",
                "unix_socket_permissions=0700"
            ]
        );
        let windows = args(&instance.server(OsFamily::Windows).unwrap());
        assert!(windows.contains(&"unix_socket_directories=".to_owned()));
        assert!(!windows.contains(&"-k".to_owned()));

        let (my, layout) = setup(RuntimeName::Mysql, "8.4.13.1", tmp.path());
        script(&my.dir.join("bin/mysqld"), "exit 0");
        let key = InstanceKey::of(ServiceName::Mysql, my.version);
        assert_eq!(
            key.series,
            Series::parse(ServiceName::Mysql, "8.4").unwrap()
        );
        let instance = Instance {
            key,
            runtime: &my,
            address: Ipv4Addr::new(127, 0, 0, 2),
            port: 3306,
            layout: &layout,
        };
        let unix = args(&instance.server(OsFamily::Linux).unwrap());
        for expected in [
            "--no-defaults".to_owned(),
            format!("--basedir={}", my.dir.display()),
            format!("--datadir={}", layout.data_dir(key).display()),
            "--bind-address=127.0.0.2".to_owned(),
            "--port=3306".to_owned(),
            "--mysqlx=OFF".to_owned(),
            format!(
                "--socket={}",
                layout.socket_dir(key).join("mysqld.sock").display()
            ),
        ] {
            assert!(unix.contains(&expected), "{expected} in {unix:?}");
        }
        assert_eq!(unix[0], "--no-defaults", "must come first");
        let windows = args(&instance.server(OsFamily::Windows).unwrap());
        assert!(windows.contains(&"--console".to_owned()));
        assert!(!windows.iter().any(|arg| arg.starts_with("--socket")));
    }

    #[cfg(unix)]
    #[test]
    fn valkey_runs_without_configuration_file_and_creates_its_data() {
        let tmp = tempfile::tempdir().unwrap();
        let (vk, layout) = setup(RuntimeName::Valkey, "9.1.4", tmp.path());
        script(&vk.dir.join("bin/valkey-server"), "exit 0");
        let key = InstanceKey::of(ServiceName::Valkey, vk.version);
        assert_eq!(key.series.to_string(), "9.1");
        let instance = Instance {
            key,
            runtime: &vk,
            address: Ipv4Addr::new(127, 0, 0, 2),
            port: 6379,
            layout: &layout,
        };
        let unix = instance.server(OsFamily::MacOs).unwrap();
        assert_eq!(unix.program, vk.dir.join("bin/valkey-server"));
        let data = layout.data_dir(key).display().to_string();
        let socket = layout
            .socket_dir(key)
            .join("valkey.sock")
            .display()
            .to_string();
        assert_eq!(
            args(&unix),
            [
                "--bind",
                "127.0.0.2",
                "--port",
                "6379",
                "--dir",
                &data,
                "--daemonize",
                "no",
                "--protected-mode",
                "no",
                "--logfile",
                "",
                "--pidfile",
                "",
                "--unixsocket",
                &socket,
                "--unixsocketperm",
                "700"
            ]
        );
        // The first argument is an option: no configuration file is read.
        assert!(args(&unix)[0].starts_with("--"));

        assert!(instance.initialize().unwrap());
        assert!(layout.data_dir(key).is_dir());
        assert!(!instance.initialize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn windows_servers_are_stopped_by_their_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let (my, layout) = setup(RuntimeName::Mysql, "8.4.13.1", tmp.path());
        script(&my.dir.join("bin/mysqladmin"), "exit 0");
        let instance = Instance {
            key: InstanceKey::of(ServiceName::Mysql, my.version),
            runtime: &my,
            address: Ipv4Addr::new(127, 0, 0, 2),
            port: 3306,
            layout: &layout,
        };
        assert!(instance.stop_command(OsFamily::MacOs).unwrap().is_none());
        let stop = instance.stop_command(OsFamily::Windows).unwrap().unwrap();
        assert_eq!(stop.program, my.dir.join("bin/mysqladmin"));
        assert_eq!(
            args(&stop),
            [
                "--no-defaults",
                "--host=127.0.0.2",
                "--port=3306",
                "--user=root",
                "shutdown"
            ]
        );

        let (pg, layout) = setup(RuntimeName::Postgresql, "18.7.0", tmp.path());
        let instance = Instance {
            key: InstanceKey::of(ServiceName::Postgresql, pg.version),
            runtime: &pg,
            address: Ipv4Addr::new(127, 0, 0, 2),
            port: 5432,
            layout: &layout,
        };
        assert!(
            instance.stop_command(OsFamily::Windows).is_err(),
            "pg_ctl missing"
        );
        script(&pg.dir.join("bin/pg_ctl"), "exit 0");
        let stop = instance.stop_command(OsFamily::Windows).unwrap().unwrap();
        assert_eq!(args(&stop)[..2], ["stop", "-D"]);
        assert_eq!(args(&stop)[3..], ["-m", "fast", "-W"]);
    }

    #[test]
    fn missing_programs_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let (pg, layout) = setup(RuntimeName::Postgresql, "18.7.0", tmp.path());
        let instance = Instance {
            key: InstanceKey::of(ServiceName::Postgresql, pg.version),
            runtime: &pg,
            address: Ipv4Addr::LOCALHOST,
            port: 5432,
            layout: &layout,
        };
        let err = instance.server(OsFamily::current()).unwrap_err();
        assert!(err.to_string().contains("`postgres`"), "{err}");
        assert!(instance.initialize().is_err());
        assert!(!layout.data_dir(instance.key).exists());
    }

    #[cfg(unix)]
    #[test]
    fn initialisation_happens_once_and_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let (pg, layout) = setup(RuntimeName::Postgresql, "18.7.0", tmp.path());
        let calls = tmp.path().join("calls");
        // A fake initdb: records its arguments and fills --pgdata.
        script(
            &pg.dir.join("bin/initdb"),
            &format!(
                "echo \"$@\" >> '{}'\nfor a in \"$@\"; do case $a in --pgdata=*) \
                 echo 18 > \"${{a#--pgdata=}}/PG_VERSION\";; esac; done\necho initialised",
                calls.display()
            ),
        );
        let key = InstanceKey::of(ServiceName::Postgresql, pg.version);
        let instance = Instance {
            key,
            runtime: &pg,
            address: Ipv4Addr::LOCALHOST,
            port: 5432,
            layout: &layout,
        };
        assert!(instance.initialize().unwrap());
        let data = layout.data_dir(key);
        assert_eq!(fs::read_to_string(data.join("PG_VERSION")).unwrap(), "18\n");
        assert!(!layout.staging_dir(key).exists());
        assert!(layout.socket_dir(key).is_dir());
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(layout.socket_dir(key))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        assert_eq!(
            fs::read_to_string(layout.socket_dir(key).join("pg_hba.conf")).unwrap(),
            PG_HBA_CONF
        );
        let recorded = fs::read_to_string(&calls).unwrap();
        assert!(recorded.contains("--auth=trust"), "{recorded}");
        assert!(recorded.contains("--username=postgres"), "{recorded}");
        assert!(
            fs::read_to_string(layout.log_file(key))
                .unwrap()
                .contains("initialised")
        );
        let leftovers: Vec<_> = fs::read_dir(data.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["18"], "no staging directory or SQL file left");

        // Already initialised: the tool is not run again.
        assert!(!instance.initialize().unwrap());
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn failed_initialisation_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (my, layout) = setup(RuntimeName::Mysql, "8.4.13.1", tmp.path());
        let seen_sql = tmp.path().join("seen.sql");
        script(
            &my.dir.join("bin/mysqld"),
            &format!(
                "for a in \"$@\"; do case $a in --init-file=*) cp \"${{a#--init-file=}}\" \
                 '{}';; --datadir=*) touch \"${{a#--datadir=}}/ibdata1\";; esac; done\n\
                 echo 'cannot initialise' >&2\nexit 1",
                seen_sql.display()
            ),
        );
        let key = InstanceKey::of(ServiceName::Mysql, my.version);
        let instance = Instance {
            key,
            runtime: &my,
            address: Ipv4Addr::LOCALHOST,
            port: 3306,
            layout: &layout,
        };
        let err = instance.initialize().unwrap_err();
        assert!(matches!(err, Error::ServiceInit { .. }), "{err}");
        assert!(err.to_string().contains("mysql 8.4"), "{err}");
        assert!(!layout.data_dir(key).exists());
        assert!(!layout.staging_dir(key).exists());
        assert_eq!(fs::read_to_string(&seen_sql).unwrap(), MYSQL_INIT_SQL);
        assert!(
            fs::read_to_string(layout.log_file(key))
                .unwrap()
                .contains("cannot initialise")
        );
    }
}
