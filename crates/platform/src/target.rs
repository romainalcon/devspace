//! Identification of the platform the binary was compiled for.

use crate::paths::OsFamily;

/// CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit x86 (`x86_64`, `amd64`).
    X86_64,
    /// 64-bit ARM (`aarch64`, `arm64`).
    Aarch64,
}

/// Operating system family and architecture of a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Operating system family.
    pub os: OsFamily,
    /// CPU architecture.
    pub arch: Arch,
}

impl Target {
    /// Target of the running binary, or `None` on an architecture DevSpace
    /// does not support.
    #[must_use]
    pub const fn current() -> Option<Self> {
        let arch = if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            return None;
        };
        Some(Self {
            os: OsFamily::current(),
            arch,
        })
    }
}

/// File name of an executable called `base` on the current platform
/// (`base.exe` on Windows, `base` elsewhere).
#[must_use]
pub fn executable_name(base: &str) -> String {
    format!("{base}{}", std::env::consts::EXE_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_target_is_supported_on_ci_hosts() {
        assert!(Target::current().is_some());
    }

    #[test]
    fn executable_name_has_platform_suffix() {
        let name = executable_name("caddy");
        assert!(name.starts_with("caddy"));
        assert_eq!(name.ends_with(".exe"), cfg!(windows));
    }
}
