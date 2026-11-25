# DevSpace — Architecture

This document describes the structure of the DevSpace codebase and the rules
that keep it maintainable. Any change to these rules must be documented here
(see [Decision log](#decision-log)).

## Overview

```
                ┌──────────────┐        ┌───────────────────┐
                │ devspace CLI │        │ Desktop (Tauri 2) │
                └──────┬───────┘        └─────────┬─────────┘
                       │      devspace-ipc        │
                       └────────────┬─────────────┘
                                    ▼
                         ┌─────────────────────┐
                         │  devspaced (daemon) │  single source of truth
                         └──────────┬──────────┘
                                    ▼
                         ┌─────────────────────┐
                         │    devspace-core    │  business logic
                         └──────────┬──────────┘
                                    ▼
                         ┌─────────────────────┐
                         │  devspace-platform  │  OS-specific code
                         └─────────────────────┘
```

Status in M1: the daemon serves the IPC protocol; the CLI uses it for the
daemon lifecycle (`devspace daemon start|stop|status`) and uses
`devspace-core` directly for local, daemon-independent commands
(configuration, paths). The desktop application is not connected yet (M6).

## Crates

| Crate | Path | Kind | Responsibility |
|---|---|---|---|
| `devspace-platform` | `crates/platform` | lib | **Only** place for platform-specific code: directory conventions, privilege detection, local sockets / named pipes and peer identity, shutdown signals, detached processes; later service registration, trust stores. Exposes portable APIs. |
| `devspace-core` | `crates/core` | lib | Business logic: typed configuration, errors, logging setup, single-instance lock; later projects, runtimes, services. No Tauri, no UI, no transport. |
| `devspace-ipc` | `crates/ipc` | lib | Protocol between daemon and clients: messages, framing, versioning, handshake, typed client and server connections. Re-exports the transport of `devspace-platform`. |
| `devspace-daemon` | `crates/daemon` | bin `devspaced` | Long-running process owning projects, services and processes. M1: IPC server and lifecycle. |
| `devspace-cli` | `crates/cli` | bin `devspace` | Command-line client. |
| `devspace-desktop` | `apps/desktop/src-tauri` | bin + lib | Tauri 2 shell with a Vue 3 / TypeScript frontend. Starter template in M0. |

### Allowed dependencies between crates

```
platform  ←  core  ←  daemon, cli
platform  ←  ipc   ←  daemon, cli, desktop
platform  ←  daemon, cli            (signals, detached processes)
```

- `core` and `platform` must never depend on `tauri`, `ipc`, or a binary crate.
- `ipc` depends only on `platform` (transport), never on `core`.
- `desktop` must never depend on `core`: all business operations go through
  the daemon via `ipc`.
- No crate other than `platform` may use `cfg(target_os = …)`, `cfg(unix)`,
  `cfg(windows)` or call OS APIs. Exception: tests may use `cfg` to exercise
  behaviour that only exists on some platforms (e.g. Unix signals).

## Cross-cutting concerns

### Configuration

- File: `<config_dir>/config.toml`, typed as `devspace_core::config::GlobalConfig`.
- Every field has a default; a missing file means "all defaults".
- Unknown keys are rejected (`deny_unknown_fields`) to surface typos.
- `schema_version` (currently `1`) is checked; future versions will be
  migrated explicitly rather than silently accepted.
- DevSpace never overwrites an existing configuration file
  (`GlobalConfig::write_new` uses `create_new`).

### Application directories

Resolved by `devspace_platform::AppPaths`:

| Kind | macOS | Linux | Windows |
|---|---|---|---|
| config | `~/Library/Application Support/DevSpace/config` | `$XDG_CONFIG_HOME/devspace` | `%APPDATA%\DevSpace\config` |
| data | `~/Library/Application Support/DevSpace/data` | `$XDG_DATA_HOME/devspace` | `%LOCALAPPDATA%\DevSpace\data` |
| cache | `~/Library/Caches/DevSpace` | `$XDG_CACHE_HOME/devspace` | `%LOCALAPPDATA%\DevSpace\cache` |
| logs | `~/Library/Logs/DevSpace` | `$XDG_STATE_HOME/devspace/logs` | `%LOCALAPPDATA%\DevSpace\logs` |
| run | `~/Library/Application Support/DevSpace/run` | `$XDG_RUNTIME_DIR/devspace` (fallback `<data>/run`) | `%LOCALAPPDATA%\DevSpace\run` |

- `DEVSPACE_HOME=<absolute path>` replaces the layout by
  `<root>/{config,data,cache,logs,run}`. Relative values are rejected.
- The runtime directory holds the daemon socket and lock. On Unix,
  `ensure_exists` fails if it is not owned by the current user or is
  accessible to group/others.
- On Windows, heavy data (runtimes, databases) is machine-local, not roaming.
- Directories are created on demand with mode `0700` on Unix.
- The layout is a pure function of `(OsFamily, BaseLocations)`, so all three
  layouts are unit-tested on every host.

### Errors

- Library crates define typed errors with `thiserror`
  (`PlatformError`, `devspace_core::Error`). Each variant carries context
  (path, value) and keeps the underlying error as `source`.
- Binaries print the full chain with `devspace_core::error::report` and exit
  with a non-zero code. No `anyhow` for now.
- `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` are denied by
  workspace lints outside tests.

### Logging

- `tracing` for instrumentation, `tracing-subscriber` (`fmt` + `env-filter`)
  for output, initialised by `devspace_core::logging::init` with a set of
  `LogTargets`.
- The CLI logs to **stderr** only, so that stdout stays machine-readable.
- The daemon logs to `<log_dir>/devspaced.<YYYY-MM-DD>.log` (daily rotation,
  7 files kept, no ANSI codes, `tracing-appender`), and also to stderr when it
  runs in the foreground. In background mode its stderr is redirected by the
  CLI to `<log_dir>/daemon-stderr.log` and only receives fatal errors, which
  `devspace daemon start` prints if the daemon exits during startup.
- Level comes from `logging.level`; `DEVSPACE_LOG` (EnvFilter syntax)
  overrides it. Formats: `compact` (one line, `key=value` fields) or `pretty`.
- Secrets must never be logged; fields are added explicitly, never by
  dumping whole structures that may contain credentials.

### Privileges

- `devspace_core::ensure_unprivileged()` is the first call of every entry
  point; it refuses to run when root (Unix, `euid == 0`) or elevated
  (Windows, token elevation).
- Operations that genuinely need privileges (binding ports < 1024, editing
  trust stores, `/etc/hosts` / resolver files) will be isolated in a separate,
  minimal helper with explicit user approval (milestone M3).

### IPC

**Transport** (`devspace_platform::local_socket`, re-exported by
`devspace-ipc`). One endpoint per runtime directory:

| | Unix | Windows |
|---|---|---|
| Endpoint | `<run>/devspaced.sock` (`0600`) in a `0700` directory | `\\.\pipe\devspace-<FNV-1a of the runtime dir>` |
| OS access control | directory and socket permissions | protected DACL: `GENERIC_ALL` for the current user SID only; remote clients rejected; `FILE_FLAG_FIRST_PIPE_INSTANCE` |
| Server checks client | `peer_cred().uid == geteuid()` | client process user SID == own user SID |
| Client checks server | same | server process user SID == own user SID |

A peer that cannot be identified is rejected (fail closed). Unix socket paths
longer than `sun_path` are rejected with an explicit error. On Unix the
daemon removes a stale socket at startup (only a socket, never another kind
of file) and its listener removes the socket when dropped.

**Framing** (`devspace_ipc::framing`): 4-byte big-endian length + JSON
payload, at most 1 MiB, checked before allocation.

**Protocol** (`devspace_ipc::protocol`): see `SPECIFICATIONS.md`. The
handshake messages never change shape; any other change increments
`PROTOCOL_VERSION`, and peers only talk when versions are equal.

**Timeouts**: connect 5 s, handshake 5 s (both sides), request 10 s.

### Daemon lifecycle

1. `ensure_unprivileged`, resolve and create directories, load
   configuration, initialise logging.
2. Take the single-instance lock `<run>/devspaced.lock`
   (`devspace_core::instance::InstanceLock`, `File::try_lock`). The OS
   releases it when the process dies, so a crash never blocks a restart.
3. Build the multi-threaded Tokio runtime, bind the endpoint, install
   signal handlers, accept connections (one task per connection).
4. On `Shutdown` request or signal (`SIGINT`, `SIGTERM`, `SIGHUP`; Windows
   Ctrl+C/Break, close, logoff, shutdown): drop the listener (endpoint
   removed), notify connections, wait up to 5 s, abort the rest, drop the
   runtime, release the lock.

`devspace daemon start` spawns `devspaced --background` from the directory of
the `devspace` executable, detached (`setsid` on Unix, `DETACHED_PROCESS` on
Windows), with the data directory as working directory, then waits up to
10 s for the handshake to succeed. `devspace daemon stop` sends `Shutdown`
and waits up to 10 s for the lock to be released.

### Unsafe code

`unsafe_code` is denied workspace-wide. It is allowed locally only in
`devspace-platform` for FFI calls (`geteuid`, `setsid`, Win32 token, pipe and
security descriptor APIs), each block having a `SAFETY:` comment.

## Tooling

- `rust-toolchain.toml`: stable channel with `rustfmt` and `clippy`.
- Workspace lints in the root `Cargo.toml`; `clippy.toml` allows
  `unwrap`/`expect` in tests.
- Shared dependency versions live in `[workspace.dependencies]`; a dependency
  is added only when a crate uses it.
- CI (`.github/workflows/ci.yml`): `fmt` on Linux, then `clippy -D warnings`
  and `test` on macOS ARM64, macOS x86_64, Linux x86_64, Linux ARM64 and
  Windows x86_64, plus a frontend type-check/build job.

## Decision log

| # | Date | Decision | Rationale |
|---|---|---|---|
| 1 | 2026-11-25 | Directory layout composed manually from `directories::BaseDirs` instead of `ProjectDirs`. | Control over macOS (`~/Library/Logs`), Windows (local vs roaming) and Linux (`XDG_STATE_HOME` for logs); testable for all OSes on any host. |
| 2 | 2026-11-25 | Configuration in TOML (`toml` crate), strict schema with `schema_version`. | Human-editable, typed, typos detected, explicit migrations. |
| 3 | 2026-11-25 | `thiserror` in libraries; no `anyhow` yet. | Typed errors at API boundaries; a small `report` helper covers the binaries. |
| 4 | 2026-11-25 | Logs to stderr only in M0; file logging deferred to M1 with the long-running daemon. | Avoids a dependency (`tracing-appender`) with no consumer yet. |
| 5 | 2026-11-25 | Removed unused dependencies (`reqwest`, `sha2`, `uuid`, `anyhow`, `tokio`). | No dependency without a consumer; they come back with the milestone that needs them. |
| 6 | 2026-11-25 | Binaries named `devspace` (CLI) and `devspaced` (daemon); desktop crate renamed `devspace-desktop`. | Clear, conventional names. |
| 7 | 2026-11-25 | Release profile defined at workspace root only; `panic = "abort"` dropped. | Member profiles are ignored by Cargo; unwinding lets the future async daemon survive a panicking task. |
| 8 | 2026-11-25 | Elevated execution is refused in every entry point. | Security requirement: no main process runs as root/administrator. |
| 9 | 2026-11-25 | IPC transport implemented in `devspace-platform` and re-exported by `devspace-ipc`; `ipc` depends on `platform`. | Sockets, pipes and peer credentials are OS-specific; keeps `cfg` out of `ipc` while clients only need `ipc`. |
| 10 | 2026-11-25 | Hand-written length-prefixed JSON framing instead of `tokio-util` codecs. | ~50 lines, no extra dependency, readable by the TypeScript side later; size limit enforced before allocation. |
| 11 | 2026-11-25 | Strict protocol version equality, with a handshake whose shape is frozen. | Simple and safe while CLI and daemon ship together; mismatches produce a clear "restart the daemon" error. |
| 12 | 2026-11-25 | Single-instance lock with `std::fs::File::try_lock` (Rust ≥ 1.89) in `core`. | Portable advisory lock without dependency or `cfg`; released by the OS on crash. |
| 13 | 2026-11-25 | Daily log rotation with `tracing-appender`, 7 files kept, blocking writer. | Low log volume; avoids the lossy non-blocking writer and its flush guard. |
| 14 | 2026-11-25 | Runtime directory in `$XDG_RUNTIME_DIR` on Linux, in Application Support on macOS (not `/tmp`). | Per-user, private locations; avoids shared temporary directories. |
| 15 | 2026-11-25 | `devspace daemon stop` waits for the instance lock rather than for the endpoint to disappear. | The lock is released last, so a `stop` followed by `start` never races with a daemon still draining connections. |
