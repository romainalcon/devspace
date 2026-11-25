//! Running the privileged helper with administrator rights.
//!
//! Unix: through `sudo`, which authenticates the user (password prompt on
//! the terminal, or cached credentials). Windows: not supported yet (UAC
//! elevation will come with the Windows integrations).

use std::io::Write as _;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use crate::PlatformError;

/// Runs `program` with administrator rights, passing it `args` and writing
/// `stdin` to its standard input; its standard output and error are those
/// of the caller. Waits for it to exit.
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
    let mut command = imp::command(program, args)?;
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

#[cfg(unix)]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use crate::PlatformError;

    /// Absolute path, so that the `PATH` cannot substitute another program.
    const SUDO: &str = "/usr/bin/sudo";

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn command(program: &Path, args: &[String]) -> Result<Command, PlatformError> {
        let mut command = Command::new(SUDO);
        command.arg("--").arg(program).args(args);
        Ok(command)
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use crate::PlatformError;

    pub(super) fn command(_program: &Path, _args: &[String]) -> Result<Command, PlatformError> {
        Err(PlatformError::Unsupported {
            feature: "running the privileged helper",
        })
    }
}
