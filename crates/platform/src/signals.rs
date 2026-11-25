//! Signals and console events requesting a graceful shutdown.
//!
//! - Unix: `SIGINT`, `SIGTERM` and `SIGHUP`.
//! - Windows: Ctrl+C, Ctrl+Break, console close, logoff and shutdown events.

use crate::PlatformError;

/// Listener for the OS events that ask the process to stop.
#[derive(Debug)]
pub struct ShutdownSignals {
    inner: imp::Signals,
}

impl ShutdownSignals {
    /// Installs the signal handlers. Must be called within a Tokio runtime.
    ///
    /// Once installed, the default action of these signals (immediate
    /// termination) is replaced: the process must stop by itself after
    /// [`ShutdownSignals::recv`] returns.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Signals`] if a handler cannot be installed.
    pub fn install() -> Result<Self, PlatformError> {
        imp::Signals::install()
            .map(|inner| Self { inner })
            .map_err(|source| PlatformError::Signals { source })
    }

    /// Waits for the next shutdown request and returns the name of the
    /// signal or event that triggered it.
    pub async fn recv(&mut self) -> &'static str {
        self.inner.recv().await
    }
}

#[cfg(unix)]
mod imp {
    use std::io;

    use tokio::signal::unix::{Signal, SignalKind, signal};

    #[derive(Debug)]
    pub(super) struct Signals {
        interrupt: Signal,
        terminate: Signal,
        hangup: Signal,
    }

    impl Signals {
        pub(super) fn install() -> io::Result<Self> {
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
                hangup: signal(SignalKind::hangup())?,
            })
        }

        pub(super) async fn recv(&mut self) -> &'static str {
            tokio::select! {
                _ = self.interrupt.recv() => "SIGINT",
                _ = self.terminate.recv() => "SIGTERM",
                _ = self.hangup.recv() => "SIGHUP",
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    use tokio::signal::windows::{
        CtrlBreak, CtrlC, CtrlClose, CtrlLogoff, CtrlShutdown, ctrl_break, ctrl_c, ctrl_close,
        ctrl_logoff, ctrl_shutdown,
    };

    #[derive(Debug)]
    pub(super) struct Signals {
        ctrl_c: CtrlC,
        ctrl_break: CtrlBreak,
        close: CtrlClose,
        logoff: CtrlLogoff,
        shutdown: CtrlShutdown,
    }

    impl Signals {
        pub(super) fn install() -> io::Result<Self> {
            Ok(Self {
                ctrl_c: ctrl_c()?,
                ctrl_break: ctrl_break()?,
                close: ctrl_close()?,
                logoff: ctrl_logoff()?,
                shutdown: ctrl_shutdown()?,
            })
        }

        pub(super) async fn recv(&mut self) -> &'static str {
            tokio::select! {
                _ = self.ctrl_c.recv() => "CTRL_C",
                _ = self.ctrl_break.recv() => "CTRL_BREAK",
                _ = self.close.recv() => "CTRL_CLOSE",
                _ = self.logoff.recv() => "CTRL_LOGOFF",
                _ = self.shutdown.recv() => "CTRL_SHUTDOWN",
            }
        }
    }
}
