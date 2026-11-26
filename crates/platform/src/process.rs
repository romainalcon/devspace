//! Spawning, inspecting and stopping processes.

use std::io;
use std::path::PathBuf;
use std::process::Command;

/// Configures `command` so that the spawned process outlives its parent and
/// is not attached to the parent's terminal or console.
///
/// - Unix: the child starts a new session (`setsid`), so it has no
///   controlling terminal and does not receive the terminal's signals.
/// - Windows: the child is created without a console
///   (`DETACHED_PROCESS`) in a new process group.
///
/// Standard streams are not changed; callers should redirect them.
pub fn detach(command: &mut Command) {
    imp::detach(command);
}

/// Returns the absolute path of the executable running as process `pid`.
///
/// Used to make sure that a process id read from a file still designates
/// the expected program before signalling it.
///
/// # Errors
///
/// Fails if the process does not exist, belongs to another user, or if the
/// platform cannot report it ([`io::ErrorKind::Unsupported`]).
pub fn executable_path(pid: u32) -> io::Result<PathBuf> {
    imp::executable_path(checked_pid(pid)?)
}

/// How a process is asked to stop gracefully.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StopRequest {
    /// `SIGTERM` on Unix: the usual request (Caddy, PHP-FPM, MySQL).
    #[default]
    Terminate,
    /// `SIGINT` on Unix: PostgreSQL's "fast" shutdown, which disconnects
    /// clients instead of waiting for them (`SIGTERM` waits).
    Interrupt,
}

/// Asks process `pid` to stop with `SIGTERM` (see [`request_stop_with`]).
///
/// # Errors
///
/// Fails if the process does not exist or cannot be signalled.
pub fn request_stop(pid: u32) -> io::Result<()> {
    request_stop_with(pid, StopRequest::Terminate)
}

/// Asks process `pid` to stop.
///
/// - Unix: sends the signal of `request`, which lets the process shut down
///   gracefully.
/// - Windows: terminates the process immediately (there is no portable
///   graceful request for console-less processes).
///
/// # Errors
///
/// Fails if the process does not exist or cannot be signalled.
pub fn request_stop_with(pid: u32, request: StopRequest) -> io::Result<()> {
    imp::request_stop(checked_pid(pid)?, request)
}

/// Kills process `pid` immediately (`SIGKILL` on Unix, `TerminateProcess`
/// on Windows).
///
/// # Errors
///
/// Fails if the process does not exist or cannot be killed.
pub fn kill(pid: u32) -> io::Result<()> {
    imp::kill(checked_pid(pid)?)
}

/// Rejects the process ids that designate several processes or the
/// system: `0`, and values that do not fit a positive `pid_t`.
fn checked_pid(pid: u32) -> io::Result<u32> {
    if pid == 0 || i32::try_from(pid).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid process id {pid}"),
        ));
    }
    Ok(pid)
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;

    #[allow(unsafe_code)]
    pub(super) fn detach(command: &mut Command) {
        // SAFETY: the closure runs in the forked child before `exec`; it only
        // calls `setsid`, which is async-signal-safe, and builds an
        // `io::Error` from the OS error code without allocating.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    pub(super) fn executable_path(pid: u32) -> io::Result<PathBuf> {
        use std::os::unix::ffi::OsStringExt as _;

        let pid = libc::c_int::try_from(pid).map_err(io::Error::other)?;
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let size = u32::try_from(buffer.len()).map_err(io::Error::other)?;
        // SAFETY: `buffer` is a writable allocation of `size` bytes, as
        // required by `proc_pidpath`.
        let written = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), size) };
        if written <= 0 {
            return Err(io::Error::last_os_error());
        }
        buffer.truncate(usize::try_from(written).map_err(io::Error::other)?);
        Ok(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
    }

    #[cfg(target_os = "linux")]
    pub(super) fn executable_path(pid: u32) -> io::Result<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn executable_path(_pid: u32) -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process executable lookup is not supported on this platform",
        ))
    }

    pub(super) fn request_stop(pid: u32, request: super::StopRequest) -> io::Result<()> {
        let signal_number = match request {
            super::StopRequest::Terminate => libc::SIGTERM,
            super::StopRequest::Interrupt => libc::SIGINT,
        };
        signal(pid, signal_number)
    }

    pub(super) fn kill(pid: u32) -> io::Result<()> {
        signal(pid, libc::SIGKILL)
    }

    #[allow(unsafe_code)]
    fn signal(pid: u32, signal: libc::c_int) -> io::Result<()> {
        // `checked_pid` guarantees a strictly positive value: a single
        // process, never a process group.
        let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
        // SAFETY: `kill` has no memory-safety preconditions.
        if unsafe { libc::kill(pid, signal) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, OpenProcess, PROCESS_ACCESS_RIGHTS,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        QueryFullProcessImageNameW, TerminateProcess,
    };

    pub(super) fn detach(command: &mut Command) {
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    /// Process handle closed on drop.
    struct ProcessHandle(HANDLE);

    impl ProcessHandle {
        #[allow(unsafe_code)]
        fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> io::Result<Self> {
            // SAFETY: `OpenProcess` has no memory-safety preconditions; a
            // null handle signals failure.
            let handle = unsafe { OpenProcess(access, 0, pid) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(handle))
        }
    }

    impl Drop for ProcessHandle {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: the handle was returned by `OpenProcess` and is owned.
            unsafe { CloseHandle(self.0) };
        }
    }

    #[allow(unsafe_code)]
    pub(super) fn executable_path(pid: u32) -> io::Result<PathBuf> {
        let process = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut buffer = vec![0u16; 32 * 1024];
        let mut size = u32::try_from(buffer.len()).map_err(io::Error::other)?;
        // SAFETY: `buffer` holds `size` UTF-16 units and `size` is a valid
        // in/out pointer; the handle has the required access right.
        let ok = unsafe {
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                buffer.as_mut_ptr(),
                &mut size,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        buffer.truncate(usize::try_from(size).map_err(io::Error::other)?);
        Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
    }

    pub(super) fn request_stop(pid: u32, _request: super::StopRequest) -> io::Result<()> {
        kill(pid)
    }

    #[allow(unsafe_code)]
    pub(super) fn kill(pid: u32) -> io::Result<()> {
        let process = ProcessHandle::open(pid, PROCESS_TERMINATE)?;
        // SAFETY: the handle has the PROCESS_TERMINATE access right.
        if unsafe { TerminateProcess(process.0, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_pids_are_rejected() {
        for pid in [0, u32::MAX] {
            assert_eq!(
                request_stop(pid).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(kill(pid).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    #[test]
    fn executable_of_current_process_is_found() {
        let found = crate::fs::canonicalize(&executable_path(std::process::id()).unwrap()).unwrap();
        let expected = crate::fs::canonicalize(&std::env::current_exe().unwrap()).unwrap();
        assert_eq!(found, expected);
    }
}
