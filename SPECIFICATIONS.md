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

Requirements are tagged with the milestone delivering them. Only M0 items are
implemented today.

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

### Planned

| ID | Milestone | Requirement |
|---|---|---|
| F-1 | M1 | Daemon runs in the background, exposes a versioned IPC API over Unix domain sockets / Windows named pipes, restricted to the owning user. |
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
- IPC clients are authenticated with OS-level permissions and peer identity.
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
