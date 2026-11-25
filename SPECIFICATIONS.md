# DevSpace — Specifications

## Product

DevSpace is a cross-platform local development environment manager, a modern
alternative to Laravel Herd. It manages complete local environments (language
runtimes, web server, databases, services, local domains) with native
binaries and **without requiring Docker**.

### Goals

- Native runtimes (PHP, Node.js, …) installed per user, several versions side
  by side, selectable per project.
- Reproducible environments: a project declares what it needs; DevSpace
  installs and runs exactly that.
- Project-level orchestration: start/stop a project with its web server,
  services and processes.
- Local domains (`*.test`) served by Caddy with locally trusted HTTPS.
- Same feature set on macOS, Windows and Linux; macOS ARM64 is validated first.
- Two front-ends of equal standing, the CLI and the desktop application,
  both driving the same daemon.

### Non-goals

- Production hosting or deployment.
- Container orchestration (Docker may be supported as an optional service
  backend later, never required).
- Exposing local services to the network by default.

## Functional requirements

Requirements are tagged with the milestone delivering them. M0 and M1 items
are implemented today.

### Implemented (M0)

| ID | Requirement |
|---|---|
| F-0.1 | `devspace paths` prints the config, data, cache and logs directories. |
| F-0.2 | `devspace config path` prints the global configuration file path, even if the file is invalid. |
| F-0.3 | `devspace config show` prints the effective configuration (file merged with defaults) as TOML. |
| F-0.4 | `devspace config init` creates the configuration file with defaults and fails if it already exists. |
| F-0.5 | `devspaced` performs its startup checks (privileges, directories, configuration, logging), reports them and exits; it does not serve requests yet. |
| F-0.6 | `DEVSPACE_HOME` (absolute path) relocates every directory. |
| F-0.7 | `DEVSPACE_LOG` overrides the configured log filter. |

`devspaced` (F-0.5) now continues with the M1 behaviour below.

### Implemented (M1)

| ID | Requirement |
|---|---|
| F-1.1 | `devspace daemon start` starts `devspaced` in the background, detached from the terminal, and returns once it accepts connections (10 s limit). It succeeds without action if the daemon already runs. If the daemon exits during startup, its error and the log directory are printed. |
| F-1.2 | `devspace daemon stop` asks the daemon to shut down and returns once it has exited (10 s limit). It succeeds without action if the daemon is not running. |
| F-1.3 | `devspace daemon status` prints status, pid, version, uptime, endpoint and log directory. Exit code `0` when running, `3` when not running, `1` on error (including a process holding the lock without answering). |
| F-1.4 | Only one daemon runs per runtime directory; a second instance exits with "already running". A crashed daemon never prevents a restart. |
| F-1.5 | The daemon stops gracefully on `Shutdown` requests and on `SIGINT`/`SIGTERM`/`SIGHUP` (Unix) or console events (Windows): the endpoint disappears first, open connections get up to 5 s. |
| F-1.6 | The IPC endpoint is only usable by the user running the daemon (OS permissions plus peer identity checks on both sides). |
| F-1.7 | The daemon writes daily-rotated logs to the log directory and keeps 7 files; `devspaced` run directly also logs to stderr. |
| F-1.8 | `devspace paths` also prints the runtime directory. |

### Planned

| ID | Milestone | Requirement |
|---|---|---|
| F-2 | M2 | Project registry persisted in SQLite: add, list, remove projects. |
| F-3 | M3 | Caddy-based reverse proxy with `*.test` domains and a locally trusted CA, configured through an isolated privileged helper after explicit approval. |
| F-4 | M4 | Runtime package manager: download, verify checksums and signatures, extract safely (no path traversal), switch versions. |
| F-5 | M5 | Services (MySQL/MariaDB, PostgreSQL, Redis, …) supervised by the daemon. |
| F-6 | M6 | Desktop application on top of the IPC API. |

## Configuration reference (schema version 1)

```toml
schema_version = 1        # required to equal 1 if present

[logging]
level = "info"            # error | warn | info | debug | trace
format = "compact"        # compact | pretty
```

Rules: all keys optional, unknown keys rejected, file never overwritten by
DevSpace.

## IPC protocol reference (version 1)

Frames: 4-byte big-endian payload length, then a UTF-8 JSON object; payloads
over 1 MiB are rejected. Every message carries a `type` tag.

| Direction | `type` | Fields |
|---|---|---|
| client → daemon | `hello` | `protocol_version`, `client: {name, version}` — first message |
| daemon → client | `welcome` | `protocol_version`, `daemon: {version, pid}` |
| daemon → client | `rejected` | `protocol_version`, `message` — then the connection closes |
| client → daemon | `request` | `id` (u64, echoed), `request: {method, …}` |
| daemon → client | `response` | `id`, `response: {kind, …}` |

| `method` | Response `kind` |
|---|---|
| `ping` | `pong` |
| `status` | `status` with `version`, `pid`, `uptime_secs` |
| `shutdown` | `shutting_down`, then the daemon stops |

Rules: `hello` must come first and within 5 s; unknown or malformed messages
close the connection; the handshake messages never change shape and any
other change increments the protocol version; peers only talk when versions
are equal.

## Non-functional requirements

### Platforms

macOS ARM64 (primary), macOS x86_64, Windows x86_64, Linux x86_64 and ARM64.
No platform-specific command or path may be assumed outside
`devspace-platform`.

### Security

- No main process (daemon, CLI, desktop) runs as root/administrator; this is
  enforced at startup (M0).
- Privileged operations are isolated in a minimal helper and require explicit
  user approval.
- IPC clients are authenticated with OS-level permissions and peer identity
  (M1); the daemon is also authenticated by its clients.
- Downloaded packages are verified (checksum and signature) before use;
  archives are extracted without path traversal.
- Services listen on loopback only by default.
- Secrets and credentials are never logged.
- Untrusted project commands are never executed without explicit approval.
- Existing system configuration is never overwritten without approval.

### Quality

- Rust stable, edition 2024; `cargo fmt`, `cargo clippy -D warnings` and
  `cargo test` pass on every supported platform in CI.
- No `unwrap`/`expect` in production code; errors are typed and carry context.
- Structured logging through `tracing`.
- Public APIs are documented (`missing_docs` lint).
- No placeholder presented as a working feature.
