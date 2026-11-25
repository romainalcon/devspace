# DevSpace — Roadmap

One milestone at a time. A milestone starts only once the previous one is
validated. Scope changes are recorded here.

Legend: ✅ done · 🔄 in progress · ⏳ planned

## M0 — Foundation ✅

- ✅ Cargo workspace cleaned up: shared `workspace.package`, centralised
  dependency versions, workspace lints, root-only release profile.
- ✅ Unused dependencies removed.
- ✅ Crate responsibilities defined (see `ARCHITECTURE.md`).
- ✅ Typed global configuration (`config.toml`, schema v1).
- ✅ Cross-platform application directories with `DEVSPACE_HOME` override.
- ✅ Structured errors (`thiserror`) with source-chain reporting.
- ✅ `tracing` logging with `DEVSPACE_LOG` override.
- ✅ Refusal to run elevated (Unix euid, Windows token elevation).
- ✅ Minimal CLI: `paths`, `config path|show|init`.
- ✅ Daemon startup sequence (no service loop yet).
- ✅ Unit tests for configuration, paths, logging filter, errors.
- ✅ GitHub Actions CI: macOS ARM64/x86_64, Linux x86_64/ARM64, Windows x86_64.
- ✅ `ARCHITECTURE.md`, `SPECIFICATIONS.md`, `ROADMAP.md`.
- ✅ Validated locally on macOS ARM64; M1 started on 2026-11-25.
- ⏳ First CI run on all platforms: the repository has no remote yet, so the
  workflow has never run.

## M1 — Daemon & IPC ✅

- ✅ Tokio runtime in the daemon, graceful shutdown on `SIGINT`/`SIGTERM`/
  `SIGHUP` (Unix) and console events (Windows).
- ✅ IPC protocol in `devspace-ipc`: versioned JSON messages, 4-byte
  length-prefixed framing (1 MiB max), handshake with version check,
  request ids, timeouts.
- ✅ Transport in `devspace-platform`: Unix domain socket (`0600`) in a
  private runtime directory (`0700`, ownership checked); Windows named pipe
  with a current-user-only DACL, first-instance flag and remote clients
  rejected; peer identity checked on both sides (uid / user SID).
- ✅ `devspace daemon start|stop|status`; `devspaced` runnable in the
  foreground.
- ✅ Single-instance lock (`devspaced.lock`, OS advisory lock released on
  crash); stale sockets replaced; daily rotated log files kept 7 days.
- ✅ Unit tests (framing, wire format, handshake, transport, lock, file
  logging) and end-to-end tests of the daemon binary.
- ✅ Validation on macOS ARM64: tests, manual lifecycle (start, status, stop,
  `kill -9` recovery, startup failure report, foreground Ctrl+C).
- ✅ `clippy -D warnings` cross-checked for Windows x86_64 and Linux x86_64
  (compilation only).
- ✅ Validated on macOS ARM64; M2 started on 2026-11-25.
- ⏳ Execution on Windows and Linux (CI, once a remote exists). The Windows
  named-pipe ACL and peer checks have only been compiled, never run.

## M2 — Project registry 🔄 (awaiting validation)

- ✅ SQLite storage (`rusqlite`, bundled SQLite) in `<data>/devspace.db`,
  opened only by the daemon after it takes the instance lock; WAL journal.
- ✅ Schema migrations tracked in `PRAGMA user_version`, one transaction per
  migration; databases from a newer DevSpace are refused, never modified.
- ✅ Project registry: unique DNS-label names, unique canonical paths,
  registration never touches the project directory.
- ✅ Per-project manifest `devspace.toml` (schema v1, optional, strict,
  64 KiB max): `project.name`.
- ✅ IPC protocol version 2: `project_add`, `project_list`,
  `project_remove`, typed `error` responses.
- ✅ `devspace project add [PATH] [--name]`, `project list`,
  `project remove <NAME>`.
- ✅ Unit tests (names, manifest, migrations, registry) and an end-to-end
  test of the registry through the daemon, including persistence across
  restarts.
- ✅ Manual run on macOS ARM64.
- ⚠️ Cross-target `clippy` from macOS now covers `platform` and `ipc` only:
  the bundled SQLite needs a C toolchain for the target, so `core`,
  `daemon` and `cli` are only checked on their native CI runners.
- ⏳ Execution on Windows and Linux (CI, once a remote exists).

## M3 — Web server, domains & privileged helper ⏳

- Caddy managed by the daemon, `*.test` domains, local CA.
- Minimal privileged helper for resolver/hosts and trust store changes,
  requiring explicit approval.

## M4 — Runtime package manager ⏳

- Package index, downloads, checksum and signature verification, safe
  extraction, multiple versions side by side, per-project selection.

## M5 — Services ⏳

- Supervised databases and caches, loopback-only by default.

## M6 — Desktop application ⏳

- Vue 3 + TypeScript + Tailwind CSS + shadcn-vue UI on the IPC API.

## M7 — Distribution ⏳

- Signed installers and updates for each platform.
