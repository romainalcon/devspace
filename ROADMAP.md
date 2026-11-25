# DevSpace — Roadmap

One milestone at a time. A milestone starts only once the previous one is
validated. Scope changes are recorded here.

Legend: ✅ done · 🔄 in progress · ⏳ planned

## M0 — Foundation 🔄 (awaiting validation)

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
- ⏳ Validation: first CI run green on all platforms (local validation done
  on macOS ARM64 only).

## M1 — Daemon & IPC ⏳

- Tokio runtime in the daemon, graceful shutdown on signals / console events.
- IPC protocol in `devspace-ipc`: versioned messages, framing, handshake.
- Transport: Unix domain socket in a user-only directory; Windows named pipe
  with a user-restricted ACL; peer credential checks.
- Daemon lifecycle from the CLI: `devspace daemon start|stop|status`.
- Single-instance lock; file logging with rotation in `log_dir`.

## M2 — Project registry ⏳

- SQLite storage in `data_dir` with migrations.
- `devspace project add|list|remove`, per-project manifest format.

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
