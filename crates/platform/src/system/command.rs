//! Running system tools with a controlled environment.

use std::ffi::OsStr;
use std::process::{Command, Output, Stdio};

use crate::PlatformError;

/// `PATH` given to system tools, so that a modified environment cannot
/// substitute another program.
#[cfg(unix)]
const SAFE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Runs `program` (an absolute path) with `args`, a cleared environment and
/// no standard input, and returns its output whatever the exit status.
pub(crate) fn output<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Output, PlatformError> {
    let mut command = Command::new(program);
    command.args(args).env_clear().stdin(Stdio::null());
    #[cfg(unix)]
    command.env("PATH", SAFE_PATH);
    command
        .output()
        .map_err(|err| PlatformError::SystemCommand {
            command: command_line(program, args),
            detail: err.to_string(),
        })
}

/// Like [`output`], but fails unless the program exits successfully.
pub(crate) fn run<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Output, PlatformError> {
    let output = output(program, args)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(failure(program, args, &output))
    }
}

/// Error describing a failed command.
pub(crate) fn failure<S: AsRef<OsStr>>(
    program: &str,
    args: &[S],
    output: &Output,
) -> PlatformError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout.trim().to_owned()
    } else {
        stderr.trim().to_owned()
    };
    PlatformError::SystemCommand {
        command: command_line(program, args),
        detail: if text.is_empty() {
            output.status.to_string()
        } else {
            format!("{}: {text}", output.status)
        },
    }
}

fn command_line<S: AsRef<OsStr>>(program: &str, args: &[S]) -> String {
    let mut line = program.to_owned();
    for arg in args {
        line.push(' ');
        line.push_str(&arg.as_ref().to_string_lossy());
    }
    line
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn success_and_failure_are_reported() {
        assert!(run("/bin/sh", &["-c", "exit 0"]).is_ok());
        let err = run("/bin/sh", &["-c", "echo nope >&2; exit 3"]).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("/bin/sh -c"), "{message}");
        assert!(message.contains("nope"), "{message}");
    }

    #[test]
    fn environment_is_cleared() {
        let out = run("/bin/sh", &["-c", "echo \"$HOME|$PATH\""]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("|{SAFE_PATH}")
        );
    }

    #[test]
    fn missing_program_is_an_error() {
        assert!(matches!(
            run("/nonexistent/tool", &["x"]),
            Err(PlatformError::SystemCommand { .. })
        ));
    }
}
