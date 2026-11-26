//! Running the privileged helper with administrator rights.
//!
//! Two ways to obtain the rights, both authenticating the user:
//!
//! - [`Elevation::Terminal`] (Unix): `sudo`, which asks for the password on
//!   the terminal (or uses cached credentials); one `sudo` per command.
//! - [`Elevation::Graphical`] (macOS): the system authentication dialog,
//!   through `osascript` (`do shell script … with administrator
//!   privileges`); every command runs in one shell script, so the user is
//!   asked once. Used by the desktop application, which has no terminal.
//!
//! Windows is not supported yet (UAC elevation will come with the Windows
//! integrations).

use std::io::Write as _;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use crate::PlatformError;

/// How administrator rights are obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    /// `sudo` on the terminal of the caller.
    Terminal,
    /// The graphical authentication dialog of the system.
    Graphical,
}

/// One program to run with administrator rights.
#[derive(Debug, Clone, Copy)]
pub struct ElevatedCommand<'a> {
    /// Absolute path of the program.
    pub program: &'a Path,
    /// Its arguments.
    pub args: &'a [String],
    /// Data written to its standard input.
    pub stdin: Option<&'a [u8]>,
}

/// Failure of [`run_all`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElevatedFailure {
    /// Index of the failed command, when known (always with
    /// [`Elevation::Terminal`]). The commands before it succeeded, the
    /// following ones did not run.
    pub index: Option<usize>,
    /// Exit status of the failed command (or of the authentication tool).
    pub status: ExitStatus,
}

/// Runs `commands` in order with administrator rights, stopping at the
/// first failure. Their standard output and error are those of the caller.
/// `reason` is shown in the graphical authentication dialog.
///
/// The caller must have obtained the user's approval first.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] when the platform has no such
/// elevation, or [`PlatformError::SystemCommand`] if the elevation tool
/// cannot be started. A command that fails is reported in the `Ok` value.
pub fn run_all(
    elevation: Elevation,
    commands: &[ElevatedCommand<'_>],
    reason: &str,
) -> Result<Result<(), ElevatedFailure>, PlatformError> {
    match elevation {
        Elevation::Terminal => {
            for (index, command) in commands.iter().enumerate() {
                let status = run_elevated(command.program, command.args, command.stdin)?;
                if !status.success() {
                    return Ok(Err(ElevatedFailure {
                        index: Some(index),
                        status,
                    }));
                }
            }
            Ok(Ok(()))
        }
        Elevation::Graphical => {
            if commands.is_empty() {
                return Ok(Ok(()));
            }
            let mut command = imp::graphical(&shell_script(commands), reason)?;
            let description = "osascript (administrator privileges)".to_owned();
            let status = command.stdin(Stdio::null()).status().map_err(|err| {
                PlatformError::SystemCommand {
                    command: description,
                    detail: err.to_string(),
                }
            })?;
            Ok(if status.success() {
                Ok(())
            } else {
                Err(ElevatedFailure {
                    index: None,
                    status,
                })
            })
        }
    }
}

/// Runs `program` with `sudo`, passing it `args` and writing `stdin` to its
/// standard input; its standard output and error are those of the caller.
/// Waits for it to exit.
///
/// The caller must have obtained the user's approval first.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] on Windows, or
/// [`PlatformError::SystemCommand`] if the program cannot be started.
pub fn run_elevated(
    program: &Path,
    args: &[String],
    stdin: Option<&[u8]>,
) -> Result<ExitStatus, PlatformError> {
    let mut command = imp::sudo(program, args)?;
    let description = format!("{command:?}");
    let failed = |detail: String| PlatformError::SystemCommand {
        command: description.clone(),
        detail,
    };
    command.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn().map_err(|err| failed(err.to_string()))?;
    if let Some(data) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        // The helper may exit before reading everything (e.g. after a
        // refused password); its exit status reports that case.
        let _ = pipe.write_all(data);
    }
    child.wait().map_err(|err| failed(err.to_string()))
}

/// POSIX shell script running `commands` in order and stopping at the
/// first failure; standard input is given with `printf`. Every value is
/// single-quoted, so nothing in it is interpreted by the shell.
fn shell_script(commands: &[ElevatedCommand<'_>]) -> String {
    let mut script = String::from("set -e\n");
    for command in commands {
        if let Some(data) = command.stdin {
            script.push_str("printf '%s' ");
            script.push_str(&quote(&String::from_utf8_lossy(data)));
            script.push_str(" | ");
        }
        script.push_str(&quote(&command.program.to_string_lossy()));
        for arg in command.args {
            script.push(' ');
            script.push_str(&quote(arg));
        }
        script.push('\n');
    }
    script
}

/// Single-quotes `value` for a POSIX shell.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(unix)]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use crate::PlatformError;

    /// Absolute paths, so that the `PATH` cannot substitute another program.
    const SUDO: &str = "/usr/bin/sudo";
    #[cfg(target_os = "macos")]
    const OSASCRIPT: &str = "/usr/bin/osascript";

    /// Environment variable carrying the script to `osascript`, so that it
    /// never has to be escaped as an AppleScript string.
    #[cfg(target_os = "macos")]
    const SCRIPT_VARIABLE: &str = "DEVSPACE_ELEVATED_SCRIPT";
    #[cfg(target_os = "macos")]
    const REASON_VARIABLE: &str = "DEVSPACE_ELEVATION_REASON";

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn sudo(program: &Path, args: &[String]) -> Result<Command, PlatformError> {
        let mut command = Command::new(SUDO);
        command.arg("--").arg(program).args(args);
        Ok(command)
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn graphical(script: &str, reason: &str) -> Result<Command, PlatformError> {
        let mut command = Command::new(OSASCRIPT);
        command
            .arg("-e")
            .arg(format!(
                "do shell script (system attribute \"{SCRIPT_VARIABLE}\") with prompt \
                 (system attribute \"{REASON_VARIABLE}\") with administrator privileges \
                 without altering line endings"
            ))
            .env(SCRIPT_VARIABLE, script)
            .env(REASON_VARIABLE, reason);
        Ok(command)
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn graphical(_script: &str, _reason: &str) -> Result<Command, PlatformError> {
        Err(PlatformError::Unsupported {
            feature: "graphical elevation",
        })
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use crate::PlatformError;

    pub(super) fn sudo(_program: &Path, _args: &[String]) -> Result<Command, PlatformError> {
        Err(PlatformError::Unsupported {
            feature: "running the privileged helper",
        })
    }

    pub(super) fn graphical(_script: &str, _reason: &str) -> Result<Command, PlatformError> {
        Err(PlatformError::Unsupported {
            feature: "graphical elevation",
        })
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn values_are_single_quoted() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$(rm -rf /) `x` \"y\""), "'$(rm -rf /) `x` \"y\"'");
    }

    #[test]
    fn script_runs_every_command_with_its_input() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let program = Path::new("/bin/sh");
        let first = vec![
            "-c".to_owned(),
            format!("cat > '{}'", out.display()),
            "ignored $HOME".to_owned(),
        ];
        let second = vec!["-c".to_owned(), "echo second; exit 0".to_owned()];
        let pem = "-----BEGIN CERTIFICATE-----\nAB'C\n-----END CERTIFICATE-----\n";
        let script = shell_script(&[
            ElevatedCommand {
                program,
                args: &first,
                stdin: Some(pem.as_bytes()),
            },
            ElevatedCommand {
                program,
                args: &second,
                stdin: None,
            },
        ]);
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .status()
            .unwrap();
        assert!(status.success(), "{script}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), pem);
    }

    #[test]
    fn script_stops_at_the_first_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("marker");
        let program = Path::new("/bin/sh");
        let fail = vec!["-c".to_owned(), "exit 3".to_owned()];
        let touch = vec!["-c".to_owned(), format!("touch '{}'", marker.display())];
        let script = shell_script(&[
            ElevatedCommand {
                program,
                args: &fail,
                stdin: None,
            },
            ElevatedCommand {
                program,
                args: &touch,
                stdin: None,
            },
        ]);
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(3));
        assert!(!marker.exists());
    }
}
