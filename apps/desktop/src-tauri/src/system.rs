//! `devspace system setup|teardown` from the application.
//!
//! The plan and the privileged changes stay in the CLI (no duplicated
//! logic): the application runs the bundled `devspace` with `--dry-run` to
//! show the changes, then, once the user approved them, with `--yes
//! --graphical`, which asks for the administrator password with the system
//! dialog. The CLI recomputes the plan and still refuses to touch files
//! DevSpace does not manage.

use std::path::PathBuf;
use std::process::Stdio;

use devspace_platform::target::executable_name;
use serde::{Deserialize, Serialize};

use crate::daemon::DesktopError;

/// Which system command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAction {
    /// `devspace system setup`.
    Setup,
    /// `devspace system teardown`.
    Teardown,
}

impl SystemAction {
    const fn name(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Teardown => "teardown",
        }
    }
}

/// Output of the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemOutput {
    /// Whether the command succeeded.
    pub success: bool,
    /// Standard output (the plan, the helper messages).
    pub stdout: String,
    /// Standard error (errors).
    pub stderr: String,
}

/// The `devspace` CLI next to the application.
fn cli() -> Result<PathBuf, DesktopError> {
    let exe = std::env::current_exe()
        .map_err(|err| DesktopError::Shell(format!("cannot locate the application: {err}")))?;
    let cli = exe.with_file_name(executable_name("devspace"));
    if cli.is_file() {
        Ok(cli)
    } else {
        Err(DesktopError::Shell(format!(
            "the `devspace` command was not found at `{}`",
            cli.display()
        )))
    }
}

/// Runs `devspace system <action>`: only showing the plan when `apply` is
/// false, otherwise applying it with the graphical elevation.
///
/// # Errors
///
/// [`DesktopError::Shell`] if the CLI cannot be found or started.
pub async fn run(action: SystemAction, apply: bool) -> Result<SystemOutput, DesktopError> {
    let mut command = tokio::process::Command::new(cli()?);
    command.args(["system", action.name()]);
    if apply {
        command.args(["--yes", "--graphical"]);
    } else {
        command.arg("--dry-run");
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|err| DesktopError::Shell(format!("failed to run `devspace`: {err}")))?;
    Ok(SystemOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
