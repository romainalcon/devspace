//! `devspace web install|start|stop|restart|status`.
//!
//! The web server is supervised by the daemon; these commands require it to
//! run.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use devspace_core::AppPaths;
use devspace_ipc::{
    DnsServerState, Endpoint, IntegrationStatus, Request, Response, SiteTargetInfo, WebServerState,
    WebStatus,
};

use crate::client::{self, unexpected};
use crate::error::CliError;

/// Time limit of `web install` (download of about 20 MB).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Time limit of `web start|restart|stop` (stop, then start, of Caddy).
const CONTROL_TIMEOUT: Duration = Duration::from_secs(45);

/// Web server subcommands.
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum WebCommand {
    /// Download and install the pinned Caddy release (verified by SHA-256).
    Install,
    /// Start the web server.
    Start,
    /// Stop the web server.
    Stop,
    /// Restart the web server, re-reading the project manifests.
    Restart,
    /// Show the web server, local domains and certificate authority.
    Status,
}

/// Runs a web subcommand.
pub fn execute(command: WebCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let (request, timeout) = match command {
        WebCommand::Install => {
            println!("installing Caddy (download and verification may take a minute)…");
            (Request::WebInstall, INSTALL_TIMEOUT)
        }
        WebCommand::Start => (Request::WebStart, CONTROL_TIMEOUT),
        WebCommand::Stop => (Request::WebStop, CONTROL_TIMEOUT),
        WebCommand::Restart => (Request::WebRestart, CONTROL_TIMEOUT),
        WebCommand::Status => (Request::WebStatus, CONTROL_TIMEOUT),
    };
    let status = client::block_on(fetch(&endpoint, request, timeout))??;
    print!("{}", format_status(&status));
    Ok(ExitCode::SUCCESS)
}

/// Sends a web request and returns the resulting status.
pub async fn fetch(
    endpoint: &Endpoint,
    request: Request,
    timeout: Duration,
) -> Result<WebStatus, CliError> {
    let mut client = client::connect(endpoint).await?;
    match client::request_with_timeout(&mut client, request, timeout).await? {
        Response::WebStatus(status) => Ok(*status),
        _ => Err(unexpected("web status")),
    }
}

/// Human-readable status.
pub fn format_status(status: &WebStatus) -> String {
    let mut out = String::new();
    // Writing to a String cannot fail.
    let caddy = if status.caddy_installed {
        format!("Caddy v{}", status.caddy_version)
    } else {
        format!(
            "Caddy v{} not installed (run `devspace web install`)",
            status.caddy_version
        )
    };
    let server = match &status.server {
        WebServerState::Stopped => "stopped".to_owned(),
        WebServerState::Starting => "starting".to_owned(),
        WebServerState::Running { pid } => format!("running (pid {pid})"),
        WebServerState::Failed { message } => format!("failed: {message}"),
    };
    let _ = writeln!(out, "web server:  {server}");
    let _ = writeln!(out, "             {caddy}");
    let _ = writeln!(
        out,
        "             listening on {}, ports {} (HTTP) and {} (HTTPS)",
        status.address, status.http_port, status.https_port
    );
    let dns = match &status.dns {
        DnsServerState::Listening => format!(
            "127.0.0.1:{} (UDP), *.{} → {}",
            status.dns_port, status.tld, status.address
        ),
        DnsServerState::Failed { message } => format!("failed: {message}"),
    };
    let _ = writeln!(out, "dns server:  {dns}");
    let ca = status.certificate_authority.as_ref().map_or_else(
        || "not created yet (start the web server once)".to_owned(),
        |ca| format!("SHA-256 {}", ca.sha256),
    );
    let _ = writeln!(out, "local CA:    {ca}");

    let system = &status.system;
    let forwarding = match (system.port_forwarding, system.forwarding_active) {
        (IntegrationStatus::Installed, Some(false)) => {
            format!(
                "installed but inactive ({}:80 does not reach the web server)",
                status.address
            )
        }
        (state, _) => integration(state).to_owned(),
    };
    let rows = [
        (
            format!("resolver for *.{}", status.tld),
            integration(system.resolver).to_owned(),
        ),
        (format!("{} + ports 80/443", status.address), forwarding),
        (
            "local CA trusted".to_owned(),
            integration(system.certificate_trust).to_owned(),
        ),
    ];
    let label_width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    for (index, (label, state)) in rows.iter().enumerate() {
        let prefix = if index == 0 {
            "system:     "
        } else {
            "            "
        };
        let _ = writeln!(out, "{prefix} {label:<label_width$}  {state}");
    }
    if [
        system.resolver,
        system.port_forwarding,
        system.certificate_trust,
    ]
    .iter()
    .any(|state| *state != IntegrationStatus::Installed)
    {
        let _ = writeln!(
            out,
            "             (run `devspace system setup` to install the missing parts)"
        );
    }

    if status.sites.is_empty() {
        let _ = writeln!(
            out,
            "sites:       none; add a project with `devspace project add`"
        );
        return out;
    }
    let _ = writeln!(out, "sites:");
    let width = status
        .sites
        .iter()
        .map(|site| site.domain.len() + "https://".len())
        .max()
        .unwrap_or_default();
    for site in &status.sites {
        let url = format!("https://{}", site.domain);
        let target = match &site.target {
            SiteTargetInfo::Static { root } => format!("static  {}", root.display()),
            SiteTargetInfo::Proxy { upstream } => format!("proxy   {upstream}"),
            SiteTargetInfo::Unavailable { reason } => format!("not served: {reason}"),
        };
        let _ = writeln!(out, "  {url:<width$}  {target}");
    }
    out
}

fn integration(state: IntegrationStatus) -> &'static str {
    match state {
        IntegrationStatus::Installed => "installed",
        IntegrationStatus::Missing => "missing",
        IntegrationStatus::Outdated => "outdated (settings changed)",
        IntegrationStatus::Conflict => "conflict (used by another tool)",
        IntegrationStatus::Unsupported => "not supported on this platform yet",
        IntegrationStatus::Unknown => "unknown (see the daemon logs)",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use devspace_ipc::{CertificateAuthorityInfo, SiteInfo, SystemIntegrations};

    use super::*;

    pub(crate) fn sample() -> WebStatus {
        WebStatus {
            caddy_version: "2.11.9".into(),
            caddy_installed: true,
            server: WebServerState::Running { pid: 42 },
            tld: "internal".into(),
            address: "127.0.0.2".into(),
            http_port: 47080,
            https_port: 47443,
            dns_port: 47053,
            dns: DnsServerState::Listening,
            certificate_authority: Some(CertificateAuthorityInfo {
                certificate_pem: String::new(),
                sha256: "AB".repeat(32),
            }),
            system: SystemIntegrations {
                resolver: IntegrationStatus::Installed,
                port_forwarding: IntegrationStatus::Installed,
                certificate_trust: IntegrationStatus::Missing,
                forwarding_active: Some(false),
            },
            sites: vec![
                SiteInfo {
                    project: "blog".into(),
                    domain: "blog.internal".into(),
                    target: SiteTargetInfo::Static {
                        root: PathBuf::from("/src/blog/public"),
                    },
                },
                SiteInfo {
                    project: "shop".into(),
                    domain: "shop.internal".into(),
                    target: SiteTargetInfo::Unavailable {
                        reason: "no `public` directory".into(),
                    },
                },
            ],
        }
    }

    #[test]
    fn status_mentions_every_part() {
        let text = format_status(&sample());
        assert!(text.contains("running (pid 42)"), "{text}");
        assert!(
            text.contains("127.0.0.1:47053 (UDP), *.internal → 127.0.0.2"),
            "{text}"
        );
        assert!(
            text.contains("listening on 127.0.0.2, ports 47080"),
            "{text}"
        );
        assert!(
            text.contains(&format!("SHA-256 {}", "AB".repeat(32))),
            "{text}"
        );
        assert!(text.contains("installed but inactive"), "{text}");
        assert!(text.contains("devspace system setup"), "{text}");
        assert!(
            text.contains("  https://blog.internal  static  /src/blog/public"),
            "{text}"
        );
        assert!(
            text.contains("  https://shop.internal  not served: no `public` directory"),
            "{text}"
        );
    }

    #[test]
    fn status_without_caddy_or_sites() {
        let mut status = sample();
        status.caddy_installed = false;
        status.server = WebServerState::Stopped;
        status.certificate_authority = None;
        status.sites.clear();
        let text = format_status(&status);
        assert!(
            text.contains("not installed (run `devspace web install`)"),
            "{text}"
        );
        assert!(text.contains("not created yet"), "{text}");
        assert!(text.contains("sites:       none"), "{text}");
    }
}
