//! `devspace service list|install|start|stop`.
//!
//! The instances are run by the daemon; these commands require it.
//! `service install` installs the programs of a service like
//! `runtime install` (same request).

use std::fmt::Write as _;
use std::process::ExitCode;

use devspace_core::AppPaths;
use devspace_ipc::{
    Endpoint, Request, Response, ServiceInstanceInfo, ServiceList, ServiceMode, WebServerState,
};

use crate::client::{self, unexpected};
use crate::error::CliError;

/// Service subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ServiceCommand {
    /// List the service instances, their state and how to connect.
    List,
    /// Download, verify and install a version of a service (same as
    /// `devspace runtime install`).
    Install {
        /// Service: `mysql`, `postgresql` or `valkey`.
        name: String,
        /// `MAJOR`, `MAJOR.MINOR` or `MAJOR.MINOR.PATCH`; the newest
        /// published match is installed (the newest version if absent).
        version: Option<String>,
    },
    /// Start an instance and keep it running, also after a daemon restart.
    Start {
        /// Service: `mysql`, `postgresql` or `valkey`.
        name: String,
        /// Series: `18` (PostgreSQL), `8.4` (MySQL); the newest installed
        /// one if absent.
        series: Option<String>,
    },
    /// Stop an instance and keep it stopped, even if projects require it,
    /// until it is started again.
    Stop {
        /// Service: `mysql`, `postgresql` or `valkey`.
        name: String,
        /// Series; every series of the service if absent.
        series: Option<String>,
    },
}

/// Runs a service subcommand.
pub fn execute(command: ServiceCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let (request, target) = match command {
        ServiceCommand::List => (Request::ServiceList, None),
        ServiceCommand::Install { name, version } => {
            println!(
                "installing {name} {} (download and verification may take a few minutes)…",
                version.as_deref().unwrap_or("(newest)")
            );
            (Request::RuntimeInstall { name, version }, None)
        }
        ServiceCommand::Start { name, series } => (
            Request::ServiceStart {
                name: name.clone(),
                series: series.clone(),
            },
            Some((name, series)),
        ),
        ServiceCommand::Stop { name, series } => (Request::ServiceStop { name, series }, None),
    };
    let response = client::block_on(async {
        let mut client = client::connect(&endpoint).await?;
        client::request(&mut client, request).await
    })??;
    match response {
        Response::Services(list) => {
            print!("{}", format_list(&list));
            if let Some((name, series)) = target
                && let Some(message) = start_failure(&list, &name, series.as_deref())
            {
                eprintln!("error: {message}");
                return Ok(ExitCode::FAILURE);
            }
        }
        Response::RuntimeInstalled(runtime) if runtime.changed => println!(
            "installed {} {} in {}",
            runtime.name,
            runtime.version,
            runtime.path.display()
        ),
        Response::RuntimeInstalled(runtime) => println!(
            "{} {} is already installed ({})",
            runtime.name,
            runtime.version,
            runtime.path.display()
        ),
        _ => return Err(unexpected("service response")),
    }
    Ok(ExitCode::SUCCESS)
}

/// Why the instance a `service start` designated does not run, if so.
fn start_failure(list: &ServiceList, name: &str, series: Option<&str>) -> Option<String> {
    let mut candidates = list
        .instances
        .iter()
        .filter(|instance| instance.name == name)
        .filter(|instance| series.is_none_or(|series| instance.series == series));
    if candidates.clone().any(|instance| {
        matches!(
            instance.state,
            WebServerState::Running { .. } | WebServerState::Starting
        )
    }) {
        return None;
    }
    let instance = candidates.find(|instance| instance.mode != ServiceMode::Stopped)?;
    match &instance.state {
        WebServerState::Running { .. } | WebServerState::Starting => None,
        WebServerState::Failed { message } => Some(message.clone()),
        WebServerState::Stopped => Some(
            instance
                .blocked
                .clone()
                .unwrap_or_else(|| format!("{} {} is not running", instance.name, instance.series)),
        ),
    }
}

fn state_text(instance: &ServiceInstanceInfo) -> String {
    match &instance.state {
        WebServerState::Running { pid } => format!("running (pid {pid})"),
        WebServerState::Starting => "starting".to_owned(),
        WebServerState::Stopped => "stopped".to_owned(),
        WebServerState::Failed { .. } => "failed".to_owned(),
    }
}

fn format_list(list: &ServiceList) -> String {
    let mut out = String::new();
    // Writing to a String cannot fail.
    if list.instances.is_empty() {
        let _ = writeln!(
            out,
            "no service installed; run `devspace service install postgresql` or \
             `devspace service install mysql`"
        );
    } else {
        let _ = writeln!(
            out,
            "services on {} (administrators have no password)",
            list.address
        );
    }
    let label = |instance: &ServiceInstanceInfo| format!("{} {}", instance.name, instance.series);
    let width = list
        .instances
        .iter()
        .map(|instance| label(instance).len())
        .max()
        .unwrap_or_default();
    let version_width = list
        .instances
        .iter()
        .map(|instance| instance.version.as_deref().map_or(13, str::len))
        .max()
        .unwrap_or_default();
    let state_width = list
        .instances
        .iter()
        .map(|instance| state_text(instance).len())
        .max()
        .unwrap_or_default();
    for instance in &list.instances {
        let version = instance.version.as_deref().unwrap_or("not installed");
        let mut why = match instance.mode {
            ServiceMode::Started => "started by you".to_owned(),
            ServiceMode::Stopped => "stopped by you".to_owned(),
            ServiceMode::Automatic if instance.required_by.is_empty() => "not required".to_owned(),
            ServiceMode::Automatic => String::new(),
        };
        if !instance.required_by.is_empty() {
            if !why.is_empty() {
                why.push_str("; ");
            }
            let _ = write!(why, "required by {}", instance.required_by.join(", "));
        }
        let _ = writeln!(
            out,
            "  {:<width$}  {version:<version_width$}  {:<state_width$}  port {}  user {}  {why}",
            label(instance),
            state_text(instance),
            instance.port,
            instance.user,
        );
        if let WebServerState::Failed { message } = &instance.state {
            for line in message.lines() {
                let _ = writeln!(out, "      {line}");
            }
        }
        if let Some(blocked) = &instance.blocked {
            let _ = writeln!(out, "      {blocked}");
        }
    }
    for message in &list.missing {
        let _ = writeln!(out, "warning: {message}");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn instance(name: &str, series: &str, state: WebServerState) -> ServiceInstanceInfo {
        ServiceInstanceInfo {
            name: name.into(),
            series: series.into(),
            version: Some(format!("{series}.1.0")),
            port: 5432,
            user: "postgres".into(),
            mode: ServiceMode::Automatic,
            required_by: Vec::new(),
            state,
            blocked: None,
            data_dir: PathBuf::from("/data"),
            log_file: PathBuf::from("/log"),
        }
    }

    #[test]
    fn list_explains_every_instance() {
        let mut running = instance("postgresql", "18", WebServerState::Running { pid: 42 });
        running.required_by = vec!["blog".into(), "shop".into()];
        let mut stopped = instance("postgresql", "17", WebServerState::Stopped);
        stopped.mode = ServiceMode::Stopped;
        stopped.required_by = vec!["legacy".into()];
        let mut failed = instance(
            "mysql",
            "8.4",
            WebServerState::Failed {
                message: "port 3306 is busy".into(),
            },
        );
        failed.mode = ServiceMode::Started;
        let text = format_list(&ServiceList {
            address: "127.0.0.2".into(),
            instances: vec![failed, stopped, running],
            missing: vec!["project `x` requires mysql 9".into()],
        });
        assert!(text.starts_with("services on 127.0.0.2"), "{text}");
        // Columns are aligned; compare words.
        let words: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            words.contains(
                "postgresql 18 18.1.0 running (pid 42) port 5432 user postgres required by blog, shop"
            ),
            "{text}"
        );
        assert!(
            words.contains("stopped by you; required by legacy"),
            "{text}"
        );
        assert!(text.contains("      port 3306 is busy\n"), "{text}");
        assert!(
            text.contains("warning: project `x` requires mysql 9\n"),
            "{text}"
        );
        assert!(
            format_list(&ServiceList {
                address: "127.0.0.2".into(),
                instances: vec![],
                missing: vec![],
            })
            .starts_with("no service installed")
        );
    }

    #[test]
    fn failed_starts_are_reported() {
        let mut blocked = instance("postgresql", "17", WebServerState::Stopped);
        blocked.blocked = Some("postgresql 18 uses the port".into());
        let list = ServiceList {
            address: "127.0.0.2".into(),
            instances: vec![
                blocked,
                instance("postgresql", "18", WebServerState::Running { pid: 1 }),
            ],
            missing: vec![],
        };
        assert_eq!(start_failure(&list, "postgresql", Some("18")), None);
        assert_eq!(
            start_failure(&list, "postgresql", Some("17")).as_deref(),
            Some("postgresql 18 uses the port")
        );
        assert_eq!(start_failure(&list, "mysql", None), None);
    }
}
