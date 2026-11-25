//! Spawning detached background processes.

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

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::process::CommandExt;
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
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

    pub(super) fn detach(command: &mut Command) {
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}
