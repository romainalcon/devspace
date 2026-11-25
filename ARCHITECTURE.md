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

Status in M0: the IPC arrows do not exist yet. The CLI uses `devspace-core`
directly for local, daemon-independent commands (configuration, paths).

## Crates

| Crate | Path | Kind | Responsibility |
|---|---|---|---|
| `devspace-platform` | `crates/platform` | lib | **Only** place for platform-specific code: directory conventions, privilege detection, later sockets/pipes, service registration, trust stores. Exposes portable APIs. |
| `devspace-core` | `crates/core` | lib | Business logic: typed configuration, errors, logging setup, later projects, runtimes, services. No Tauri, no UI, no transport. |
| `devspace-ipc` | `crates/ipc` | lib | Protocol between daemon and clients (messages, framing, versioning, transport). Empty in M0. |
| `devspace-daemon` | `crates/daemon` | bin `devspaced` | Long-running process owning projects, services and processes. M0: startup sequence only. |
| `devspace-cli` | `crates/cli` | bin `devspace` | Command-line client. |
| `devspace-desktop` | `apps/desktop/src-tauri` | bin + lib | Tauri 2 shell with a Vue 3 / TypeScript frontend. Starter template in M0. |

### Allowed dependencies between crates

```
platform  ←  core  ←  daemon
             core  ←  cli        (local commands only)
ipc       ←  daemon, cli, desktop  (from M1)
```

- `core` and `platform` must never depend on `tauri`, `ipc`, or a binary crate.
- `desktop` must never depend on `core`: all business operations go through
  the daemon via `ipc`.
- No crate other than `platform` may use `cfg(target_os = …)` or call OS APIs.

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

- `DEVSPACE_HOME=<absolute path>` replaces the layout by
  `<root>/{config,data,cache,logs}`. Relative values are rejected.
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
  for output, initialised by `devspace_core::logging::init`.
- Output goes to **stderr** so that stdout stays machine-readable.
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

### Unsafe code

`unsafe_code` is denied workspace-wide. It is allowed locally only in
`devspace-platform` for FFI calls, each block having a `SAFETY:` comment.

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
