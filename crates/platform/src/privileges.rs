//! Detection of elevated execution (root on Unix, administrator on Windows).
//!
//! DevSpace's main processes (daemon, CLI, desktop) must never run elevated.
//! Privileged operations will be isolated in a dedicated helper in a later
//! milestone.

use crate::PlatformError;

/// Returns `true` when the current process runs with elevated privileges.
///
/// - Unix: the effective user id is `0` (root).
/// - Windows: the process token is elevated (UAC "Run as administrator").
///
/// # Errors
///
/// Returns [`PlatformError::PrivilegeQuery`] if the OS cannot be queried
/// (Windows only; the Unix query cannot fail).
pub fn is_elevated() -> Result<bool, PlatformError> {
    imp::is_elevated()
}

#[cfg(unix)]
mod imp {
    use crate::PlatformError;

    #[allow(unsafe_code)]
    pub(super) fn is_elevated() -> Result<bool, PlatformError> {
        // SAFETY: `geteuid` takes no arguments, has no preconditions and
        // always succeeds (POSIX.1-2008).
        let euid = unsafe { libc::geteuid() };
        Ok(euid == 0)
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use crate::PlatformError;

    #[allow(unsafe_code)]
    pub(super) fn is_elevated() -> Result<bool, PlatformError> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
        // closing; `token` is a valid out-pointer.
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if opened == 0 {
            return Err(PlatformError::PrivilegeQuery {
                source: io::Error::last_os_error(),
            });
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let size = u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>()).unwrap_or(u32::MAX);
        // SAFETY: `token` was opened with TOKEN_QUERY above; the buffer points
        // to a properly sized and aligned TOKEN_ELEVATION.
        let queried = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                (&raw mut elevation).cast(),
                size,
                &mut returned,
            )
        };
        let query_error = io::Error::last_os_error();
        // SAFETY: `token` is a valid handle owned by this function.
        unsafe { CloseHandle(token) };

        if queried == 0 {
            return Err(PlatformError::PrivilegeQuery {
                source: query_error,
            });
        }
        Ok(elevation.TokenIsElevated != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privilege_query_succeeds() {
        // The result depends on how tests are launched; the query itself must
        // not fail on supported platforms.
        assert!(is_elevated().is_ok());
    }
}
