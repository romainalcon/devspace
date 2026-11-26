//! `devspace daemon start|stop|status`.

use std::process::ExitCode;

use devspace_core::AppPaths;
use devspace_ipc::Endpoint;
use devspace_ipc::launcher;

use crate::client::{self, peer_info};
use crate::error::CliError;

/// Exit code of `devspace daemon status` when the daemon is not running
/// (same convention as LSB init scripts).
pub const NOT_RUNNING_EXIT_CODE: u8 = 3;

/// Daemon subcommands.
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum DaemonCommand {
    /// Start the daemon in the background.
    Start,
    /// Stop the running daemon gracefully.
    Stop,
    /// Show whether the daemon is running (exit code 3 when it is not).
    Status,
}

/// Runs a daemon subcommand.
pub fn execute(command: DaemonCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let peer = peer_info();
    client::block_on(async {
        match command {
            DaemonCommand::Start => {
                let executable = launcher::daemon_executable()?;
                let started = launcher::start(paths, &endpoint, &peer, &executable).await?;
                if started.already_running {
                    println!("daemon already running (pid {})", started.pid);
                } else {
                    println!("daemon started (pid {})", started.pid);
                }
                Ok(ExitCode::SUCCESS)
            }
            DaemonCommand::Stop => {
                match launcher::stop(paths, &endpoint, &peer).await? {
                    Some(pid) => println!("daemon stopped (pid {pid})"),
                    None => println!("daemon is not running"),
                }
                Ok(ExitCode::SUCCESS)
            }
            DaemonCommand::Status => status(paths, &endpoint).await,
        }
    })?
}

async fn status(paths: &AppPaths, endpoint: &Endpoint) -> Result<ExitCode, CliError> {
    let Some(status) = launcher::status(paths, endpoint, &peer_info()).await? else {
        println!("status:   not running");
        return Ok(ExitCode::from(NOT_RUNNING_EXIT_CODE));
    };
    println!("status:   running");
    println!("pid:      {}", status.pid);
    println!("version:  {}", status.version);
    println!("uptime:   {}", format_uptime(status.uptime_secs));
    println!("endpoint: {endpoint}");
    println!("logs:     {}", paths.log_dir.display());
    Ok(ExitCode::SUCCESS)
}

fn format_uptime(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (hours, rest) = (rest / 3_600, rest % 3_600);
    let (minutes, seconds) = (rest / 60, rest % 60);
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m {seconds:02}s")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_formatting() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(59), "59s");
        assert_eq!(format_uptime(61), "1m 01s");
        assert_eq!(format_uptime(3_600 + 120 + 3), "1h 02m 03s");
        assert_eq!(format_uptime(2 * 86_400 + 5), "2d 00h 00m 05s");
    }
}
