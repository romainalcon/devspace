//! User-restricted local IPC transport.
//!
//! - **Unix:** a Unix domain socket named [`SOCKET_FILE_NAME`] inside the
//!   private runtime directory (`0700`, owned by the user). The socket itself
//!   is `0600`.
//! - **Windows:** a named pipe `\\.\pipe\devspace-<hash>` whose DACL grants
//!   access to the current user only, created with
//!   `FILE_FLAG_FIRST_PIPE_INSTANCE` (to detect squatting) and rejecting
//!   remote clients.
//!
//! On top of OS access control, both sides check the identity of their peer:
//! the server rejects clients, and clients reject servers, that do not run as
//! the current user (Unix: peer credentials; Windows: user SID of the peer
//! process).
//!
//! All functions must be called within a Tokio runtime.

use std::fmt;
use std::path::Path;

use crate::PlatformError;

pub use imp::{ClientStream, ServerStream};

/// File name of the daemon socket inside the runtime directory (Unix).
pub const SOCKET_FILE_NAME: &str = "devspaced.sock";

/// Address of the daemon IPC endpoint for a given runtime directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    inner: imp::Address,
}

impl Endpoint {
    /// Computes the endpoint associated with `runtime_dir`
    /// (see [`crate::AppPaths::runtime_dir`]).
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::SocketPathTooLong`] on Unix when the socket
    /// path does not fit in `sun_path`.
    pub fn new(runtime_dir: &Path) -> Result<Self, PlatformError> {
        imp::Address::new(runtime_dir).map(|inner| Self { inner })
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

/// Server side of the endpoint, accepting peer-verified connections.
#[derive(Debug)]
pub struct Listener {
    inner: imp::Listener,
}

impl Listener {
    /// Starts listening on `endpoint`.
    ///
    /// The caller must guarantee that it is the only server for this
    /// endpoint (the daemon holds the single-instance lock): on Unix, a stale
    /// socket left by a crashed daemon is removed. The endpoint is removed
    /// when the listener is dropped.
    ///
    /// # Errors
    ///
    /// Fails if a non-socket file occupies the path, or if the endpoint
    /// cannot be created or secured.
    pub fn bind(endpoint: &Endpoint) -> Result<Self, PlatformError> {
        imp::Listener::bind(&endpoint.inner).map(|inner| Self { inner })
    }

    /// Waits for the next client and verifies that it runs as the current
    /// user.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Accept`] on I/O failure, and
    /// [`PlatformError::PeerIdentity`] / [`PlatformError::PeerRejected`]
    /// when the client cannot be identified or is another user; the listener
    /// stays usable in every case.
    pub async fn accept(&mut self) -> Result<ServerStream, PlatformError> {
        self.inner.accept().await
    }
}

/// Connects to `endpoint` and verifies that the server runs as the current
/// user.
///
/// # Errors
///
/// Returns [`PlatformError::Connect`] when the endpoint cannot be reached
/// (see [`PlatformError::is_endpoint_absent`]), and
/// [`PlatformError::PeerIdentity`] / [`PlatformError::PeerRejected`] when the
/// server cannot be identified or is another user.
pub async fn connect(endpoint: &Endpoint) -> Result<ClientStream, PlatformError> {
    imp::connect(&endpoint.inner).await
}

#[cfg(unix)]
mod imp {
    use std::fmt;
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use tokio::net::{UnixListener, UnixStream};

    use super::SOCKET_FILE_NAME;
    use crate::PlatformError;

    /// Stream accepted by the server.
    pub type ServerStream = UnixStream;
    /// Stream opened by a client.
    pub type ClientStream = UnixStream;

    /// Size of `sockaddr_un::sun_path`, including the terminating NUL.
    const SUN_PATH_CAPACITY: usize = if cfg!(any(target_os = "linux", target_os = "android")) {
        108
    } else {
        104
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Address {
        path: PathBuf,
    }

    impl Address {
        pub(super) fn new(runtime_dir: &Path) -> Result<Self, PlatformError> {
            let path = runtime_dir.join(SOCKET_FILE_NAME);
            let max = SUN_PATH_CAPACITY - 1;
            if path.as_os_str().as_bytes().len() > max {
                return Err(PlatformError::SocketPathTooLong { path, max });
            }
            Ok(Self { path })
        }
    }

    impl fmt::Display for Address {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.path.display().fmt(f)
        }
    }

    #[derive(Debug)]
    pub(super) struct Listener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub(super) fn bind(address: &Address) -> Result<Self, PlatformError> {
            let path = &address.path;
            remove_stale_socket(path)?;
            let inner = UnixListener::bind(path).map_err(|source| PlatformError::Bind {
                endpoint: path.display().to_string(),
                source,
            })?;
            let listener = Self {
                inner,
                path: path.clone(),
            };
            // The runtime directory is already private; restricting the
            // socket itself is defence in depth.
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
                PlatformError::SecureEndpoint {
                    path: path.clone(),
                    source,
                }
            })?;
            Ok(listener)
        }

        pub(super) async fn accept(&mut self) -> Result<ServerStream, PlatformError> {
            let (stream, _) = self
                .inner
                .accept()
                .await
                .map_err(|source| PlatformError::Accept { source })?;
            verify_peer(&stream)?;
            Ok(stream)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // Best effort: a leftover socket is detected as stale and removed
            // by the next daemon. Only the lock holder ever binds this path,
            // so this cannot remove another daemon's socket.
            let _ = fs::remove_file(&self.path);
        }
    }

    fn remove_stale_socket(path: &Path) -> Result<(), PlatformError> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => {
                fs::remove_file(path).map_err(|source| PlatformError::RemoveStaleEndpoint {
                    path: path.to_path_buf(),
                    source,
                })
            }
            Ok(_) => Err(PlatformError::EndpointOccupied {
                path: path.to_path_buf(),
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(PlatformError::RemoveStaleEndpoint {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub(super) async fn connect(address: &Address) -> Result<ClientStream, PlatformError> {
        let stream =
            UnixStream::connect(&address.path)
                .await
                .map_err(|source| PlatformError::Connect {
                    endpoint: address.to_string(),
                    source,
                })?;
        verify_peer(&stream)?;
        Ok(stream)
    }

    fn verify_peer(stream: &UnixStream) -> Result<(), PlatformError> {
        let credentials = stream
            .peer_cred()
            .map_err(|source| PlatformError::PeerIdentity { source })?;
        let expected = crate::privileges::effective_uid();
        if credentials.uid() != expected {
            return Err(PlatformError::PeerRejected {
                detail: format!(
                    "peer runs as uid {}, expected uid {expected}",
                    credentials.uid()
                ),
            });
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fmt;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_BUSY, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::System::Pipes::{
        GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use crate::PlatformError;

    /// Stream accepted by the server.
    pub type ServerStream = NamedPipeServer;
    /// Stream opened by a client.
    pub type ClientStream = NamedPipeClient;

    /// How long a client retries while every pipe instance is busy.
    const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
    const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Address {
        name: String,
    }

    impl Address {
        #[allow(clippy::unnecessary_wraps)]
        pub(super) fn new(runtime_dir: &Path) -> Result<Self, PlatformError> {
            // Pipe names are machine-global: derive a stable name from the
            // (per-user) runtime directory so that each user and each
            // DEVSPACE_HOME gets its own pipe. Paths are case-insensitive.
            let key = runtime_dir.to_string_lossy().to_lowercase();
            Ok(Self {
                name: format!(r"\\.\pipe\devspace-{:016x}", fnv1a64(key.as_bytes())),
            })
        }
    }

    impl fmt::Display for Address {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.name.fmt(f)
        }
    }

    /// 64-bit FNV-1a, stable across Rust versions (unlike `DefaultHasher`).
    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }

    #[derive(Debug)]
    pub(super) struct Listener {
        name: String,
        /// Instance waiting for the next client.
        next: NamedPipeServer,
    }

    impl Listener {
        pub(super) fn bind(address: &Address) -> Result<Self, PlatformError> {
            let next =
                create_instance(&address.name, true).map_err(|source| PlatformError::Bind {
                    endpoint: address.name.clone(),
                    source,
                })?;
            Ok(Self {
                name: address.name.clone(),
                next,
            })
        }

        pub(super) async fn accept(&mut self) -> Result<ServerStream, PlatformError> {
            self.next
                .connect()
                .await
                .map_err(|source| PlatformError::Accept { source })?;
            let fresh = create_instance(&self.name, false)
                .map_err(|source| PlatformError::Accept { source })?;
            let connected = std::mem::replace(&mut self.next, fresh);
            verify_peer(connected.as_raw_handle(), PeerSide::Client)?;
            Ok(connected)
        }
    }

    pub(super) async fn connect(address: &Address) -> Result<ClientStream, PlatformError> {
        let deadline = Instant::now() + BUSY_TIMEOUT;
        let client = loop {
            match ClientOptions::new().open(&address.name) {
                Ok(client) => break client,
                Err(err)
                    if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                        && Instant::now() < deadline =>
                {
                    tokio::time::sleep(BUSY_RETRY_INTERVAL).await;
                }
                Err(source) => {
                    return Err(PlatformError::Connect {
                        endpoint: address.name.clone(),
                        source,
                    });
                }
            }
        };
        verify_peer(client.as_raw_handle(), PeerSide::Server)?;
        Ok(client)
    }

    #[allow(unsafe_code)]
    fn create_instance(name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let descriptor = SecurityDescriptor::current_user_only()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES whose descriptor
        // stays alive until the end of this function; the system copies the
        // descriptor when the pipe instance is created.
        unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast()) }
    }

    /// Self-relative security descriptor allocated by the system.
    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl SecurityDescriptor {
        #[allow(unsafe_code)]
        fn current_user_only() -> io::Result<Self> {
            // SAFETY: `GetCurrentProcess` returns a pseudo-handle valid for
            // the lifetime of the process.
            let sid = process_user_sid(unsafe { GetCurrentProcess() })?;
            // Protected DACL (no inherited ACEs) with a single ACE granting
            // GENERIC_ALL to the current user.
            let sddl = to_wide(&format!("D:P(A;;GA;;;{sid})"));
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: `sddl` is NUL-terminated, `descriptor` is a valid
            // out-pointer and the size out-pointer is optional.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(descriptor))
        }
    }

    impl Drop for SecurityDescriptor {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: the descriptor was allocated with LocalAlloc by
            // ConvertStringSecurityDescriptorToSecurityDescriptorW and is
            // freed exactly once.
            unsafe { LocalFree(self.0) };
        }
    }

    #[derive(Clone, Copy)]
    enum PeerSide {
        Client,
        Server,
    }

    #[allow(unsafe_code)]
    fn verify_peer(pipe: RawHandle, side: PeerSide) -> Result<(), PlatformError> {
        let identity = |source| PlatformError::PeerIdentity { source };
        let mut pid = 0u32;
        // SAFETY: `pipe` is a valid named pipe handle and `pid` a valid
        // out-pointer.
        let ok = unsafe {
            match side {
                PeerSide::Client => GetNamedPipeClientProcessId(pipe, &mut pid),
                PeerSide::Server => GetNamedPipeServerProcessId(pipe, &mut pid),
            }
        };
        if ok == 0 {
            return Err(identity(io::Error::last_os_error()));
        }
        // SAFETY: plain FFI call; a null handle signals failure.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(identity(io::Error::last_os_error()));
        }
        // SAFETY: `process` is a freshly opened handle owned by this function.
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        let peer = process_user_sid(process.as_raw_handle()).map_err(identity)?;
        // SAFETY: pseudo-handle of the current process, always valid.
        let own = process_user_sid(unsafe { GetCurrentProcess() }).map_err(identity)?;
        if peer != own {
            return Err(PlatformError::PeerRejected {
                detail: format!("peer process {pid} runs as {peer}, expected {own}"),
            });
        }
        Ok(())
    }

    /// String form (`S-1-5-…`) of the user SID owning `process`.
    #[allow(unsafe_code)]
    fn process_user_sid(process: RawHandle) -> io::Result<String> {
        let mut raw_token = std::ptr::null_mut();
        // SAFETY: `process` is a valid process handle with query rights and
        // `raw_token` a valid out-pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw_token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw_token` is a freshly opened handle owned by this function.
        let token = unsafe { OwnedHandle::from_raw_handle(raw_token) };

        let mut len = 0u32;
        // SAFETY: size query with an empty buffer, as documented.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut len,
            )
        };
        let size_error = io::Error::last_os_error();
        if size_error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(size_error);
        }
        // `u64` elements give the buffer the alignment required by TOKEN_USER.
        let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buffer` holds at least `len` writable, suitably aligned bytes.
        let ok = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                len,
                &mut len,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call succeeded, so the buffer starts with an initialised
        // TOKEN_USER whose SID points inside `buffer`, alive below.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        sid_to_string(user.User.Sid)
    }

    #[allow(unsafe_code)]
    fn sid_to_string(sid: *mut c_void) -> io::Result<String> {
        let mut raw: *mut u16 = std::ptr::null_mut();
        // SAFETY: `sid` is a valid SID and `raw` a valid out-pointer.
        if unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0usize;
        // SAFETY: on success `raw` is a NUL-terminated UTF-16 string.
        while unsafe { *raw.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` code units before the terminator are initialised.
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, len) });
        // SAFETY: allocated with LocalAlloc by ConvertSidToStringSidW, freed once.
        unsafe { LocalFree(raw.cast()) };
        Ok(text)
    }

    fn to_wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fnv1a64_is_stable() {
            assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
            assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        }

        #[test]
        fn pipe_name_is_case_insensitive() {
            let lower = Address::new(Path::new(r"c:\users\me\devspace\run")).unwrap();
            let upper = Address::new(Path::new(r"C:\Users\Me\DevSpace\run")).unwrap();
            assert_eq!(lower, upper);
            assert!(lower.name.starts_with(r"\\.\pipe\devspace-"));
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn runtime_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        crate::AppPaths::from_root(tmp.path())
            .ensure_exists()
            .unwrap();
        tmp
    }

    #[tokio::test]
    async fn connect_and_exchange_bytes() {
        let tmp = runtime_dir();
        let endpoint = Endpoint::new(&tmp.path().join("run")).unwrap();
        let mut listener = Listener::bind(&endpoint).unwrap();

        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
            listener
        });

        let mut client = connect(&endpoint).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        drop(server.await.unwrap());
    }

    #[tokio::test]
    async fn absent_endpoint_is_reported_as_such() {
        let tmp = runtime_dir();
        let endpoint = Endpoint::new(&tmp.path().join("run")).unwrap();
        let err = connect(&endpoint).await.unwrap_err();
        assert!(err.is_endpoint_absent(), "{err:?}");
    }

    #[tokio::test]
    async fn endpoint_is_removed_on_drop() {
        let tmp = runtime_dir();
        let endpoint = Endpoint::new(&tmp.path().join("run")).unwrap();
        drop(Listener::bind(&endpoint).unwrap());
        let err = connect(&endpoint).await.unwrap_err();
        assert!(err.is_endpoint_absent(), "{err:?}");
        // The endpoint can be bound again.
        drop(Listener::bind(&endpoint).unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_is_replaced_and_socket_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = runtime_dir();
        let run = tmp.path().join("run");
        let socket = run.join(SOCKET_FILE_NAME);
        // Simulate a crashed daemon: a socket file nobody listens on.
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        assert!(socket.exists());
        let err = connect(&Endpoint::new(&run).unwrap()).await.unwrap_err();
        assert!(err.is_endpoint_absent(), "{err:?}");

        let _listener = Listener::bind(&Endpoint::new(&run).unwrap()).unwrap();
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        connect(&Endpoint::new(&run).unwrap()).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn regular_file_is_never_replaced() {
        let tmp = runtime_dir();
        let run = tmp.path().join("run");
        std::fs::write(run.join(SOCKET_FILE_NAME), "user data").unwrap();
        let err = Listener::bind(&Endpoint::new(&run).unwrap()).unwrap_err();
        assert!(matches!(err, PlatformError::EndpointOccupied { .. }));
        assert_eq!(
            std::fs::read_to_string(run.join(SOCKET_FILE_NAME)).unwrap(),
            "user data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn overlong_socket_path_is_rejected() {
        let long = std::path::PathBuf::from("/").join("d".repeat(120));
        let err = Endpoint::new(&long).unwrap_err();
        assert!(matches!(err, PlatformError::SocketPathTooLong { .. }));
    }
}
