//! `devspace system setup|teardown`: the machine-wide integrations that
//! need administrator rights.
//!
//! The CLI computes the plan from the daemon's web status, shows every
//! change, asks for confirmation, then runs the privileged helper through
//! `sudo` once per change. Nothing is changed when an integration conflicts
//! with a file DevSpace does not manage, or when the user declines.

use std::io::{self, BufRead as _, IsTerminal as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use devspace_core::AppPaths;
use devspace_helper::{Certificate, HELPER_NAME, LoopbackAddress, Operation, Tld};
use devspace_ipc::{Endpoint, IntegrationStatus, Request, WebServerState, WebStatus};
use devspace_platform::system::port_forwarding::{self, Forwarding};
use devspace_platform::system::{elevation, resolver, trust_store};
use devspace_platform::target::executable_name;

use crate::client;
use crate::error::CliError;
use crate::web;

/// System subcommands.
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum SystemCommand {
    /// Install the local DNS resolver, the loopback address of the sites
    /// with the forwarding of its ports 80/443, and the trust of the local
    /// certificate authority (asks for approval, then for your password
    /// through sudo).
    Setup {
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Remove everything `setup` installed.
    Teardown {
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// One privileged change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Operation performed by the helper.
    pub operation: Operation,
    /// What changes, for the approval prompt.
    pub description: String,
    /// Certificate given to the helper on standard input.
    pub certificate: Option<String>,
}

/// Runs a system subcommand.
pub fn execute(command: SystemCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let status = client::block_on(web::fetch(&endpoint, Request::WebStatus, STATUS_TIMEOUT))??;
    match command {
        SystemCommand::Setup { yes } => setup(&endpoint, &status, yes),
        SystemCommand::Teardown { yes } => {
            let plan = teardown_plan(&status)?;
            if plan.is_empty() {
                println!("nothing to do");
                return Ok(ExitCode::SUCCESS);
            }
            apply(&plan, yes)
        }
    }
}

/// Time limit of the status requests.
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Time limit of a web server start.
const START_TIMEOUT: Duration = Duration::from_secs(45);

/// Setup runs in two phases when the local CA does not exist yet: the
/// network integrations first, then (once the web server could start on
/// the sites address and create the CA) the trust of the CA, approved
/// separately with its fingerprint.
fn setup(endpoint: &Endpoint, status: &WebStatus, yes: bool) -> Result<ExitCode, CliError> {
    let plan = setup_plan(status)?;
    let waits_for_ca = trust_waits_for_ca(status);
    if waits_for_ca && !status.caddy_installed {
        return Err(CliError::SetupBlocked {
            reason: "the web server is not installed, so the local certificate authority \
                     cannot be created; run `devspace web install` first"
                .to_owned(),
        });
    }
    if plan.is_empty() && !waits_for_ca {
        println!("nothing to do");
        return Ok(ExitCode::SUCCESS);
    }
    if !plan.is_empty() {
        let code = apply(&plan, yes)?;
        if code != ExitCode::SUCCESS {
            return Ok(code);
        }
    }

    // The web server can now listen on the sites address.
    let mut status = client::block_on(web::fetch(endpoint, Request::WebStatus, STATUS_TIMEOUT))??;
    if status.caddy_installed && !matches!(status.server, WebServerState::Running { .. }) {
        println!("starting the web server…");
        status = client::block_on(web::fetch(endpoint, Request::WebStart, START_TIMEOUT))??;
    }
    if waits_for_ca {
        let plan = setup_plan(&status)?;
        if plan.is_empty() {
            return Err(CliError::SetupBlocked {
                reason: "the web server did not create the local certificate authority; see \
                         `devspace web status`"
                    .to_owned(),
            });
        }
        return apply(&plan, yes);
    }
    println!("done; check with `devspace web status`");
    Ok(ExitCode::SUCCESS)
}

/// Shows `plan`, asks for approval and runs each step through the helper.
fn apply(plan: &[Step], yes: bool) -> Result<ExitCode, CliError> {
    let helper = helper_executable()?;
    println!("DevSpace will make these changes with administrator rights:");
    for (index, step) in plan.iter().enumerate() {
        println!("  {}. {}", index + 1, step.description);
    }
    println!(
        "Each change runs `sudo {} <operation>`; sudo may ask for your password.",
        helper.display()
    );
    if !yes && !confirm()? {
        println!("cancelled; nothing was changed");
        return Ok(ExitCode::FAILURE);
    }
    for step in plan {
        let status = elevation::run_elevated(
            &helper,
            &step.operation.to_args(),
            step.certificate.as_deref().map(str::as_bytes),
        )?;
        if !status.success() {
            return Err(CliError::HelperFailed {
                operation: step.operation.name(),
                status,
            });
        }
    }
    println!("done; check with `devspace web status`");
    Ok(ExitCode::SUCCESS)
}

/// Whether the CA must be trusted but does not exist yet.
fn trust_waits_for_ca(status: &WebStatus) -> bool {
    needs_install(status.system.certificate_trust) && status.certificate_authority.is_none()
}

/// Domain and address of the sites announced by the daemon.
fn sites_location(status: &WebStatus) -> Result<(Tld, LoopbackAddress), CliError> {
    let invalid = |reason: String| CliError::SetupBlocked {
        reason: format!("the daemon sent an invalid setting: {reason}"),
    };
    Ok((
        status.tld.parse().map_err(invalid)?,
        status.address.parse().map_err(invalid)?,
    ))
}

/// Changes needed to make the local sites reachable. The trust of the CA
/// is only planned once the CA exists (see [`trust_waits_for_ca`]).
///
/// # Errors
///
/// Fails without planning anything if an integration conflicts with
/// another tool or if the platform does not support it.
pub fn setup_plan(status: &WebStatus) -> Result<Vec<Step>, CliError> {
    let (tld, address) = sites_location(status)?;
    let system = &status.system;
    let mut conflicts = Vec::new();
    for (state, what) in [
        (system.resolver, format!("the resolver file for *.{tld}")),
        (
            system.port_forwarding,
            "the port forwarding files".to_owned(),
        ),
        (system.certificate_trust, "the certificate trust".to_owned()),
    ] {
        match state {
            IntegrationStatus::Conflict => conflicts.push(what),
            IntegrationStatus::Unsupported => {
                return Err(CliError::SetupBlocked {
                    reason: format!("{what} is not supported on this platform yet"),
                });
            }
            _ => {}
        }
    }
    if !conflicts.is_empty() {
        return Err(CliError::SetupBlocked {
            reason: format!(
                "already in use by another tool (such as Laravel Herd or Valet), which \
                 DevSpace never modifies: {}. Choose another `web.tld` in the configuration, \
                 or remove that tool's configuration",
                conflicts.join(", ")
            ),
        });
    }

    let mut plan = Vec::new();
    if needs_install(system.resolver) {
        plan.push(Step {
            description: resolver::describe_install(tld.as_str(), status.dns_port)?,
            operation: Operation::InstallResolver {
                tld,
                port: status.dns_port,
            },
            certificate: None,
        });
    }
    if needs_install(system.port_forwarding) {
        let target = Forwarding::new(address.ip(), status.http_port, status.https_port)?;
        plan.push(Step {
            operation: Operation::InstallPortForwarding {
                address,
                http_port: status.http_port,
                https_port: status.https_port,
            },
            description: port_forwarding::describe_install(target)?,
            certificate: None,
        });
    }
    if needs_install(system.certificate_trust)
        && let Some(certificate) = local_ca(status)?
    {
        let sha256 = certificate.fingerprint();
        plan.push(Step {
            operation: Operation::TrustCertificate { sha256 },
            description: trust_store::describe_add(&sha256.to_colon_hex())?,
            certificate: Some(certificate.to_pem()),
        });
    }
    Ok(plan)
}

/// Changes undoing [`setup_plan`]; integrations owned by another tool are
/// left alone.
///
/// # Errors
///
/// Fails if a description cannot be produced (unsupported platform).
pub fn teardown_plan(status: &WebStatus) -> Result<Vec<Step>, CliError> {
    let (tld, address) = sites_location(status)?;
    let system = &status.system;
    let mut plan = Vec::new();
    if is_ours(system.certificate_trust)
        && let Some(certificate) = local_ca(status)?
    {
        let sha256 = certificate.fingerprint();
        plan.push(Step {
            operation: Operation::UntrustCertificate { sha256 },
            description: trust_store::describe_remove(&sha256.to_colon_hex())?,
            certificate: Some(certificate.to_pem()),
        });
    }
    if is_ours(system.port_forwarding) {
        plan.push(Step {
            description: port_forwarding::describe_remove(address.ip())?,
            operation: Operation::RemovePortForwarding { address },
            certificate: None,
        });
    }
    if is_ours(system.resolver) {
        plan.push(Step {
            description: resolver::describe_remove(tld.as_str())?,
            operation: Operation::RemoveResolver { tld },
            certificate: None,
        });
    }
    Ok(plan)
}

fn needs_install(state: IntegrationStatus) -> bool {
    matches!(
        state,
        IntegrationStatus::Missing | IntegrationStatus::Outdated | IntegrationStatus::Unknown
    )
}

fn is_ours(state: IntegrationStatus) -> bool {
    matches!(
        state,
        IntegrationStatus::Installed | IntegrationStatus::Outdated
    )
}

/// The local CA certificate sent by the daemon, checked against the
/// fingerprint it announced.
fn local_ca(status: &WebStatus) -> Result<Option<Certificate>, CliError> {
    let Some(info) = &status.certificate_authority else {
        return Ok(None);
    };
    let certificate =
        Certificate::from_pem(&info.certificate_pem).map_err(|err| CliError::SetupBlocked {
            reason: format!("the daemon sent an invalid local CA certificate: {err}"),
        })?;
    if certificate.fingerprint().to_string() != info.sha256 {
        return Err(CliError::SetupBlocked {
            reason: "the local CA fingerprint sent by the daemon does not match its certificate"
                .to_owned(),
        });
    }
    Ok(Some(certificate))
}

/// The helper is installed next to the `devspace` executable.
fn helper_executable() -> Result<PathBuf, CliError> {
    let cli = std::env::current_exe().map_err(CliError::CurrentExe)?;
    let path = cli.with_file_name(executable_name(HELPER_NAME));
    if path.is_file() {
        Ok(path)
    } else {
        Err(CliError::HelperNotFound { path })
    }
}

fn confirm() -> Result<bool, CliError> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Err(CliError::SetupBlocked {
            reason: "confirmation needed but standard input is not a terminal; review the \
                     changes above and pass `--yes` to approve them"
                .to_owned(),
        });
    }
    print!("Proceed? [y/N] ");
    io::stdout().flush().map_err(CliError::Prompt)?;
    let mut answer = String::new();
    stdin
        .lock()
        .read_line(&mut answer)
        .map_err(CliError::Prompt)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::tests::sample;

    fn sample_with_ca() -> WebStatus {
        let mut status = sample();
        let der: Vec<u8> = std::iter::once(0x30).chain([7; 40]).collect();
        let certificate = Certificate::from_der(der).unwrap();
        let info = status.certificate_authority.as_mut().unwrap();
        info.certificate_pem = certificate.to_pem();
        info.sha256 = certificate.fingerprint().to_string();
        status
    }

    fn operations(plan: &[Step]) -> Vec<&'static str> {
        plan.iter().map(|step| step.operation.name()).collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn setup_installs_only_what_is_missing() {
        let mut status = sample_with_ca();
        let plan = setup_plan(&status).unwrap();
        assert_eq!(operations(&plan), ["trust-certificate"]);
        assert!(
            plan[0]
                .certificate
                .as_deref()
                .unwrap()
                .contains("BEGIN CERTIFICATE")
        );

        status.system.resolver = IntegrationStatus::Outdated;
        status.system.port_forwarding = IntegrationStatus::Missing;
        status.system.certificate_trust = IntegrationStatus::Installed;
        let plan = setup_plan(&status).unwrap();
        assert_eq!(
            operations(&plan),
            ["install-resolver", "install-port-forwarding"]
        );
        assert_eq!(
            plan[0].operation,
            Operation::InstallResolver {
                tld: Tld::default(),
                port: 47053
            }
        );
        assert_eq!(
            plan[1].operation,
            Operation::InstallPortForwarding {
                address: LoopbackAddress::default(),
                http_port: 47080,
                https_port: 47443
            }
        );
        assert!(plan[1].description.contains("127.0.0.2"));

        status.system.resolver = IntegrationStatus::Installed;
        status.system.port_forwarding = IntegrationStatus::Installed;
        assert!(setup_plan(&status).unwrap().is_empty());
    }

    #[test]
    fn conflicts_block_the_whole_setup() {
        let mut status = sample_with_ca();
        status.system.resolver = IntegrationStatus::Conflict;
        let err = setup_plan(&status).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("never modifies"), "{message}");
        assert!(
            message.contains("the resolver file for *.internal"),
            "{message}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn trust_waits_until_the_local_ca_exists() {
        let mut status = sample();
        status.certificate_authority = None;
        status.system.port_forwarding = IntegrationStatus::Missing;
        status.system.certificate_trust = IntegrationStatus::Missing;
        assert!(trust_waits_for_ca(&status));
        assert_eq!(
            operations(&setup_plan(&status).unwrap()),
            ["install-port-forwarding"]
        );
    }

    #[test]
    fn invalid_settings_from_the_daemon_are_refused() {
        let mut status = sample_with_ca();
        status.address = "10.0.0.1".into();
        assert!(setup_plan(&status).is_err());
        assert!(teardown_plan(&status).is_err());
    }

    #[test]
    fn mismatched_fingerprint_is_refused() {
        let mut status = sample_with_ca();
        status.certificate_authority.as_mut().unwrap().sha256 = "00".repeat(32);
        assert!(setup_plan(&status).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn teardown_removes_only_what_devspace_installed() {
        let mut status = sample_with_ca();
        status.system.certificate_trust = IntegrationStatus::Installed;
        status.system.resolver = IntegrationStatus::Conflict;
        let plan = teardown_plan(&status).unwrap();
        assert_eq!(
            operations(&plan),
            ["untrust-certificate", "remove-port-forwarding"]
        );
    }
}
