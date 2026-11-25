//! Loopback address of the sites, and forwarding of its standard web ports
//! to the unprivileged ports of the web server.
//!
//! Binding ports below 1024 on a loopback address needs privileges, and the
//! web server never runs privileged. Instead, connections to
//! `<address>:80`/`:443` are redirected to the web server ports. Using an
//! address other than `127.0.0.1` (by default `127.0.0.2`) leaves the
//! standard ports of `127.0.0.1` to other tools (e.g. Laravel Herd).
//!
//! macOS:
//! - only `127.0.0.1` is configured on `lo0`; other addresses get an alias
//!   (`ifconfig lo0 alias <address>`);
//! - `pf` redirection rules live in the anchor [`ANCHOR`], below the
//!   `com.apple/*` anchor point that the default `/etc/pf.conf` already
//!   evaluates, so `/etc/pf.conf` itself is never modified. The rules are
//!   stored in `/etc/pf.anchors/devspace`;
//! - a launch daemon restores the alias and reloads the rules at boot.
//!
//! Installing adds the alias, loads the rules and enables `pf`; removing
//! flushes the anchor and removes the alias (`pf` stays enabled, without
//! DevSpace rules). `127.0.0.1` itself is never removed.

use std::net::Ipv4Addr;
use std::path::Path;

use super::{IntegrationState, MANAGED_MARKER, command, managed};
use crate::PlatformError;

/// `pf` anchor holding the DevSpace rules.
pub const ANCHOR: &str = "com.apple/devspace";

/// File holding the rules of [`ANCHOR`].
const ANCHOR_FILE: &str = "/etc/pf.anchors/devspace";

/// Label of the launch daemon restoring the forwarding at boot.
const LAUNCH_DAEMON_LABEL: &str = "me.romainalcon.devspace.port-forwarding";

/// Property list of that launch daemon.
const LAUNCH_DAEMON_FILE: &str =
    "/Library/LaunchDaemons/me.romainalcon.devspace.port-forwarding.plist";

const PFCTL: &str = "/sbin/pfctl";
const LAUNCHCTL: &str = "/bin/launchctl";
const IFCONFIG: &str = "/sbin/ifconfig";
const SH: &str = "/bin/sh";
const LOOPBACK_INTERFACE: &str = "lo0";

/// Standard ports that are forwarded.
const HTTP_PORT: u16 = 80;
const HTTPS_PORT: u16 = 443;

/// Address and ports of the web server, target of the forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forwarding {
    /// Loopback address of the sites.
    pub address: Ipv4Addr,
    /// Target of port 80.
    pub http: u16,
    /// Target of port 443.
    pub https: u16,
}

impl Forwarding {
    /// Validates the target: a loopback address, unprivileged (at least
    /// 1024) and distinct ports.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidArgument`].
    pub fn new(address: Ipv4Addr, http: u16, https: u16) -> Result<Self, PlatformError> {
        check_address(address)?;
        if http < 1024 || https < 1024 || http == https {
            return Err(PlatformError::InvalidArgument {
                detail: format!(
                    "forwarded ports must be distinct and at least 1024, got {http} and {https}"
                ),
            });
        }
        Ok(Self {
            address,
            http,
            https,
        })
    }
}

/// Accepts the addresses of `127.0.0.0/8` except the network and broadcast
/// addresses; this also guarantees a plain dotted-quad in commands.
fn check_address(address: Ipv4Addr) -> Result<(), PlatformError> {
    let usable = address.octets()[0] == 127
        && address != Ipv4Addr::new(127, 0, 0, 0)
        && address != Ipv4Addr::new(127, 255, 255, 255);
    if usable {
        Ok(())
    } else {
        Err(PlatformError::InvalidArgument {
            detail: format!("`{address}` is not a usable loopback address"),
        })
    }
}

/// Whether `address` needs an alias on the loopback interface.
fn needs_alias(address: Ipv4Addr) -> bool {
    address != Ipv4Addr::LOCALHOST
}

/// `pf` rules redirecting the standard ports of the address.
#[must_use]
pub fn anchor_rules(target: Forwarding) -> String {
    let address = target.address;
    let mut rules = format!(
        "# {MANAGED_MARKER}. Removed by `devspace system teardown`.\n\
         # Loaded into the pf anchor {ANCHOR}.\n"
    );
    for (from, to) in [(HTTP_PORT, target.http), (HTTPS_PORT, target.https)] {
        rules.push_str(&format!(
            "rdr pass on {LOOPBACK_INTERFACE} inet proto tcp from any to {address} port {from} \
             -> {address} port {to}\n"
        ));
    }
    rules
}

/// Command run at boot: restore the alias if needed, then load the rules.
fn boot_arguments(target: Forwarding) -> Vec<String> {
    let load = [PFCTL, "-a", ANCHOR, "-f", ANCHOR_FILE, "-E"].map(str::to_owned);
    if needs_alias(target.address) {
        // The address is validated, so the script contains no
        // user-controlled text.
        let script = format!(
            "{IFCONFIG} {LOOPBACK_INTERFACE} alias {} up; exec {}",
            target.address,
            load.join(" ")
        );
        vec![SH.to_owned(), "-c".to_owned(), script]
    } else {
        load.to_vec()
    }
}

/// Launch daemon property list restoring the forwarding at boot.
#[must_use]
pub fn launch_daemon_plist(target: Forwarding) -> String {
    let arguments: String = boot_arguments(target)
        .iter()
        .map(|argument| format!("        <string>{argument}</string>\n"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- {MANAGED_MARKER}. Removed by `devspace system teardown`. -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_DAEMON_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{arguments}    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// Whether `address` is configured on the loopback interface. Does not need
/// privileges.
fn has_alias(address: Ipv4Addr) -> Result<bool, PlatformError> {
    let output = command::run(IFCONFIG, &[LOOPBACK_INTERFACE])?;
    let needle = format!("inet {address} ");
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.trim_start().starts_with(&needle)))
}

/// Whether the forwarding to `target` is installed: files with the
/// expected content and, when needed, the loopback alias. The rules loaded
/// in the kernel are not inspected. Does not need privileges.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS,
/// [`PlatformError::SystemFile`] if a file cannot be read, or
/// [`PlatformError::SystemCommand`] if the interface cannot be inspected.
pub fn state(target: Forwarding) -> Result<IntegrationState, PlatformError> {
    ensure_supported()?;
    let files = IntegrationState::combine(&[
        managed::state(Path::new(ANCHOR_FILE), &anchor_rules(target))?,
        managed::state(Path::new(LAUNCH_DAEMON_FILE), &launch_daemon_plist(target))?,
    ]);
    // Files present but alias missing (e.g. lost at reboot): installing
    // again restores it.
    if files == IntegrationState::Installed
        && needs_alias(target.address)
        && !has_alias(target.address)?
    {
        return Ok(IntegrationState::Outdated);
    }
    Ok(files)
}

/// Describes what [`install`] changes, for the approval prompt.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS.
pub fn describe_install(target: Forwarding) -> Result<String, PlatformError> {
    ensure_supported()?;
    let alias = if needs_alias(target.address) {
        format!(
            "add the address {} to the loopback interface {LOOPBACK_INTERFACE}, ",
            target.address
        )
    } else {
        String::new()
    };
    Ok(format!(
        "{alias}forward {address} ports {HTTP_PORT} and {HTTPS_PORT} to the web server ports {} \
         and {}: write `{ANCHOR_FILE}` and `{LAUNCH_DAEMON_FILE}` (restores this at boot), load \
         the pf anchor `{ANCHOR}` and enable pf; `/etc/pf.conf` is not modified and other \
         addresses are not affected",
        target.http,
        target.https,
        address = target.address,
    ))
}

/// Describes what [`remove`] changes, for the approval prompt.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS, or
/// [`PlatformError::InvalidArgument`] for a non-loopback address.
pub fn describe_remove(address: Ipv4Addr) -> Result<String, PlatformError> {
    ensure_supported()?;
    check_address(address)?;
    let alias = if needs_alias(address) {
        format!(", remove the address {address} from {LOOPBACK_INTERFACE}")
    } else {
        String::new()
    };
    Ok(format!(
        "stop forwarding ports {HTTP_PORT} and {HTTPS_PORT} of {address}: flush the pf anchor \
         `{ANCHOR}`, delete `{ANCHOR_FILE}` and `{LAUNCH_DAEMON_FILE}`{alias} (pf stays enabled)"
    ))
}

/// Installs the forwarding and applies it immediately. Requires
/// administrator rights.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS,
/// [`PlatformError::ForeignSystemFile`] if one of the files exists without
/// the DevSpace marker (nothing is changed then),
/// [`PlatformError::SystemFile`] or [`PlatformError::SystemCommand`].
pub fn install(target: Forwarding) -> Result<(), PlatformError> {
    ensure_supported()?;
    let anchor_file = Path::new(ANCHOR_FILE);
    let plist_file = Path::new(LAUNCH_DAEMON_FILE);
    // Check both files before changing anything.
    managed::ensure_not_foreign(anchor_file)?;
    managed::ensure_not_foreign(plist_file)?;
    managed::install(anchor_file, &anchor_rules(target))?;
    managed::install(plist_file, &launch_daemon_plist(target))?;
    if needs_alias(target.address) && !has_alias(target.address)? {
        let address = target.address.to_string();
        command::run(IFCONFIG, &[LOOPBACK_INTERFACE, "alias", &address, "up"])?;
    }
    // Applied directly rather than through launchd, to report errors.
    command::run(PFCTL, &["-a", ANCHOR, "-f", ANCHOR_FILE, "-E"])?;
    Ok(())
}

/// Flushes the rules, removes the files and the loopback alias of
/// `address`; returns `false` if nothing was installed. Requires
/// administrator rights.
///
/// # Errors
///
/// [`PlatformError::Unsupported`] outside macOS,
/// [`PlatformError::InvalidArgument`], [`PlatformError::ForeignSystemFile`],
/// [`PlatformError::SystemFile`] or [`PlatformError::SystemCommand`].
pub fn remove(address: Ipv4Addr) -> Result<bool, PlatformError> {
    ensure_supported()?;
    check_address(address)?;
    let anchor_file = Path::new(ANCHOR_FILE);
    let plist_file = Path::new(LAUNCH_DAEMON_FILE);
    managed::ensure_not_foreign(anchor_file)?;
    managed::ensure_not_foreign(plist_file)?;
    command::run(PFCTL, &["-a", ANCHOR, "-F", "all"])?;
    // The job only runs at boot and exits; it is usually not loaded.
    let target = format!("system/{LAUNCH_DAEMON_LABEL}");
    let _ = command::output(LAUNCHCTL, &["bootout", target.as_str()]);
    let removed_plist = managed::remove(plist_file)?;
    let removed_anchor = managed::remove(anchor_file)?;
    let mut removed_alias = false;
    if needs_alias(address) && has_alias(address)? {
        let address = address.to_string();
        command::run(IFCONFIG, &[LOOPBACK_INTERFACE, "-alias", &address])?;
        removed_alias = true;
    }
    Ok(removed_plist || removed_anchor || removed_alias)
}

fn ensure_supported() -> Result<(), PlatformError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(PlatformError::Unsupported {
            feature: "the loopback address and the forwarding of ports 80 and 443",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITES: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);

    fn target() -> Forwarding {
        Forwarding::new(SITES, 47080, 47443).unwrap()
    }

    #[test]
    fn target_is_validated() {
        assert!(Forwarding::new(Ipv4Addr::LOCALHOST, 47080, 47443).is_ok());
        for (address, http, https) in [
            (SITES, 80, 47443),
            (SITES, 47080, 443),
            (SITES, 47080, 47080),
            (Ipv4Addr::new(10, 0, 0, 1), 47080, 47443),
            (Ipv4Addr::new(127, 0, 0, 0), 47080, 47443),
            (Ipv4Addr::new(127, 255, 255, 255), 47080, 47443),
        ] {
            assert!(
                Forwarding::new(address, http, https).is_err(),
                "{address} {http} {https}"
            );
        }
    }

    #[test]
    fn rules_only_redirect_the_sites_address() {
        let rules = anchor_rules(target());
        assert!(rules.contains(MANAGED_MARKER));
        let rdr: Vec<_> = rules.lines().filter(|l| l.starts_with("rdr ")).collect();
        assert_eq!(
            rdr,
            [
                "rdr pass on lo0 inet proto tcp from any to 127.0.0.2 port 80 -> 127.0.0.2 port 47080",
                "rdr pass on lo0 inet proto tcp from any to 127.0.0.2 port 443 -> 127.0.0.2 port 47443",
            ]
        );
    }

    #[test]
    fn boot_job_restores_the_alias_when_needed() {
        let plist = launch_daemon_plist(target());
        assert!(plist.starts_with("<?xml"));
        assert!(plist.contains(MANAGED_MARKER));
        assert!(plist.contains(&format!("<string>{LAUNCH_DAEMON_LABEL}</string>")));
        assert!(plist.contains(
            "<string>/sbin/ifconfig lo0 alias 127.0.0.2 up; exec /sbin/pfctl -a \
             com.apple/devspace -f /etc/pf.anchors/devspace -E</string>"
        ));

        let primary = Forwarding::new(Ipv4Addr::LOCALHOST, 47080, 47443).unwrap();
        let plist = launch_daemon_plist(primary);
        assert!(!plist.contains("ifconfig"));
        assert!(plist.contains("<string>/sbin/pfctl</string>"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn primary_loopback_address_is_detected() {
        assert!(has_alias(Ipv4Addr::LOCALHOST).unwrap());
        assert!(!has_alias(Ipv4Addr::new(127, 254, 253, 252)).unwrap());
    }
}
