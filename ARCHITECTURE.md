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

   devspaced ──spawns──▶ Caddy (user, loopback only)
   devspace system setup ──sudo──▶ devspace-helper (one operation, exits)
```

Status in M3: the daemon serves the IPC protocol, owns the SQLite
database, runs the DNS server and supervises Caddy; the CLI uses IPC for
the daemon lifecycle, the project registry (`devspace project …`) and the
web server (`devspace web …`), uses `devspace-core` directly for local,
daemon-independent commands (configuration, paths), and starts the
privileged helper through `sudo` for `devspace system setup|teardown`. The
desktop application is not connected yet (M6).

## Crates

| Crate | Path | Kind | Responsibility |
|---|---|---|---|
| `devspace-platform` | `crates/platform` | lib | **Only** place for platform-specific code: directory conventions, build target, privilege detection, local sockets / named pipes and peer identity, shutdown signals, processes (detach, executable of a pid, stop), path canonicalisation and atomic writes, system integrations (resolver, port forwarding, trust store) and elevation; later service registration. Exposes portable APIs. |
| `devspace-core` | `crates/core` | lib | Business logic: typed configuration, errors, logging setup, single-instance lock, SQLite storage and migrations, project registry and manifest, web server (pinned Caddy release, archive extraction, sites, Caddy configuration, local CA, DNS answers); later runtimes, services. No Tauri, no UI, no transport. |
| `devspace-helper` | `crates/helper` | lib + bin `devspace-helper` | Privileged helper (bin) and what its callers share with it (lib): typed operations and their command-line encoding, PEM certificates and fingerprints, the local TLD. The lib has no OS code. |
| `devspace-ipc` | `crates/ipc` | lib | Protocol between daemon and clients: messages, framing, versioning, handshake, typed client and server connections. Re-exports the transport of `devspace-platform`. |
| `devspace-daemon` | `crates/daemon` | bin `devspaced` | Long-running process owning projects, services and processes. M3: IPC server, lifecycle, database owner, project requests, DNS server, Caddy supervision. |
| `devspace-cli` | `crates/cli` | bin `devspace` | Command-line client. |
| `devspace-desktop` | `apps/desktop/src-tauri` | bin + lib | Tauri 2 shell with a Vue 3 / TypeScript frontend. Starter template in M0. |

### Allowed dependencies between crates

```
platform  ←  core  ←  daemon, cli
platform  ←  ipc   ←  daemon, cli, desktop
platform  ←  daemon, cli            (signals, processes, system integrations)
platform  ←  helper (bin)
helper (lib)  ←  core, cli
```

- `core` and `platform` must never depend on `tauri`, `ipc`, or a binary crate.
- The helper binary depends only on `platform` and its own lib, so that
  the privileged code stays small and auditable; it never depends on
  `core` (no database, network or archive code runs privileged).
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
- Operations that genuinely need privileges are isolated in
  `devspace-helper`, the only program meant to run elevated (it refuses to
  run otherwise). It is one-shot: `devspace system setup|teardown` shows
  the plan, asks for confirmation, then runs `sudo devspace-helper
  <operation>` once per change. There is no privileged service.
- Operations are a closed, typed set (`devspace_helper::Operation`) with a
  strict command-line encoding; the only input on stdin is a PEM
  certificate whose SHA-256 must equal the fingerprint on the command line
  (the one the user approved). The helper copies it into a directory only
  root can access before use.
- Every system file written by DevSpace contains the marker "Managed by
  DevSpace"; files without it are never modified or removed, and the plan
  aborts before any change when one is in the way. System tools are run by
  absolute path with a cleared environment.
- Ports 80/443 are not bound by DevSpace: the web server listens on
  unprivileged ports of the sites address and the helper installs a port
  forwarding for that address only (macOS: `lo0` alias for addresses other
  than `127.0.0.1`, `pf` rdr rules in the anchor `com.apple/devspace`, both
  restored at boot by a launch daemon, without editing `/etc/pf.conf`).
- The TLD and address are validated types (`devspace_helper::Tld`,
  `LoopbackAddress`) shared by the configuration, the CLI and the helper;
  the platform layer validates them again before building paths or
  commands.
- In installed layouts the helper must be root-owned (M7); in development it
  is run from `target/`, which is acceptable only because the user
  approves every invocation with their own password.

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

### Storage

- One SQLite database, `<data>/devspace.db` (`AppPaths::database_file`),
  accessed through `devspace_core::db::Database` (`rusqlite` with the
  bundled SQLite, so every platform runs the same SQLite version).
- Only the daemon opens it, after taking the instance lock: there is a
  single writer and clients never read the file. WAL journal, foreign keys
  on, 5 s busy timeout.
- Schema version in `PRAGMA user_version`. `db::MIGRATIONS` is an
  append-only list of SQL scripts; each pending one runs in its own
  `IMMEDIATE` transaction together with the version bump. A database newer
  than the build is refused without modification (no downgrade).
- Tables use `STRICT` typing. Schema v1: `projects(id, name UNIQUE,
  path UNIQUE, added_at)`.
- The connection is synchronous: the daemon keeps it behind a
  `std::sync::Mutex` and uses it from `spawn_blocking` tasks, never from
  async worker threads.

### Projects

- `devspace_core::project`: `ProjectName` (validated DNS label), `Project`,
  and the registry operations on `Database` (`add_project`,
  `list_projects`, `remove_project`).
- Paths sent to the daemon must be absolute (the CLI resolves relative ones
  against its own working directory); the daemon canonicalises them with
  `devspace_platform::fs::canonicalize` (no `\\?\` prefix on Windows) and
  stores them as UTF-8 text.
- The optional manifest `devspace.toml` (`project::manifest`) is parsed
  with the same strict rules as the global configuration. It is read when
  a project is added, never written.
- Core errors are mapped to IPC `ErrorCode`s in the daemon
  (`handlers::error_response`); the message is the full error chain.

### Web server and local domains

- `devspace_core::web::WebLayout` places everything under the DevSpace
  directories: Caddy in `<data>/tools/caddy/<version>/`, its configuration
  and storage (CA, certificates) in `<data>/caddy/`, a contained
  `HOME`/XDG directory in `<data>/caddy/home`, its output in
  `<logs>/caddy.log`, its pid in `<run>/caddy.pid`, downloads in
  `<cache>/downloads`.
- Installation (`web::release`): one pinned version with SHA-256 checksums
  per platform embedded in the source; HTTPS-only download (`ureq`,
  rustls) with size and time limits; checksum verified before extraction;
  `web::archive` copies only the named entry of the archive root into a file
  chosen by DevSpace (no path from the archive is ever used); `caddy version`
  is checked; the executable appears atomically.
- Configuration (`web::caddy_config`): generated JSON, admin API disabled
  and never persisted, servers on `web.address` only (an IPv4 loopback
  address, `127.0.0.2` by default), one route per site (`<name>.<tld>`,
  `*.<name>.<tld>`), certificates from the internal CA
  with `install_trust: false`, HTTP redirects carrying an `X-DevSpace`
  header used to probe the forwarding.
- Sites (`web::site`): targets read from the manifest each time the
  configuration is generated; static roots canonicalised and confined to
  the project; proxies restricted to loopback addresses.
- Supervision (`devspaced::web`): Caddy runs in its own session; the daemon
  checks the ports are free, waits until Caddy holds the HTTPS port (15 s;
  checked by a failing bind, because with the `pf` redirection active,
  direct connections to the redirection target ports stall on macOS),
  and a monitor task records unexpected exits. Changes restart Caddy.
  `SIGTERM` then kill after 5 s. At startup, a pid file designating a
  process that still runs the DevSpace Caddy executable is stopped (daemon
  crash recovery).
- DNS (`web::dns` + `devspaced::dns`): UDP on `127.0.0.1:<dns_port>`,
  authoritative for `<tld>.` only, `A` answers with the sites address (no
  `AAAA`), `REFUSED` elsewhere, no recursion; malformed input never panics (exhaustive
  truncation tests).

### Daemon lifecycle

1. `ensure_unprivileged`, resolve and create directories, load
   configuration, initialise logging.
2. Take the single-instance lock `<run>/devspaced.lock`
   (`devspace_core::instance::InstanceLock`, `File::try_lock`). The OS
   releases it when the process dies, so a crash never blocks a restart.
   Then open and migrate the database, and stop a Caddy left by a crashed
   daemon.
3. Build the multi-threaded Tokio runtime, bind the endpoint, install
   signal handlers, start the DNS server and (in the background, when
   installed) Caddy, accept connections (one task per connection).
4. On `Shutdown` request or signal (`SIGINT`, `SIGTERM`, `SIGHUP`; Windows
   Ctrl+C/Break, close, logoff, shutdown): drop the listener (endpoint
   removed), notify connections, wait up to 5 s, abort the rest, stop
   Caddy, drop the runtime (which closes the database), release the lock.

`devspace daemon start` spawns `devspaced --background` from the directory of
the `devspace` executable, detached (`setsid` on Unix, `DETACHED_PROCESS` on
Windows), with the data directory as working directory, then waits up to
10 s for the handshake to succeed. `devspace daemon stop` sends `Shutdown`
and waits up to 10 s for the lock to be released.

### Unsafe code

`unsafe_code` is denied workspace-wide. It is allowed locally only in
`devspace-platform` for FFI calls (`geteuid`, `setsid`, `kill`,
`proc_pidpath`, Win32 token, process, pipe and security descriptor APIs),
each block having a `SAFETY:` comment.

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
| 16 | 2026-11-25 | `rusqlite` with bundled SQLite, synchronous, behind a mutex in `spawn_blocking`; no `sqlx`. | Single writer (the daemon) and tiny queries; no async driver or compile-time database needed; same SQLite everywhere, including Windows. |
| 17 | 2026-11-25 | Hand-written migrations in `PRAGMA user_version` rather than a migration crate. | A dozen lines, no dependency, one transaction per step; newer databases are refused. |
| 18 | 2026-11-25 | The daemon reads the project manifest, not the CLI. | The daemon is the source of truth; the desktop app will only send a path. |
| 19 | 2026-11-25 | Project names are DNS labels; directory names are normalised, explicit names are only validated. | Names become `<name>.test` domains in M3; deriving a name must not fail on common directory names, but an explicit choice is never altered silently. |
| 20 | 2026-11-25 | Project paths stored canonical and as UTF-8 text; non-UTF-8 paths refused. | One entry per directory whatever the path used; JSON (IPC) and SQLite text require Unicode. |
| 21 | 2026-11-25 | Caddy is a pinned download (v2.11.9) with embedded SHA-256 checksums, ahead of the M4 package manager. | Approved by the user; a hash reviewed at pin time is a stronger anchor than a signature fetched at run time. Cosign signatures were not verified at pin time (cosign unavailable); the checksums were cross-checked with the published SHA-512 file. |
| 22 | 2026-11-25 | `ureq` (sync, rustls) for downloads, called from `spawn_blocking`; `tar` + `flate2` and `zip` (inflate only) for archives. | Small dependencies, no async client needed in `core`; same model as the database. |
| 23 | 2026-11-25 | Caddy admin API disabled; configuration applied by restarting Caddy. | The admin API is an unauthenticated TCP endpoint reachable by every local user; a restart takes well under a second for a development server. |
| 24 | 2026-11-25 | Embedded DNS server + per-TLD resolver file instead of `/etc/hosts`. | Approved by the user: wildcard subdomains, one privileged change for all projects. |
| 25 | 2026-11-25 | One-shot helper through `sudo`, no privileged service. | Approved by the user: no permanent root process or IPC surface; every change is approved and authenticated. |
| 26 | 2026-11-25 | Ports 80/443 reach Caddy through `pf` forwarding; Caddy listens on 47080/47443. | macOS refuses unprivileged binds of low ports on loopback addresses (only the wildcard address, which would expose the sites); approved by the user over visible high ports or launchd sockets. |
| 27 | 2026-11-25 | The helper lib is shared by `core` and `cli`; the helper bin does not depend on `core`. | One definition of operations and certificate handling; minimal privileged dependency tree. |
| 28 | 2026-11-25 | Static sites serve `[web] root` (default `public/`), never the project root by default. | Avoids exposing `.env` and sources; approved by the user. |
| 29 | 2026-11-25 | Orphan Caddy detection by pid file + executable path check. | A crashed daemon leaves Caddy holding the ports; checking the executable avoids signalling a reused pid. |
| 30 | 2026-11-25 | Configurable `web.tld` and `web.address`, defaulting to `internal` and `127.0.0.2`. | Approved by the user: coexistence with Herd/Valet, which use `*.test` and the ports of `127.0.0.1`; `pf` cannot route by host name, so a distinct address is required. `.internal` is reserved by ICANN for private use; `local` and `localhost` are refused. |
| 31 | 2026-11-25 | Sites served over IPv4 only (no `AAAA`, no `[::1]` listener). | IPv6 loopback has a single address, which cannot be dedicated to DevSpace; clients fall back to the `A` record. |
| 32 | 2026-11-25 | Unserved sites keep a route and a certificate and answer 503 with the reason; PHP front controllers are not served statically. | A blank page or a TLS failure in the browser does not explain anything; serving `index.php` as a file would be wrong. |
