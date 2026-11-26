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
   devspaced ──spawns──▶ PHP-FPM per PHP version (user, private socket)
   devspaced ──spawns──▶ PostgreSQL / MySQL / Valkey per series (user, loopback)
   devspace system setup ──sudo──▶ devspace-helper (one operation, exits;
                                   macOS: the administrator-owned copy)
```

Status in M7: the daemon serves the IPC protocol, owns the SQLite
database, runs the DNS server, supervises Caddy, the PHP-FPM masters and
the service servers, and installs runtimes; the CLI uses IPC for the
daemon lifecycle, the project registry (`devspace project …`), the web
server (`devspace web …`), the runtimes (`devspace runtime …`,
`devspace exec`) and the services (`devspace service …`), uses `devspace-core` directly for local,
daemon-independent commands (configuration, paths), and starts the
privileged helper through `sudo` for `devspace system setup|teardown`. The
desktop application (M6) talks to the daemon through `devspace-ipc` only:
its Tauri shell exposes a few commands to the Vue interface, starts the
daemon with the same launcher as the CLI (`devspace_ipc::launcher`), runs
the bundled CLI for `system setup|teardown`, and installs its own updates.
The bundles ship `devspaced`, `devspace` and `devspace-helper` next to the
application executable (see [Distribution](#distribution)).

## Crates

| Crate | Path | Kind | Responsibility |
|---|---|---|---|
| `devspace-platform` | `crates/platform` | lib | **Only** place for platform-specific code: directory conventions, single-instance lock of the daemon, build target, privilege detection, local sockets / named pipes and peer identity, shutdown signals, processes (detach, executable of a pid, stop), path canonicalisation and atomic writes, system integrations (resolver, port forwarding, trust store) and elevation; later service registration. Exposes portable APIs. |
| `devspace-core` | `crates/core` | lib | Business logic: typed configuration, errors, logging setup, SQLite storage and migrations, project registry and manifest, web server (pinned Caddy release, archive extraction, sites, Caddy configuration, local CA, DNS answers), archives and downloads, runtimes (signed package index, installation, selection, PHP-FPM configuration), services (plan of the instances, data directories, command lines). No Tauri, no UI, no transport. |
| `devspace-helper` | `crates/helper` | lib + bin `devspace-helper` | Privileged helper (bin) and what its callers share with it (lib): typed operations and their command-line encoding, PEM certificates and fingerprints, the local TLD. The lib has no OS code. |
| `devspace-ipc` | `crates/ipc` | lib | Protocol between daemon and clients: messages, framing, versioning, handshake, time limit of each request, typed client and server connections, and the client-side launcher (start, stop and status of the daemon) shared by the CLI and the desktop application. Re-exports the transport of `devspace-platform`. |
| `devspace-daemon` | `crates/daemon` | bin `devspaced` | Long-running process owning projects, services and processes. M5: IPC server, lifecycle, database owner, project requests, DNS server, supervision of Caddy, PHP-FPM and the service servers, runtime and service requests. |
| `devspace-cli` | `crates/cli` | bin `devspace` | Command-line client. |
| `devspace-registry` | `tools/registry` | bin `devspace-registry` | Maintainer tool: signing key generation, signing and verification of the package index. Never shipped. |
| `devspace-desktop` | `apps/desktop/src-tauri` | bin + lib | Tauri 2 shell (commands, menu bar icon, daemon autostart) with a Vue 3 / TypeScript / Tailwind CSS / shadcn-vue interface in `apps/desktop/src`. Depends on `ipc` and `platform` only. |

### Allowed dependencies between crates

```
platform  ←  core  ←  daemon, cli
platform  ←  ipc   ←  daemon, cli, desktop
platform  ←  daemon, cli, desktop   (signals, processes, system integrations,
                                     directories, privileges)
platform  ←  helper (bin)
helper (lib)  ←  core, cli
platform  ←  registry (tool)
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
- The helper next to the CLI (in the application bundle, or `target/` in
  development) is writable by the user. On macOS it runs elevated only for
  `install-helper`: it reads its own executable, checks that those bytes
  have the SHA-256 the user approved (computed by the CLI) and writes them
  to `/Library/PrivilegedHelperTools/me.romainalcon.devspace.helper`
  (`devspace_platform::system::helper_install`, a directory owned and only
  writable by root). Every other operation runs that copy, and the helper
  refuses them when it is not the administrator-owned installed copy. The
  plan reinstalls the copy when its content differs from the helper of
  the running CLI (e.g. after an update). The first `install-helper` still
  runs a user-writable program elevated, once, with the user's approval.
- `devspace_platform::system::cli_link` links `/usr/local/bin/devspace` to
  the CLI of an application bundle; a symbolic link cannot carry the
  marker, so a link is DevSpace's only when it points to
  `…/<name>.app/Contents/MacOS/devspace`.
- Elevation (`system::elevation::run_all`): `sudo` once per operation on
  the terminal, or (`--graphical`, used by the desktop application) one
  `osascript` `do shell script … with administrator privileges` running
  every operation in a POSIX script whose values are all single-quoted; the
  script and the prompt reach `osascript` through environment variables,
  so nothing is escaped as AppleScript.

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

**Timeouts**: connect 5 s, handshake 5 s (both sides); each request has its
own limit, `Request::time_limit` (10 s for the registry, 45 s for the web
server, 1 min for lists, index refreshes and removals, 10 min for service
start/stop, 15 to 45 min for downloads), so that every client waits the same time.

**Launcher** (`devspace_ipc::launcher`): `start`, `stop`, `status` and
`daemon_executable` (`devspaced` next to the running executable), used by
`devspace daemon …` and by the desktop application.

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
  path UNIQUE, added_at)`. Schema v2: `services(name, series, mode)`,
  primary key `(name, series)`, `mode` `started|stopped`.
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

### Runtimes

- `devspace_core::runtime::RuntimeLayout`: installations in
  `<data>/runtimes/<name>/<version>/`, staging in
  `<data>/runtimes/.staging/` (same filesystem, so renames are atomic),
  downloads in `<cache>/downloads`, PHP-FPM files in `<data>/php/<version>/`
  (configuration, `php.ini`), `<run>/php/<version>.{sock,pid}` and
  `<logs>/php-<version>-fpm.log`.
- Index (`runtime::index`): `registry/index.json`, its minisign signature
  and the public key `registry/devspace.pub` are embedded with
  `include_bytes!`; `Index::embedded` checks the signature (prehashed
  Ed25519 only) before parsing, then validates strictly (`deny_unknown_fields`,
  `https://` URLs, lowercase SHA-256, sizes ≤ 1 GiB, relative paths without
  `..`). The secret key never enters the repository; the
  `devspace-registry` tool signs with it after asking its password.
- Remote index (`runtime::remote::IndexStore`): `index.json` and
  `index.json.minisig` of the GitHub release `index`, downloaded by the
  daemon (8 MiB / 4 KiB at most) at startup + 5 s and every 6 h
  (`packages.remote_index`), and on `index_refresh`. Accepted only when the
  signature verifies with the embedded key and the serial is higher than
  the one in use (no rollback, even to a correctly signed older index);
  kept in `<data>/index/` (signature written last, so an interrupted write
  never verifies) and loaded at startup when newer than the embedded
  index. The daemon keeps the index behind a `RwLock`; installations read
  the current one.
- Installation (`runtime::install`): per archive, download of at most the
  declared size (`download::fetch`), SHA-256 check, `archive::extract_tree`
  into the staging directory; then each program's version is checked, the
  receipt `devspace-runtime.json` is written last and the directory is
  renamed into place. A directory without a valid receipt is never an
  installation. Removal renames to the staging area, then deletes.
  Installations and removals are serialised by the daemon.
- Extraction (`archive::tree`): names split on `/` only and refused when
  absolute, `..`, or holding `\`, `:` or NUL; regular files created with
  `create_new` (duplicates refused), executable bit only; symbolic links
  (tar only) created after every file, with relative targets lexically
  inside, then all resolved and required to stay inside; hard links,
  devices and FIFOs refused; 2 GiB / 100 000 entries.
- Selection (`runtime::select`): `[runtimes]` requirements of the manifest
  (`MAJOR`, `MAJOR.MINOR`, exact), newest installed match; newest installed
  version when absent. Used by the sites (PHP) and by `runtime_env`
  (`devspace exec`).
- PHP sites (`web::site`, `web::caddy_config`): a root with `index.php` and
  no `index.html` becomes a PHP site; its Caddy route is the JSON
  equivalent of `php_fastcgi` with a `unix/` upstream.
- PHP-FPM supervision (`devspaced::php`): before every (re)start of Caddy,
  `PhpPools::sync` runs exactly the masters of the versions used by the
  sites (foreground, own session, socket `0600` in the private runtime
  directory, `pm = ondemand`); a master that fails to start leaves its
  sites answering 502 and is reported by `web status`. Child supervision
  (monitor, graceful stop, pid file, orphan cleanup by executable check)
  is shared with Caddy in `devspaced::process`.
- `devspace exec` asks the daemon for the environment of the current
  directory and runs the command itself (with the user's terminal), the
  runtime directories first in `PATH` and `PHPRC` pointing to the
  DevSpace `php.ini`, and the variables of the database clients (`PGHOST`,
  `PGPORT`, `PGUSER`, `MYSQL_HOST`, `MYSQL_TCP_PORT`; never `DB_*`, which
  would override a project's `.env`). Nothing from the project is
  executed by DevSpace.
- Versions have three numbers, or four for rebuilds (`8.4.13.1`, the
  fourth never `0`; MySQL programs report `8.4.13`, so their check ignores
  it). Requirements stay `MAJOR[.MINOR[.PATCH]]`; an exact
  `PATCH` also accepts its rebuilds. Version checks compare the first
  line of the program's output, from its start or (MySQL, which prints its
  own path first) anywhere; PostgreSQL prints `MAJOR.MINOR`, its index
  versions have a patch number of `0`.
- Archives: `.tar.gz`, `.tar.xz` (liblzma, built from source and linked
  statically) and `.zip`. Hard links of tar archives are recreated as
  independent copies of a regular file the same extraction created
  before them; anything else (later entries, links, directories, paths
  outside) is refused.

### Services

- `devspace_core::service`: `ServiceName` (`mysql`, `postgresql`,
  `valkey`, also their package names), `Series` (PostgreSQL `MAJOR`, MySQL
  and Valkey `MAJOR.MINOR`), `InstanceKey` (service + series). An instance runs the
  newest installed version of its series.
- Plan (`service::plan`, pure): an instance runs when a registered project
  requires it (`[services]`, resolved to the newest installed version
  matching the requirement, hence to its series) or the user started it,
  unless the user stopped it, and only if a version is installed. All the
  series of a service share its port: only one runs, the one started by
  the user, otherwise the newest; the others report why. Unsatisfied
  requirements are listed with the command to run.
- Choices (`services` table): `service start` stores `started`, unless the
  projects already require the instance (then nothing is stored);
  `service stop` stores `stopped` if projects require it, otherwise
  removes the row. Starting a series clears `started` on the other series
  of the service.
- Layout (`ServiceLayout`): data in `<data>/services/<name>/<series>/`,
  initialised in `.init-<series>` next to it and renamed once the tool
  succeeded (a data directory that exists is initialised); sockets in
  `<run>/services/<name>-<series>/` (private directory), pid files
  `<run>/services/<name>-<series>.pid`, output in
  `<logs>/<name>-<series>.log`.
- Command lines (`service::process`): every setting on the command line.
  PostgreSQL: `initdb --auth=trust --username=postgres --encoding=UTF8
  --no-locale`, then `postgres -D … -h <address> -p <port> -k <sockets>
  -c hba_file=<sockets>/pg_hba.conf`, a file DevSpace rewrites at every
  start (socket and `127.0.0.0/8`, trust; `initdb` only trusts
  `127.0.0.1`).
  MySQL: `--no-defaults` first (no option file of the machine or the user
  is read), `--initialize-insecure` with an init file creating
  `root@'127.%'` (TCP clients come from a loopback address,
  `--skip-name-resolve`), then `mysqld` with `--bind-address`, `--port`,
  `--socket`, `--mysqlx=OFF`, `--secure-file-priv=NULL`.
  Valkey: options only (no configuration file is read), `--bind`,
  `--port`, `--dir <data>`, `--protected-mode no`, `--logfile ""`,
  `--unixsocket` with `--unixsocketperm 700`; no initialisation tool (an
  empty data directory). Built by DevSpace (`scripts/packages/`, workflow
  `packages.yml`): official source archive checked against the SHA-256 of
  valkey-io/valkey-hashes pinned in `valkey-sources.txt`, `make
  BUILD_TLS=no USE_SYSTEMD=no`, macOS 12 deployment target, Linux in the
  `manylinux_2_28` image (glibc 2.28); assets of the release
  `valkey-<version>`, never replaced once published.
- Supervision (`devspaced::services`): `Services::sync` stops the servers
  not in the plan or running another version, then starts the missing
  ones: port check on the sites address (unconfigured address → hint to
  run `system setup`; busy port → setting to change), initialisation on a
  blocking thread (5 min limit), start, readiness by a failing bind of the
  port (60 s). Stop: `SIGINT` (PostgreSQL fast shutdown) or `SIGTERM`
  (MySQL, Valkey), kill after 30 s; on Windows, which has no such signal,
  `pg_ctl stop -m fast -W` or `mysqladmin --no-defaults shutdown`
  (`Instance::stop_command`) instead. The plan is applied at startup and after every
  change of the projects, of the installed service versions and of the
  choices; a removed version is stopped first. Orphans are stopped at
  startup when their pid file designates a server program of an installed
  version of the series.

### Desktop application

- `apps/desktop/src-tauri` (Rust): `daemon::Daemon` holds the directories,
  endpoint and client identity; every operation opens its own connection
  (the daemon serves connections concurrently, so a download never delays
  the refresh). Commands: `daemon_status|start|stop`, `daemon_request` (any
  request of the protocol except `shutdown`, answered with the protocol
  response), `open_site` and `reveal_project` (URL and directory taken from
  the daemon, never from the interface). It refuses to run elevated, starts
  the daemon when the application starts, keeps a menu bar / notification
  area icon (state, start/stop, open, quit) refreshed every 3 s, and hides
  the window when it is closed. Quitting leaves the daemon running.
- `apps/desktop/src` (Vue 3, TypeScript, Tailwind CSS 4, shadcn-vue
  components in `src/components/ui`): `lib/protocol.ts` mirrors the
  protocol types by hand (keep in sync with `crates/ipc/src/protocol.rs`);
  `lib/store.ts` polls while the window is visible: daemon, projects and
  services every 2 s, web status (it runs system checks) and package index
  every 10 s, and everything after each action; `lib/actions.ts` runs one
  action per key with notifications. Pages: Overview (daemon, web server,
  system integrations, PHP-FPM), Projects, Runtimes, Services.
- Capabilities: `core:default` and `dialog:allow-open` (folder picker) only;
  strict CSP (`script-src 'self'`).
- System integrations (`src-tauri/src/system.rs`): the plan and the
  privileged changes stay in the CLI. The shell runs the bundled
  `devspace system setup|teardown --dry-run` to show the plan, then, once
  the user approved it, `--yes --graphical`; the CLI recomputes the plan
  and keeps its refusals.
- Updates (`src-tauri/src/update.rs`, `tauri-plugin-updater`, Rust side
  only, no webview permission): `latest.json` of the latest release,
  bundles signed with the updater key (minisign, public key in
  `tauri.conf.json`). Install: download and verify, stop the daemon, install,
  restart. At startup a daemon of another version is stopped and the
  bundled one started (`Daemon::start_current`).

### Daemon lifecycle

1. `ensure_unprivileged`, resolve and create directories, load
   configuration, initialise logging.
2. Take the single-instance lock `<run>/devspaced.lock`
   (`devspace_platform::instance::InstanceLock`, `File::try_lock`). The OS
   releases it when the process dies, so a crash never blocks a restart.
   Then open and migrate the database, stop a Caddy and PHP-FPM masters
   left by a crashed daemon, clean the runtime staging area and load the
   package index.
3. Build the multi-threaded Tokio runtime, bind the endpoint, install
   signal handlers, start the DNS server and (in the background, when
   installed) Caddy, accept connections (one task per connection).
4. On `Shutdown` request or signal (`SIGINT`, `SIGTERM`, `SIGHUP`; Windows
   Ctrl+C/Break, close, logoff, shutdown): drop the listener (endpoint
   removed), notify connections, wait up to 5 s, abort the rest, stop
   Caddy and the PHP-FPM masters, drop the runtime (which closes the
   database), release the lock.

`devspace daemon start` and the desktop application (`devspace_ipc::launcher::start`)
spawn `devspaced --background` from the directory of the running executable, detached (`setsid` on Unix, `DETACHED_PROCESS` on
Windows), with the data directory as working directory, then waits up to
10 s for the handshake to succeed. `devspace daemon stop` sends `Shutdown`
and waits up to 10 s for the lock to be released.

`InstanceLock::is_held` briefly takes the lock when it is free, so a probe
running at the moment another client checks the lock before spawning makes
that start report an unresponsive daemon. The desktop application
serialises its own probes, starts and stops; probes from two different
processes (e.g. `devspace daemon status` during a start from the
application) can still collide, which only makes that start fail with a
clear error.

### Unsafe code

`unsafe_code` is denied workspace-wide. It is allowed locally only in
`devspace-platform` for FFI calls (`geteuid`, `setsid`, `kill`,
`proc_pidpath`, Win32 token, process, pipe and security descriptor APIs),
each block having a `SAFETY:` comment.

## Distribution

- `apps/desktop/src-tauri/tauri.bundle.conf.json`, merged by `npm run
  bundle` (`tauri build --config …`), adds the sidecars (`externalBin`),
  the updater artifacts and the platform settings; the base configuration
  has none, so `cargo clippy --workspace` needs no prior build.
  `scripts/sidecars.mjs` (its `beforeBuildCommand`) builds `devspaced`,
  `devspace` and `devspace-helper` in release mode for the bundle target
  and copies them to `src-tauri/binaries/<name>-<target>`.
- macOS: `.app` and `.dmg`, ad hoc signature, macOS 12+. Windows: NSIS,
  per user, `PATH` of the user updated by installer hooks
  (`src-tauri/windows/installer-hooks.nsh`). Linux: `.deb`, `.rpm`
  (programs in `/usr/bin`) and AppImage (the only Linux bundle the updater
  replaces).
- `.github/workflows/release.yml`: on a tag `vX.Y.Z` equal to the
  workspace version, builds the five targets with `tauri-action` into a
  draft release with `latest.json`. Apple and Windows signing secrets are
  exported only when they exist (an empty variable would replace the ad
  hoc identity).
- `[profile.release.build-override] strip = false`: stripped procedural
  macro dylibs fail to load on macOS.

## Tooling

- `rust-toolchain.toml`: stable channel with `rustfmt` and `clippy`.
- Workspace lints in the root `Cargo.toml`; `clippy.toml` allows
  `unwrap`/`expect` in tests.
- Shared dependency versions live in `[workspace.dependencies]`; a dependency
  is added only when a crate uses it.
- CI (`.github/workflows/ci.yml`): `fmt` on Linux, then `clippy -D warnings`
  and `test` on macOS ARM64, macOS x86_64, Linux x86_64, Linux ARM64 and
  Windows x86_64, plus a frontend type-check/build job.
- `.github/workflows/packages.yml` (manual): DevSpace builds of packages
  without official relocatable builds (Valkey), attached to their release.
- `.github/workflows/release.yml`: desktop bundles (see
  [Distribution](#distribution)).

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
| 12 | 2026-11-25 | Single-instance lock with `std::fs::File::try_lock` (Rust ≥ 1.89) in `core` (moved to `platform` in M6, #49). | Portable advisory lock without dependency or `cfg`; released by the OS on crash. |
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
| 33 | 2026-11-26 | Runtime authenticity from a DevSpace index signed with the maintainer's Ed25519 key (minisign format, `minisign-verify` at run time, `minisign` in the tool), plus a SHA-256 per archive. | Approved by the user: one trust anchor for every source, including static-php-cli, which signs nothing; standard, small format verifiable with the reference tool. |
| 34 | 2026-11-26 | PHP from static-php-cli (macOS, Linux) and windows.php.net (Windows); Node.js from nodejs.org. | Approved by the user: relocatable binaries with the usual extensions, no build infrastructure needed yet. |
| 35 | 2026-11-26 | Index embedded in the build; no remote index yet. | No hosting exists; the signature still makes every index change require the maintainer key, and the format is ready for a remote index (M7). |
| 36 | 2026-11-26 | One PHP-FPM master per PHP version in use, on a Unix socket of the private runtime directory, restarted with the sites. | Several versions side by side without TCP ports reachable by other local users; Caddy runs as the same user. Not available on Windows (no PHP-FPM). |
| 37 | 2026-11-26 | A runtime the manifest does not mention resolves to the newest installed version. | PHP projects registered before M4 work without editing their manifest; `[runtimes]` pins versions for reproducibility. |
| 38 | 2026-11-26 | Child process supervision extracted to `devspaced::process`, shared by Caddy and PHP-FPM. | One implementation of monitor, graceful stop and crash recovery. |
| 39 | 2026-11-26 | M5 services: PostgreSQL and MySQL; Redis/Valkey postponed to M7. | Approved by the user: both publish relocatable binaries; Redis and Valkey publish none for macOS and Windows, so they need a DevSpace build and a hosting. |
| 40 | 2026-11-26 | Service programs are packages of the signed index (`postgresql`, `mysql`), installed by the runtime installer. | One verified installation path; `devspace service install` is the same request as `runtime install`. |
| 41 | 2026-11-26 | Instances shared by every project, one per service and series, started on demand. | Approved by the user (Herd's model); a series shares the on-disk format, so patch upgrades reuse the data, major upgrades get a new directory. |
| 42 | 2026-11-26 | Services on the sites address with their standard ports (`127.0.0.2:5432`, `:3306`). | Approved by the user: `.env` files keep the usual ports, and Herd's services on `127.0.0.1` are not disturbed; requires the loopback alias of `system setup` (a clear hint otherwise). |
| 43 | 2026-11-26 | Administrators without a password (`postgres` trust, MySQL `root` with an empty password). | Approved by the user (as Herd): development databases on the loopback only; any local user able to reach the loopback address can connect. |
| 44 | 2026-11-26 | PostgreSQL from theseus-rs/postgresql-binaries, MySQL from cdn.mysql.com. | No official relocatable PostgreSQL build exists for macOS and Linux; theseus-rs publishes SHA-256 and GitHub attestations for every target. Oracle publishes MD5 and GPG only: archives hashed at pin time (MD5 cross-checked), then fixed by the signed index. |
| 45 | 2026-11-26 | `liblzma` (static) for `.tar.xz`. | MySQL's Linux archives are xz only; the reference implementation, built like the bundled SQLite. |
| 46 | 2026-11-26 | Hard links of tar archives become copies of an already extracted file. | PostgreSQL's time zone data uses them; a copy never designates anything outside the extraction. |
| 47 | 2026-11-26 | M6 desktop: parity with the CLI except `system setup|teardown`, polling, menu bar icon, daemon started with the application. | Approved by the user; privileged changes keep their terminal approval flow, and polling needs no protocol change. |
| 48 | 2026-11-26 | Daemon launcher and request time limits moved to `devspace-ipc`. | The desktop must not depend on `core`, and the CLI and the desktop must start and wait for the daemon the same way. |
| 49 | 2026-11-26 | Single-instance lock moved from `core` to `platform`. | The launcher (in `ipc`, which depends on `platform` only) checks it. |
| 50 | 2026-11-26 | One generic `daemon_request` command rather than one Tauri command per request; protocol types mirrored by hand in TypeScript. | No business logic in the shell and no duplicated command list; a type generator would add a dependency to `ipc` for about forty types. |
| 51 | 2026-11-26 | Site URLs and project directories opened by the shell from the daemon's answers; no opener permission for the webview. | The interface cannot make the system open arbitrary URLs or paths. |
| 52 | 2026-11-26 | M7 hosted on GitHub Releases of the public repository; no signing certificates yet (ad hoc macOS signature, unsigned Windows), signing activated by secrets. | Approved by the user; the updater, the remote index and the packages need unauthenticated downloads. |
| 53 | 2026-11-26 | Administrator-owned copy of the helper installed by `system setup` (macOS), checked by SHA-256; other operations refused outside it. | Approved by the user: the bundle is writable by the user, so running its helper elevated repeatedly would let any program of the user gain root. Compatible with `.dmg` and the updater, unlike a root-owned `.pkg` installation. |
| 54 | 2026-11-26 | `/usr/local/bin/devspace` linked by the helper; a link is DevSpace's when it points to the CLI of an application bundle. | Approved by the user; the Linux packages and the Windows installer put the CLI in the `PATH` themselves. |
| 55 | 2026-11-26 | `system setup` from the application runs the bundled CLI (`--dry-run`, then `--yes --graphical`). | No duplicated plan logic; the desktop still never depends on `core`. |
| 56 | 2026-11-26 | Remote index: GitHub release `index`, same key as the embedded index, strictly increasing serial. | One trust anchor; the serial prevents serving an older signed index again. |
| 57 | 2026-11-26 | Valkey rather than Redis, built by DevSpace (8.1 and 9.1). | BSD-licensed, protocol-compatible; neither project publishes relocatable macOS builds. Sources pinned by the official hashes. |
| 58 | 2026-11-26 | MySQL Linux ARM64 from the full archive; archive limit raised to 1 GiB, downloads to 25 min. | Approved by the user; Oracle publishes no minimal ARM64 archive (≈ 790/930 MiB, 1.5–1.8 GiB extracted, under the 2 GiB extraction limit). MD5 cross-checked with Oracle's download page. |
| 59 | 2026-11-26 | Graceful stop on Windows with the servers' own tools. | Console-less processes cannot receive Ctrl+C; `pg_ctl` and `mysqladmin` are shipped with the servers. |
