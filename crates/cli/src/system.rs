//! `devspace system setup|teardown`: the machine-wide integrations that
//! need administrator rights.
//!
//! The CLI computes the plan from the daemon's web status, shows every
//! change, asks for confirmation, then runs the privileged helper through
//! `sudo` once per change. Nothing is changed when an integration conflicts
//! with a file DevSpace does not manage, or when the user declines.

use std::io::{self, BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use devspace_core::AppPaths;
use devspace_helper::{Certificate, Fingerprint, HELPER_NAME, LoopbackAddress, Operation, Tld};
use devspace_ipc::{Endpoint, IntegrationStatus, Request, WebServerState, WebStatus};
use devspace_platform::PlatformError;
use devspace_platform::system::elevation::{self, ElevatedCommand, Elevation};
use devspace_platform::system::port_forwarding::{self, Forwarding};
use devspace_platform::system::{
    IntegrationState, cli_link, helper_install, resolver, trust_store,
};
use devspace_platform::target::executable_name;

use crate::client;
use crate::error::CliError;
use crate::web;

/// System subcommands.
#[derive(Debug, Clone, Copy, clap::Subcommand)]
pub enum SystemCommand {
    /// Install the local DNS resolver, the loopback address of the sites
    /// with the forwarding of its ports 80/443, the trust of the local
    /// certificate authority and, on macOS, the administrator-owned copy of
    /// the privileged helper and the `devspace` command of the application
    /// (asks for approval, then for your password through sudo).
    Setup {
        #[command(flatten)]
        options: Options,
    },
    /// Remove everything `setup` installed.
    Teardown {
        #[command(flatten)]
        options: Options,
    },
}

/// Options shared by `setup` and `teardown`.
#[derive(Debug, Clone, Copy, clap::Args)]
pub struct Options {
    /// Do not ask for confirmation.
    #[arg(long)]
    yes: bool,
    /// Only show the changes, without making them.
    #[arg(long, conflicts_with = "yes")]
    dry_run: bool,
    /// Ask for the administrator password with the system dialog instead of
    /// sudo (macOS; used by the desktop application).
    #[arg(long)]
    graphical: bool,
}

impl Options {
    const fn elevation(self) -> Elevation {
        if self.graphical {
            Elevation::Graphical
        } else {
            Elevation::Terminal
        }
    }
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

/// The programs next to the running CLI and their installed counterparts.
#[derive(Debug, Clone)]
pub struct LocalInstall {
    /// Helper next to the CLI.
    bundled_helper: PathBuf,
    /// Installed copy of the helper; `None` on platforms without one.
    helper: Option<HelperCopy>,
    /// Link of the `devspace` command; `None` when the CLI is not that of an
    /// application bundle or the platform does not link it.
    cli_link: Option<CliLink>,
}

#[derive(Debug, Clone)]
struct HelperCopy {
    installed: PathBuf,
    state: IntegrationState,
    sha256: Fingerprint,
}

#[derive(Debug, Clone)]
struct CliLink {
    target: PathBuf,
    state: IntegrationState,
}

impl LocalInstall {
    /// Inspects the helper and the CLI of the running installation.
    fn detect() -> Result<Self, CliError> {
        let cli = std::env::current_exe().map_err(CliError::CurrentExe)?;
        let bundled_helper = cli.with_file_name(executable_name(HELPER_NAME));
        if !bundled_helper.is_file() {
            return Err(CliError::HelperNotFound {
                path: bundled_helper,
            });
        }
        let helper = match helper_install::installed_path() {
            Ok(installed) => Some(HelperCopy {
                installed,
                state: helper_install::state(&bundled_helper)?,
                sha256: Fingerprint::of(&helper_install::read_source(&bundled_helper)?),
            }),
            Err(PlatformError::Unsupported { .. }) => None,
            Err(err) => return Err(err.into()),
        };
        let cli_link = if cli_link::is_bundled_cli(&cli) && cli_link::link_path().is_ok() {
            Some(CliLink {
                state: cli_link::state(&cli)?,
                target: cli,
            })
        } else {
            None
        };
        Ok(Self {
            bundled_helper,
            helper,
            cli_link,
        })
    }

    /// Program running `operation`: the installed helper, except to install
    /// it (or where there is none).
    fn program_for(&self, operation: &Operation) -> &Path {
        match (&self.helper, operation) {
            (Some(copy), op) if !matches!(op, Operation::InstallHelper { .. }) => &copy.installed,
            _ => &self.bundled_helper,
        }
    }
}

/// Runs a system subcommand.
pub fn execute(command: SystemCommand, paths: &AppPaths) -> Result<ExitCode, CliError> {
    let endpoint = Endpoint::new(&paths.runtime_dir)?;
    let status = client::block_on(web::fetch(&endpoint, Request::WebStatus))??;
    let local = LocalInstall::detect()?;
    match command {
        SystemCommand::Setup { options } => setup(&endpoint, &status, &local, options),
        SystemCommand::Teardown { options } => {
            let plan = teardown_plan(&status, &local)?;
            if plan.is_empty() {
                println!("nothing to do");
                return Ok(ExitCode::SUCCESS);
            }
            apply(&plan, &local, options)
        }
    }
}

/// Setup runs in two phases when the local CA does not exist yet: the
/// network integrations first, then (once the web server could start on
/// the sites address and create the CA) the trust of the CA, approved
/// separately with its fingerprint.
fn setup(
    endpoint: &Endpoint,
    status: &WebStatus,
    local: &LocalInstall,
    options: Options,
) -> Result<ExitCode, CliError> {
    let plan = setup_plan(status, local)?;
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
    if options.dry_run {
        if !plan.is_empty() {
            show(&plan, local, options.elevation());
        }
        if waits_for_ca {
            println!(
                "Then, once the web server created the local certificate authority, DevSpace \
                 will ask to trust it, showing its fingerprint."
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !plan.is_empty() {
        let code = apply(&plan, local, options)?;
        if code != ExitCode::SUCCESS {
            return Ok(code);
        }
    }

    // The web server can now listen on the sites address.
    let mut status = client::block_on(web::fetch(endpoint, Request::WebStatus))??;
    if status.caddy_installed && !matches!(status.server, WebServerState::Running { .. }) {
        println!("starting the web server…");
        status = client::block_on(web::fetch(endpoint, Request::WebStart))??;
    }
    if waits_for_ca {
        // The helper may have been installed by the first phase.
        let local = LocalInstall::detect()?;
        let plan = setup_plan(&status, &local)?;
        if plan.is_empty() {
            return Err(CliError::SetupBlocked {
                reason: "the web server did not create the local certificate authority; see \
                         `devspace web status`"
                    .to_owned(),
            });
        }
        return apply(&plan, &local, options);
    }
    println!("done; check with `devspace web status`");
    Ok(ExitCode::SUCCESS)
}

fn show(plan: &[Step], local: &LocalInstall, elevation: Elevation) {
    println!("DevSpace will make these changes with administrator rights:");
    for (index, step) in plan.iter().enumerate() {
        println!("  {}. {}", index + 1, step.description);
    }
    match elevation {
        Elevation::Terminal => {
            let installs = matches!(plan[0].operation, Operation::InstallHelper { .. });
            if installs {
                println!(
                    "Change 1 runs `sudo {} install-helper …`.",
                    local.bundled_helper.display()
                );
            }
            if let Some(last) = plan.last()
                && (!installs || plan.len() > 1)
            {
                println!(
                    "{} `sudo {} <operation>`; sudo may ask for your password.",
                    if installs {
                        "The others run"
                    } else {
                        "Each change runs"
                    },
                    local.program_for(&last.operation).display()
                );
            }
        }
        Elevation::Graphical => {
            println!("The system asks once for an administrator password.");
        }
    }
}

/// Shows `plan`, asks for approval (unless `--yes`) and runs each step
/// through the helper; with `--dry-run`, only shows it.
fn apply(plan: &[Step], local: &LocalInstall, options: Options) -> Result<ExitCode, CliError> {
    show(plan, local, options.elevation());
    if options.dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    if !options.yes && !confirm()? {
        println!("cancelled; nothing was changed");
        return Ok(ExitCode::FAILURE);
    }
    let args: Vec<Vec<String>> = plan.iter().map(|step| step.operation.to_args()).collect();
    let commands: Vec<ElevatedCommand<'_>> = plan
        .iter()
        .zip(&args)
        .map(|(step, args)| ElevatedCommand {
            program: local.program_for(&step.operation),
            args,
            stdin: step.certificate.as_deref().map(str::as_bytes),
        })
        .collect();
    if let Err(failure) = elevation::run_all(
        options.elevation(),
        &commands,
        "DevSpace wants to configure local domains and HTTPS.",
    )? {
        return Err(CliError::HelperFailed {
            operation: failure
                .index
                .map(|index| format!("`{HELPER_NAME} {}`", plan[index].operation.name())),
            status: failure.status,
        });
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
pub fn setup_plan(status: &WebStatus, local: &LocalInstall) -> Result<Vec<Step>, CliError> {
    let (tld, address) = sites_location(status)?;
    let system = &status.system;
    let mut conflicts = Vec::new();
    if let Some(link) = &local.cli_link
        && link.state == IntegrationState::Conflict
    {
        conflicts.push(format!("`{}`", cli_link::link_path()?.display()));
    }
    if let Some(copy) = &local.helper
        && copy.state == IntegrationState::Conflict
    {
        conflicts.push(format!("`{}`", copy.installed.display()));
    }
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
    if let Some(link) = &local.cli_link
        && matches!(
            link.state,
            IntegrationState::Missing | IntegrationState::Outdated
        )
    {
        plan.push(Step {
            description: cli_link::describe_install(&link.target)?,
            operation: Operation::LinkCli {
                target: link.target.clone(),
            },
            certificate: None,
        });
    }
    with_installed_helper(plan, local)
}

/// Prepends the installation of the helper to a non-empty `plan` when the
/// installed copy is missing or differs from the helper of this CLI.
fn with_installed_helper(mut plan: Vec<Step>, local: &LocalInstall) -> Result<Vec<Step>, CliError> {
    if let Some(copy) = &local.helper
        && !plan.is_empty()
        && copy.state != IntegrationState::Installed
    {
        plan.insert(
            0,
            Step {
                description: helper_install::describe_install(&copy.sha256.to_colon_hex())?,
                operation: Operation::InstallHelper {
                    sha256: copy.sha256,
                },
                certificate: None,
            },
        );
    }
    Ok(plan)
}

/// Changes undoing [`setup_plan`]; integrations owned by another tool are
/// left alone.
///
/// # Errors
///
/// Fails if a description cannot be produced (unsupported platform).
pub fn teardown_plan(status: &WebStatus, local: &LocalInstall) -> Result<Vec<Step>, CliError> {
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
    if let Some(link) = &local.cli_link
        && matches!(
            link.state,
            IntegrationState::Installed | IntegrationState::Outdated
        )
    {
        plan.push(Step {
            description: cli_link::describe_remove()?,
            operation: Operation::UnlinkCli,
            certificate: None,
        });
    }
    if let Some(copy) = &local.helper
        && (!plan.is_empty()
            || matches!(
                copy.state,
                IntegrationState::Installed | IntegrationState::Outdated
            ))
    {
        plan.push(Step {
            description: helper_install::describe_remove()?,
            operation: Operation::RemoveHelper,
            certificate: None,
        });
    }
    with_installed_helper(plan, local)
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

    /// No installed helper, no link (the platforms without them).
    fn none() -> LocalInstall {
        LocalInstall {
            bundled_helper: "/opt/devspace/devspace-helper".into(),
            helper: None,
            cli_link: None,
        }
    }

    fn bundled(helper: IntegrationState, link: IntegrationState) -> LocalInstall {
        LocalInstall {
            bundled_helper: "/Applications/DevSpace.app/Contents/MacOS/devspace-helper".into(),
            helper: Some(HelperCopy {
                installed: "/Library/PrivilegedHelperTools/me.romainalcon.devspace.helper".into(),
                state: helper,
                sha256: Fingerprint::of(b"helper"),
            }),
            cli_link: Some(CliLink {
                target: "/Applications/DevSpace.app/Contents/MacOS/devspace".into(),
                state: link,
            }),
        }
    }

    fn operations(plan: &[Step]) -> Vec<&'static str> {
        plan.iter().map(|step| step.operation.name()).collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn setup_installs_only_what_is_missing() {
        let mut status = sample_with_ca();
        let plan = setup_plan(&status, &none()).unwrap();
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
        let plan = setup_plan(&status, &none()).unwrap();
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
        assert!(setup_plan(&status, &none()).unwrap().is_empty());
    }

    #[test]
    fn conflicts_block_the_whole_setup() {
        let mut status = sample_with_ca();
        status.system.resolver = IntegrationStatus::Conflict;
        let err = setup_plan(&status, &none()).unwrap_err();
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
            operations(&setup_plan(&status, &none()).unwrap()),
            ["install-port-forwarding"]
        );
    }

    #[test]
    fn invalid_settings_from_the_daemon_are_refused() {
        let mut status = sample_with_ca();
        status.address = "10.0.0.1".into();
        assert!(setup_plan(&status, &none()).is_err());
        assert!(teardown_plan(&status, &none()).is_err());
    }

    #[test]
    fn mismatched_fingerprint_is_refused() {
        let mut status = sample_with_ca();
        status.certificate_authority.as_mut().unwrap().sha256 = "00".repeat(32);
        assert!(setup_plan(&status, &none()).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn teardown_removes_only_what_devspace_installed() {
        let mut status = sample_with_ca();
        status.system.certificate_trust = IntegrationStatus::Installed;
        status.system.resolver = IntegrationStatus::Conflict;
        let plan = teardown_plan(&status, &none()).unwrap();
        assert_eq!(
            operations(&plan),
            ["untrust-certificate", "remove-port-forwarding"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_helper_is_installed_first_and_runs_from_its_copy() {
        use IntegrationState::{Installed, Missing, Outdated};
        let mut status = sample_with_ca();
        status.system.certificate_trust = IntegrationStatus::Installed;

        // Nothing to change: the helper is not installed for nothing.
        assert!(
            setup_plan(&status, &bundled(Missing, Installed))
                .unwrap()
                .is_empty()
        );

        let local = bundled(Outdated, Missing);
        let plan = setup_plan(&status, &local).unwrap();
        assert_eq!(operations(&plan), ["install-helper", "link-cli"]);
        assert_eq!(
            plan[0].operation,
            Operation::InstallHelper {
                sha256: Fingerprint::of(b"helper")
            }
        );
        assert!(
            plan[0]
                .description
                .contains("/Library/PrivilegedHelperTools/")
        );
        assert_eq!(local.program_for(&plan[0].operation), local.bundled_helper);
        assert_eq!(
            local.program_for(&plan[1].operation),
            Path::new("/Library/PrivilegedHelperTools/me.romainalcon.devspace.helper")
        );

        let plan = setup_plan(&status, &bundled(Installed, Outdated)).unwrap();
        assert_eq!(operations(&plan), ["link-cli"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_foreign_devspace_command_blocks_the_setup() {
        let status = sample_with_ca();
        let err = setup_plan(
            &status,
            &bundled(IntegrationState::Installed, IntegrationState::Conflict),
        )
        .unwrap_err();
        assert!(err.to_string().contains("/usr/local/bin/devspace"), "{err}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn teardown_removes_the_link_then_the_helper() {
        use IntegrationState::{Installed, Missing, Outdated};
        let mut status = sample_with_ca();
        status.system.resolver = IntegrationStatus::Missing;
        status.system.port_forwarding = IntegrationStatus::Missing;
        status.system.certificate_trust = IntegrationStatus::Missing;

        assert!(
            teardown_plan(&status, &bundled(Missing, Missing))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            operations(&teardown_plan(&status, &bundled(Installed, Missing)).unwrap()),
            ["remove-helper"]
        );
        assert_eq!(
            operations(&teardown_plan(&status, &bundled(Missing, Installed)).unwrap()),
            ["install-helper", "unlink-cli", "remove-helper"]
        );
        status.system.resolver = IntegrationStatus::Installed;
        assert_eq!(
            operations(&teardown_plan(&status, &bundled(Installed, Outdated)).unwrap()),
            ["remove-resolver", "unlink-cli", "remove-helper"]
        );
    }
}
