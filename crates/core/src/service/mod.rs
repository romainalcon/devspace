//! Services: databases run by the daemon (MySQL, PostgreSQL).
//!
//! The programs of a service are installed like the runtimes
//! ([`crate::runtime`], packages `mysql` and `postgresql`). An *instance*
//! is identified by a service and a [`Series`]: the major version of
//! PostgreSQL, or the `MAJOR.MINOR` series of MySQL, the versions that share
//! a data format. Each instance runs the newest installed version of its
//! series and keeps its data in `<data>/services/<name>/<series>/`, shared
//! by every project using it.
//!
//! What runs is decided by [`plan`]: an instance runs when a registered
//! project requires it (`[services]` of its manifest) or when the user
//! started it, unless the user stopped it. Those choices are stored in the
//! database ([`Database::set_service_mode`]). All the series of a service
//! listen on the same port, so only one of them runs at a time.
//!
//! - [`process`]: initialisation of a data directory and the command
//!   lines of the servers.
//!
//! Servers listen on the loopback address of the sites only, and accept
//! their administrator (`root`, `postgres`) without a password: they are
//! development databases, never reachable from the network.

pub mod process;

use std::collections::BTreeMap;
use std::fmt;
use std::net::Ipv4Addr;
use std::path::PathBuf;

use devspace_platform::process::StopRequest;

use crate::AppPaths;
use crate::config::ServicesConfig;
use crate::db::Database;
use crate::project::ProjectName;
use crate::runtime::{InstalledRuntime, RuntimeName, Version, VersionReq};
use crate::{Error, Result};

/// A service DevSpace can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceName {
    /// MySQL Community Server.
    Mysql,
    /// PostgreSQL.
    Postgresql,
}

impl ServiceName {
    /// Every service, in display order.
    pub const ALL: [Self; 2] = [Self::Mysql, Self::Postgresql];

    /// Lowercase name, as used in manifests, paths and commands; also the
    /// name of its package.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.package().as_str()
    }

    /// Package holding the programs of the service.
    #[must_use]
    pub const fn package(self) -> RuntimeName {
        match self {
            Self::Mysql => RuntimeName::Mysql,
            Self::Postgresql => RuntimeName::Postgresql,
        }
    }

    /// The service whose programs `package` holds, if any.
    #[must_use]
    pub const fn of_package(package: RuntimeName) -> Option<Self> {
        match package {
            RuntimeName::Mysql => Some(Self::Mysql),
            RuntimeName::Postgresql => Some(Self::Postgresql),
            RuntimeName::Node | RuntimeName::Php => None,
        }
    }

    /// Parses a service name.
    ///
    /// # Errors
    ///
    /// Returns a description listing the known services.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|name| name.as_str() == text)
            .ok_or_else(|| format!("unknown service `{text}`; expected `mysql` or `postgresql`"))
    }

    /// TCP port of the service.
    #[must_use]
    pub const fn port(self, config: &ServicesConfig) -> u16 {
        match self {
            Self::Mysql => config.mysql_port,
            Self::Postgresql => config.postgresql_port,
        }
    }

    /// Administrator account, accepted without a password.
    #[must_use]
    pub const fn user(self) -> &'static str {
        match self {
            Self::Mysql => "root",
            Self::Postgresql => "postgres",
        }
    }

    /// How the server is asked to stop: PostgreSQL waits for its clients
    /// to disconnect on `SIGTERM`, `SIGINT` disconnects them.
    #[must_use]
    pub const fn stop_request(self) -> StopRequest {
        match self {
            Self::Mysql => StopRequest::Terminate,
            Self::Postgresql => StopRequest::Interrupt,
        }
    }

    /// Program of the server, without the `.exe` suffix.
    #[must_use]
    pub const fn server_program(self) -> &'static str {
        match self {
            Self::Mysql => "mysqld",
            Self::Postgresql => "postgres",
        }
    }

    /// Environment variables pointing the command-line clients of the
    /// service (`mysql`, `psql`, …) at its instance.
    #[must_use]
    pub fn client_variables(self, address: Ipv4Addr, port: u16) -> Vec<(String, String)> {
        // The MySQL clients have no variable for the user name: they need
        // `-u root`.
        let vars = match self {
            Self::Mysql => vec![
                ("MYSQL_HOST", address.to_string()),
                ("MYSQL_TCP_PORT", port.to_string()),
            ],
            Self::Postgresql => vec![
                ("PGHOST", address.to_string()),
                ("PGPORT", port.to_string()),
                ("PGUSER", self.user().to_owned()),
            ],
        };
        vars.into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect()
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Versions of a service sharing a data format: `MAJOR` for PostgreSQL,
/// `MAJOR.MINOR` for MySQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Series {
    /// Major version.
    pub major: u32,
    /// Minor version (MySQL only).
    pub minor: Option<u32>,
}

impl Series {
    /// Series of `version` of `service`.
    #[must_use]
    pub const fn of(service: ServiceName, version: Version) -> Self {
        Self {
            major: version.major,
            minor: match service {
                ServiceName::Mysql => Some(version.minor),
                ServiceName::Postgresql => None,
            },
        }
    }

    /// Parses a series of `service`.
    ///
    /// # Errors
    ///
    /// Returns a description of the expected form.
    pub fn parse(service: ServiceName, text: &str) -> std::result::Result<Self, String> {
        let req = VersionReq::parse(text).ok();
        let series = match (service, req) {
            (
                ServiceName::Postgresql,
                Some(VersionReq {
                    major,
                    minor: None,
                    patch: None,
                }),
            ) => Some(Self { major, minor: None }),
            (
                ServiceName::Mysql,
                Some(VersionReq {
                    major,
                    minor: Some(minor),
                    patch: None,
                }),
            ) => Some(Self {
                major,
                minor: Some(minor),
            }),
            _ => None,
        };
        series.ok_or_else(|| match service {
            ServiceName::Postgresql => {
                format!(
                    "invalid PostgreSQL series `{text}`; expected a major version, such as `18`"
                )
            }
            ServiceName::Mysql => {
                format!("invalid MySQL series `{text}`; expected MAJOR.MINOR, such as `8.4`")
            }
        })
    }

    /// The series every version accepted by `req` belongs to, if the
    /// requirement determines one (`18`, `18.7` for PostgreSQL; `8.4`,
    /// `8.4.13` for MySQL, but not `8`).
    #[must_use]
    pub const fn of_requirement(service: ServiceName, req: VersionReq) -> Option<Self> {
        match (service, req.minor) {
            (ServiceName::Postgresql, _) => Some(Self {
                major: req.major,
                minor: None,
            }),
            (ServiceName::Mysql, Some(minor)) => Some(Self {
                major: req.major,
                minor: Some(minor),
            }),
            (ServiceName::Mysql, None) => None,
        }
    }

    /// Whether `version` belongs to the series.
    #[must_use]
    pub fn contains(&self, version: Version) -> bool {
        self.major == version.major && self.minor.is_none_or(|minor| minor == version.minor)
    }
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.major)?;
        if let Some(minor) = self.minor {
            write!(f, ".{minor}")?;
        }
        Ok(())
    }
}

/// An instance: a service and a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceKey {
    /// Service.
    pub service: ServiceName,
    /// Series.
    pub series: Series,
}

impl InstanceKey {
    /// Instance of `version` of `service`.
    #[must_use]
    pub const fn of(service: ServiceName, version: Version) -> Self {
        Self {
            service,
            series: Series::of(service, version),
        }
    }

    /// `<service>-<series>`, used in file names.
    #[must_use]
    pub fn file_stem(&self) -> String {
        format!("{}-{}", self.service, self.series)
    }

    /// Newest installed version of the instance.
    #[must_use]
    pub fn newest_installed(&self, installed: &[InstalledRuntime]) -> Option<InstalledRuntime> {
        installed
            .iter()
            .filter(|runtime| {
                runtime.name == self.service.package() && self.series.contains(runtime.version)
            })
            .max_by_key(|runtime| runtime.version)
            .cloned()
    }
}

impl fmt::Display for InstanceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.service, self.series)
    }
}

/// Choice of the user for an instance, overriding the projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceMode {
    /// Runs even if no project requires it.
    Started,
    /// Does not run even if projects require it.
    Stopped,
}

impl ServiceMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "started" => Some(Self::Started),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// Files of the instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLayout {
    /// `<data>/services`: one directory per service, then per series.
    pub data_root: PathBuf,
    /// `<run>/services`: sockets and pid files.
    pub run_dir: PathBuf,
    /// Log directory.
    pub log_dir: PathBuf,
}

impl ServiceLayout {
    /// Layout inside the DevSpace directories.
    #[must_use]
    pub fn new(paths: &AppPaths) -> Self {
        Self {
            data_root: paths.data_dir.join("services"),
            run_dir: paths.runtime_dir.join("services"),
            log_dir: paths.log_dir.clone(),
        }
    }

    /// Data directory of an instance; it exists only once initialised.
    #[must_use]
    pub fn data_dir(&self, key: InstanceKey) -> PathBuf {
        self.data_root
            .join(key.service.as_str())
            .join(key.series.to_string())
    }

    /// Directory where an instance is initialised before being renamed to
    /// [`Self::data_dir`], on the same filesystem.
    #[must_use]
    pub fn staging_dir(&self, key: InstanceKey) -> PathBuf {
        self.data_root
            .join(key.service.as_str())
            .join(format!(".init-{}", key.series))
    }

    /// Private directory of the Unix sockets of an instance.
    #[must_use]
    pub fn socket_dir(&self, key: InstanceKey) -> PathBuf {
        self.run_dir.join(key.file_stem())
    }

    /// Pid file of a running instance.
    #[must_use]
    pub fn pid_file(&self, key: InstanceKey) -> PathBuf {
        self.run_dir.join(format!("{}.pid", key.file_stem()))
    }

    /// Output of the server and of its initialisation.
    #[must_use]
    pub fn log_file(&self, key: InstanceKey) -> PathBuf {
        self.log_dir.join(format!("{}.log", key.file_stem()))
    }
}

/// A service required by a registered project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// Project.
    pub project: ProjectName,
    /// Service.
    pub service: ServiceName,
    /// Accepted versions.
    pub versions: VersionReq,
}

/// What [`plan`] decided for one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedInstance {
    /// Instance.
    pub key: InstanceKey,
    /// Newest installed version of its series, which it runs.
    pub runtime: Option<InstalledRuntime>,
    /// Choice of the user, if any.
    pub mode: Option<ServiceMode>,
    /// Projects requiring it, sorted.
    pub required_by: Vec<ProjectName>,
    /// Whether it must run.
    pub run: bool,
    /// Why it does not run although it should.
    pub blocked: Option<String>,
}

/// The instances to know about, and the requirements no installed version
/// satisfies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Every instance installed, required or chosen, sorted.
    pub instances: Vec<PlannedInstance>,
    /// Explanation of every unsatisfied requirement.
    pub missing: Vec<String>,
}

impl Plan {
    /// The instances that must run.
    pub fn running(&self) -> impl Iterator<Item = &PlannedInstance> {
        self.instances.iter().filter(|instance| instance.run)
    }
}

/// Decides which instances run: those required by a project or started by
/// the user, unless stopped by the user, and installed. At most one series
/// of a service runs (they share its port): the one the user started,
/// otherwise the newest.
#[must_use]
pub fn plan(
    requirements: &[Requirement],
    installed: &[InstalledRuntime],
    modes: &[(InstanceKey, ServiceMode)],
) -> Plan {
    let entry = |key: InstanceKey| PlannedInstance {
        key,
        runtime: key.newest_installed(installed),
        mode: None,
        required_by: Vec::new(),
        run: false,
        blocked: None,
    };
    let mut missing = Vec::new();
    let mut planned: BTreeMap<InstanceKey, PlannedInstance> = BTreeMap::new();

    for runtime in installed {
        if let Some(service) = ServiceName::of_package(runtime.name) {
            let key = InstanceKey::of(service, runtime.version);
            planned.entry(key).or_insert_with(|| entry(key));
        }
    }
    for requirement in requirements {
        let service = requirement.service;
        let best = installed
            .iter()
            .filter(|runtime| {
                runtime.name == service.package() && requirement.versions.matches(runtime.version)
            })
            .max_by_key(|runtime| runtime.version);
        match best {
            Some(runtime) => {
                let key = InstanceKey::of(service, runtime.version);
                planned
                    .entry(key)
                    .or_insert_with(|| entry(key))
                    .required_by
                    .push(requirement.project.clone());
            }
            // Not installed: the instance is known when the requirement
            // designates a series by itself (so that the choices of the
            // user apply to it), otherwise only reported.
            None => match Series::of_requirement(service, requirement.versions) {
                Some(series) => {
                    let key = InstanceKey { service, series };
                    planned
                        .entry(key)
                        .or_insert_with(|| entry(key))
                        .required_by
                        .push(requirement.project.clone());
                }
                None => missing.push(format!(
                    "project `{}` requires {service} {}, which is not installed; run \
                     `devspace service install {service} {}`",
                    requirement.project.as_str(),
                    requirement.versions,
                    requirement.versions
                )),
            },
        }
    }
    for (key, mode) in modes {
        planned.entry(*key).or_insert_with(|| entry(*key)).mode = Some(*mode);
    }

    for instance in planned.values_mut() {
        instance.required_by.sort();
        instance.required_by.dedup();
        let wanted = match instance.mode {
            Some(ServiceMode::Started) => true,
            Some(ServiceMode::Stopped) => false,
            None => !instance.required_by.is_empty(),
        };
        if wanted && instance.runtime.is_none() {
            instance.blocked = Some(format!(
                "no {} {} version is installed; run `devspace service install {} {}`",
                instance.key.service,
                instance.key.series,
                instance.key.service,
                instance.key.series
            ));
        }
        instance.run = wanted && instance.runtime.is_some();
    }

    // One running series per service: explicitly started first, then the
    // newest.
    for service in ServiceName::ALL {
        let winner = planned
            .values()
            .filter(|instance| instance.run && instance.key.service == service)
            .max_by_key(|instance| (instance.mode == Some(ServiceMode::Started), instance.key))
            .map(|instance| instance.key);
        if let Some(winner) = winner {
            for instance in planned.values_mut() {
                if instance.run && instance.key.service == service && instance.key != winner {
                    instance.run = false;
                    instance.blocked = Some(format!(
                        "{winner} uses the port of {service}; stop it to run this series"
                    ));
                }
            }
        }
    }

    missing.sort();
    missing.dedup();
    Plan {
        instances: planned.into_values().collect(),
        missing,
    }
}

impl Database {
    /// Choices of the user, sorted. Rows written by a newer DevSpace that
    /// this build does not understand are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on storage failure.
    pub fn service_modes(&self) -> Result<Vec<(InstanceKey, ServiceMode)>> {
        let db_err = |source| Error::Database {
            operation: "listing the service choices",
            source,
        };
        let mut statement = self
            .conn
            .prepare("SELECT name, series, mode FROM services ORDER BY name, series")
            .map_err(db_err)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db_err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err)?;
        let mut modes = Vec::new();
        for (name, series, mode) in rows {
            let parsed = ServiceName::parse(&name).ok().and_then(|service| {
                Some((
                    InstanceKey {
                        service,
                        series: Series::parse(service, &series).ok()?,
                    },
                    ServiceMode::parse(&mode)?,
                ))
            });
            match parsed {
                Some(choice) => modes.push(choice),
                None => {
                    tracing::warn!(%name, %series, %mode, "ignoring an unknown service choice");
                }
            }
        }
        modes.sort_by_key(|(key, _)| *key);
        Ok(modes)
    }

    /// Records the choice of the user for `key`, or forgets it (`None`:
    /// the projects decide).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] on storage failure.
    pub fn set_service_mode(&mut self, key: InstanceKey, mode: Option<ServiceMode>) -> Result<()> {
        let db_err = |source| Error::Database {
            operation: "recording a service choice",
            source,
        };
        let name = key.service.as_str();
        let series = key.series.to_string();
        match mode {
            Some(mode) => self.conn.execute(
                "INSERT INTO services (name, series, mode) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name, series) DO UPDATE SET mode = excluded.mode",
                [name, series.as_str(), mode.as_str()],
            ),
            None => self.conn.execute(
                "DELETE FROM services WHERE name = ?1 AND series = ?2",
                [name, series.as_str()],
            ),
        }
        .map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn runtime(name: RuntimeName, version: &str) -> InstalledRuntime {
        InstalledRuntime {
            name,
            version: v(version),
            dir: PathBuf::from(format!("/r/{name}/{version}")),
            bin_dirs: vec![PathBuf::from(format!("/r/{name}/{version}/bin"))],
        }
    }

    fn key(service: ServiceName, series: &str) -> InstanceKey {
        InstanceKey {
            service,
            series: Series::parse(service, series).unwrap(),
        }
    }

    fn requirement(project: &str, service: ServiceName, versions: &str) -> Requirement {
        Requirement {
            project: ProjectName::parse(project).unwrap(),
            service,
            versions: VersionReq::parse(versions).unwrap(),
        }
    }

    #[test]
    fn series_follow_the_data_formats() {
        use ServiceName::{Mysql, Postgresql};
        assert_eq!(Series::of(Postgresql, v("18.7.0")).to_string(), "18");
        assert_eq!(Series::of(Mysql, v("8.4.13.1")).to_string(), "8.4");
        assert!(
            Series::parse(Postgresql, "18")
                .unwrap()
                .contains(v("18.1.0"))
        );
        assert!(!Series::parse(Mysql, "8.4").unwrap().contains(v("9.7.4.1")));
        for (service, bad) in [
            (Postgresql, "18.7"),
            (Postgresql, "x"),
            (Mysql, "8"),
            (Mysql, "8.4.1"),
            (Mysql, ""),
        ] {
            assert!(Series::parse(service, bad).is_err(), "{service} {bad}");
        }
        assert_eq!(key(Mysql, "9.7").file_stem(), "mysql-9.7");
        for service in ServiceName::ALL {
            assert_eq!(ServiceName::parse(service.as_str()).unwrap(), service);
            assert_eq!(ServiceName::of_package(service.package()), Some(service));
            assert!(service.package().is_service());
        }
        assert!(ServiceName::parse("redis").is_err());
        assert_eq!(ServiceName::of_package(RuntimeName::Php), None);
    }

    #[test]
    fn layout_keeps_instances_apart() {
        let layout = ServiceLayout::new(&AppPaths::from_root(Path::new("/home")));
        let pg = key(ServiceName::Postgresql, "18");
        assert_eq!(
            layout.data_dir(pg),
            Path::new("/home/data/services/postgresql/18")
        );
        assert_eq!(
            layout.staging_dir(pg),
            Path::new("/home/data/services/postgresql/.init-18")
        );
        assert_eq!(
            layout.socket_dir(pg),
            Path::new("/home/run/services/postgresql-18")
        );
        assert_eq!(
            layout.pid_file(pg),
            Path::new("/home/run/services/postgresql-18.pid")
        );
        assert_eq!(
            layout.log_file(pg),
            Path::new("/home/logs/postgresql-18.log")
        );
    }

    #[test]
    fn required_instances_run_with_their_newest_version() {
        let installed = [
            runtime(RuntimeName::Postgresql, "17.12.0"),
            runtime(RuntimeName::Postgresql, "18.6.0"),
            runtime(RuntimeName::Postgresql, "18.7.0"),
            runtime(RuntimeName::Mysql, "8.4.13.1"),
            runtime(RuntimeName::Php, "8.4.23"),
        ];
        let plan = plan(
            &[
                requirement("blog", ServiceName::Postgresql, "18"),
                requirement("shop", ServiceName::Postgresql, "18.6"),
                requirement("shop", ServiceName::Mysql, "9"),
                requirement("wiki", ServiceName::Postgresql, "19"),
            ],
            &installed,
            &[],
        );
        let summary: Vec<String> = plan
            .instances
            .iter()
            .map(|instance| {
                format!(
                    "{} {} run={} by={:?}",
                    instance.key,
                    instance
                        .runtime
                        .as_ref()
                        .map_or_else(String::new, |r| r.version.to_string()),
                    instance.run,
                    instance
                        .required_by
                        .iter()
                        .map(ProjectName::as_str)
                        .collect::<Vec<_>>()
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                "mysql 8.4 8.4.13.1 run=false by=[]",
                "postgresql 17 17.12.0 run=false by=[]",
                // `18.6` selects the 18 series, which runs its newest version.
                "postgresql 18 18.7.0 run=true by=[\"blog\", \"shop\"]",
                // Not installed: known, required, not running.
                "postgresql 19  run=false by=[\"wiki\"]",
            ]
        );
        let pg19 = plan.instances.last().unwrap();
        assert!(
            pg19.blocked
                .as_deref()
                .unwrap()
                .contains("devspace service install postgresql 19"),
            "{pg19:?}"
        );
        assert_eq!(plan.missing.len(), 1);
        assert!(
            plan.missing[0].contains("`shop` requires mysql 9"),
            "{:?}",
            plan.missing
        );
        assert!(plan.missing[0].contains("devspace service install mysql 9"));
    }

    #[test]
    fn user_choices_override_the_projects() {
        let installed = [
            runtime(RuntimeName::Postgresql, "17.12.0"),
            runtime(RuntimeName::Postgresql, "18.7.0"),
            runtime(RuntimeName::Mysql, "8.4.13.1"),
        ];
        let requirements = [
            requirement("blog", ServiceName::Postgresql, "17"),
            requirement("shop", ServiceName::Mysql, "8.4"),
        ];
        let modes = [
            (key(ServiceName::Mysql, "8.4"), ServiceMode::Stopped),
            (key(ServiceName::Postgresql, "18"), ServiceMode::Started),
            (key(ServiceName::Mysql, "9.7"), ServiceMode::Started),
        ];
        let plan = plan(&requirements, &installed, &modes);
        let get = |k: InstanceKey| {
            plan.instances
                .iter()
                .find(|instance| instance.key == k)
                .unwrap()
        };
        assert!(
            !get(key(ServiceName::Mysql, "8.4")).run,
            "stopped by the user"
        );
        let mysql97 = get(key(ServiceName::Mysql, "9.7"));
        assert!(!mysql97.run);
        assert!(
            mysql97
                .blocked
                .as_deref()
                .unwrap()
                .contains("no mysql 9.7 version is installed"),
            "{mysql97:?}"
        );
        // Started explicitly: wins the port over the required 17.
        assert!(get(key(ServiceName::Postgresql, "18")).run);
        let pg17 = get(key(ServiceName::Postgresql, "17"));
        assert!(!pg17.run);
        assert!(
            pg17.blocked.as_deref().unwrap().contains("postgresql 18"),
            "{pg17:?}"
        );
        assert_eq!(
            plan.running()
                .map(|i| i.key.to_string())
                .collect::<Vec<_>>(),
            ["postgresql 18"]
        );
    }

    #[test]
    fn newest_series_wins_between_projects() {
        let installed = [
            runtime(RuntimeName::Mysql, "8.4.13.1"),
            runtime(RuntimeName::Mysql, "9.7.4.1"),
        ];
        let plan = plan(
            &[
                requirement("a", ServiceName::Mysql, "8.4"),
                requirement("b", ServiceName::Mysql, "9.7"),
            ],
            &installed,
            &[],
        );
        assert_eq!(
            plan.running()
                .map(|i| i.key.to_string())
                .collect::<Vec<_>>(),
            ["mysql 9.7"]
        );
    }

    #[test]
    fn choices_are_stored() {
        let mut db = Database::open_in_memory().unwrap();
        assert!(db.service_modes().unwrap().is_empty());
        let pg = key(ServiceName::Postgresql, "18");
        let my = key(ServiceName::Mysql, "8.4");
        db.set_service_mode(pg, Some(ServiceMode::Started)).unwrap();
        db.set_service_mode(my, Some(ServiceMode::Started)).unwrap();
        db.set_service_mode(my, Some(ServiceMode::Stopped)).unwrap();
        assert_eq!(
            db.service_modes().unwrap(),
            [(my, ServiceMode::Stopped), (pg, ServiceMode::Started)]
        );
        db.set_service_mode(pg, None).unwrap();
        db.set_service_mode(pg, None).unwrap();
        assert_eq!(db.service_modes().unwrap(), [(my, ServiceMode::Stopped)]);

        db.conn
            .execute(
                "INSERT INTO services (name, series, mode) VALUES ('redis', '7', 'started')",
                [],
            )
            .unwrap();
        assert_eq!(db.service_modes().unwrap().len(), 1);
    }

    #[test]
    fn clients_are_pointed_at_the_instance() {
        let address = Ipv4Addr::new(127, 0, 0, 2);
        assert_eq!(
            ServiceName::Postgresql.client_variables(address, 5432),
            [
                ("PGHOST".to_owned(), "127.0.0.2".to_owned()),
                ("PGPORT".to_owned(), "5432".to_owned()),
                ("PGUSER".to_owned(), "postgres".to_owned()),
            ]
        );
        assert_eq!(
            ServiceName::Mysql.client_variables(address, 3306),
            [
                ("MYSQL_HOST".to_owned(), "127.0.0.2".to_owned()),
                ("MYSQL_TCP_PORT".to_owned(), "3306".to_owned()),
            ]
        );
    }
}
