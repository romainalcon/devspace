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
- Local domains (`*.internal` by default, configurable) served by Caddy with
  locally trusted HTTPS, coexisting with other local tools.
- Same feature set on macOS, Windows and Linux; macOS ARM64 is validated first.
- Two front-ends of equal standing, the CLI and the desktop application,
  both driving the same daemon.

### Non-goals

- Production hosting or deployment.
- Container orchestration (Docker may be supported as an optional service
  backend later, never required).
- Exposing local services to the network by default.

## Functional requirements

Requirements are tagged with the milestone delivering them. M0 to M4
items are implemented and validated, as are M5 items. M6 items are
implemented and awaiting validation by the user.

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

### Implemented (M2)

| ID | Requirement |
|---|---|
| F-2.1 | The daemon stores its state in an SQLite database, `<data>/devspace.db`, created and migrated at startup. A database created by a newer DevSpace makes the daemon refuse to start and is left untouched. |
| F-2.2 | `devspace project add [PATH] [--name NAME]` registers a directory (default: the current directory). The name is `--name`, else `project.name` from the project's `devspace.toml`, else the directory name normalised (lowercased, other characters turned into `-`). |
| F-2.3 | A directory is registered once, under its canonical path (symbolic links resolved); names are unique. Missing paths, non-directories, invalid names or manifests and duplicates are refused with an explanation. |
| F-2.4 | `devspace project list` prints the registered projects sorted by name. |
| F-2.5 | `devspace project remove NAME` unregisters a project. |
| F-2.6 | Registering or removing a project never creates, modifies or deletes files in the project directory. |
| F-2.7 | `project` commands require the daemon; when it is not running they fail with a hint to run `devspace daemon start`. |

### Implemented (M3)

| ID | Requirement |
|---|---|
| F-3.1 | `devspace web install` downloads Caddy v2.11.9 for the current platform over HTTPS, verifies its SHA-256 against the checksum embedded in DevSpace, extracts only the executable and installs it in `<data>/tools/caddy/2.11.9/`. A mismatching download is discarded. |
| F-3.2 | The daemon runs Caddy as the current user, listening only on `web.address` (IPv4 loopback, `127.0.0.2` by default) on `web.http_port` and `web.https_port`. It starts with the daemon when installed, restarts when a project is added or removed, and stops with the daemon. `devspace web start|stop|restart` control it; `restart` re-reads the project manifests. |
| F-3.3 | Every registered project is served at `https://<name>.<tld>` and `https://*.<name>.<tld>` (`web.tld`, `internal` by default): static files of `[web] root` (default `public/`, which must exist), or the loopback development server of `[web] proxy`. Roots resolving outside the project are refused; roots holding an `index.php` without `index.html` are PHP sites (F-4.7). Unserved projects are reported with the reason by `web status` and answer HTTP 503 with that reason; error responses carry a short text instead of an empty page. HTTP requests are redirected to HTTPS on the standard port. |
| F-3.4 | Certificates are issued by a local certificate authority created in `<data>/caddy/storage`; Caddy never installs its trust itself. |
| F-3.5 | The daemon answers DNS queries for `*.<tld>` on `127.0.0.1:web.dns_port` (UDP) with `web.address` (no IPv6 answer), and refuses other names. |
| F-3.6 | `devspace web status` prints the web server, DNS server, CA fingerprint, system integrations (and whether port 80 really reaches the web server) and every site. |
| F-3.7 | `devspace system setup` lists the missing integrations (resolver for `*.<tld>`, the sites address on the loopback interface with the forwarding of its ports 80/443, trust of the local CA with its fingerprint), asks for confirmation (`--yes` to skip), then runs `devspace-helper` through `sudo` for each one. If the CA does not exist yet, the web server is started after the first changes and the trust is approved separately. Starting the web server before its address exists fails with a hint to run `system setup`. `devspace system teardown` removes what DevSpace installed. Implemented on macOS. |
| F-3.8 | System files that exist without the DevSpace marker are never modified or removed; `system setup` then stops before any change and explains the conflict. |
| F-3.9 | Web and system commands require the daemon (F-2.7). Starting the web server before installing it fails with a hint to run `devspace web install`. |

### Implemented (M4)

| ID | Requirement |
|---|---|
| F-4.1 | The package index embedded in DevSpace is used only if its signature matches the embedded DevSpace public key; it lists, per runtime version and platform, HTTPS archive URLs with their size and SHA-256. |
| F-4.2 | `devspace runtime list` prints the index serial, the platform, and every published or installed version of PHP and Node.js with its state. |
| F-4.3 | `devspace runtime install NAME [VERSION]` installs the newest published version matching `VERSION` (`MAJOR`, `MAJOR.MINOR` or exact, including a rebuild such as `9.7.4.1`; newest overall if absent) in `<data>/runtimes/NAME/VERSION/`. Each archive is downloaded over HTTPS (at most its declared size), verified against its SHA-256, extracted without leaving the installation directory, and the programs must report the expected version; otherwise nothing is installed. Installing an installed version does nothing. |
| F-4.4 | `devspace runtime remove NAME VERSION` removes an installed version; its PHP-FPM is stopped first and the sites are reloaded. |
| F-4.5 | Several versions of a runtime can be installed side by side; installing or removing one never modifies another. |
| F-4.6 | `[runtimes] php` and `node` in `devspace.toml` select the newest installed version matching the requirement; without a requirement, the newest installed version is used. |
| F-4.7 | A site whose root has an `index.php` and no `index.html` is served by PHP-FPM of the selected PHP version (one master per version in use, running as the user on a private socket), with requests for missing files going to `index.php`. If no installed version matches, the site answers 503 with the command to run. `web status` lists the PHP version of each PHP site and the state of each PHP-FPM. Not available on Windows (reported as not served). |
| F-4.8 | `devspace exec -- COMMAND [ARGS…]` runs a command with the runtime versions of the registered project containing the current directory (newest installed versions elsewhere) first in `PATH`, the DevSpace `php.ini` for PHP, and returns its exit code; unmet requirements are reported as warnings. |
| F-4.9 | Runtime commands require the daemon (F-2.7). |

### Implemented (M5)

| ID | Requirement |
|---|---|
| F-5.1 | The package index also publishes the programs of the services: `postgresql` (17, 18, 19) and `mysql` (8.4, 9.7 LTS; not for macOS x86_64 and Linux ARM64). `devspace service install NAME [VERSION]` installs them like `runtime install` (F-4.3); versions may have a fourth number (`8.4.13.1`). |
| F-5.2 | An instance is a service and a series (PostgreSQL `MAJOR`, MySQL `MAJOR.MINOR`); it runs the newest installed version of its series and keeps its data in `<data>/services/<name>/<series>/`, shared by every project. |
| F-5.3 | `[services]` in `devspace.toml` (`postgresql = "18"`, `mysql = "8.4"`) makes the daemon run the instance of the newest installed matching version while the project is registered; unmet requirements are listed by `service list` with the command to run. |
| F-5.4 | `devspace service start NAME [SERIES]` runs an instance even if no project requires it; `devspace service stop NAME [SERIES]` stops it even if projects require it. Both are remembered across daemon restarts; starting again returns to the projects' choice. Without `SERIES`, `start` uses the newest installed series and `stop` every series. |
| F-5.5 | Only one series of a service runs at a time (they share its port): the one started by the user, otherwise the newest; the others report why. |
| F-5.6 | The first start creates the data directory atomically; failures are reported with the end of the log `<logs>/<name>-<series>.log` and leave no data directory. |
| F-5.7 | Servers run as the current user and listen only on `web.address` (`127.0.0.2` by default) on `services.postgresql_port` (5432) and `services.mysql_port` (3306), and on Unix sockets in the private runtime directory. The administrators (`postgres`, MySQL `root`) have no password. An unconfigured address or a busy port is reported with what to do. |
| F-5.8 | `devspace service list` prints every instance: version, state, port, user, why it runs or not (projects, choice of the user, conflict), failures. |
| F-5.9 | `devspace exec` (F-4.8) also puts the client programs of the services first in `PATH` and sets `PGHOST`, `PGPORT`, `PGUSER`, `MYSQL_HOST` and `MYSQL_TCP_PORT`. |
| F-5.10 | Removing a service version stops its instance first; the daemon stops every server when it stops, and stops those left by a crashed daemon at startup. |
| F-5.11 | Service commands require the daemon (F-2.7). |

### Implemented (M6)

| ID | Requirement |
|---|---|
| F-6.1 | The desktop application (`devspace-desktop`) talks to the daemon through the IPC protocol only. It refuses to run elevated. |
| F-6.2 | When it starts, it starts the daemon (`devspaced` next to its executable) unless it runs, like `devspace daemon start` (F-1.1); a failure is shown with the daemon's error output. Quitting the application leaves the daemon running. |
| F-6.3 | An icon in the menu bar (notification area) shows the daemon state and offers Open DevSpace, Start/Stop daemon and Quit. Closing the window hides it; on macOS a click on the Dock icon shows it again. |
| F-6.4 | Overview: daemon (version, pid, uptime, stop), web server (Caddy installation, state, start/stop/restart, address and ports, DNS server, CA fingerprint, failures), state of the three system integrations with the `devspace system setup` command to copy when one is missing or outdated, PHP-FPM masters. |
| F-6.5 | Projects: registered projects with their domain and what they serve (or why not); add a directory chosen with the system folder picker or typed, with an optional name (F-2.2, F-2.3); remove after confirmation (F-2.5, files kept); open the site in the default browser; show the directory in the file manager. |
| F-6.6 | Runtimes: published and installed versions of the language runtimes, install and remove (F-4.3, F-4.4). |
| F-6.7 | Services: instances with version, address and port, user, why they run, state, blocking reasons and failures; start and stop (F-5.4); unmet requirements (F-5.3); published and installed versions of PostgreSQL and MySQL, install and remove. |
| F-6.8 | While the window is visible the state is refreshed every 2 s (daemon, projects, services) and every 10 s (web server, package index), and after every action; changes made with the CLI appear without action. Long actions show a notification until they end; failures show the daemon's message. |
| F-6.9 | The light or dark appearance follows the system. |

### Implemented (M7)

| ID | Requirement |
|---|---|
| F-7.1 | The desktop bundles (macOS `.app`/`.dmg`, Windows NSIS installer, Linux `.deb`, `.rpm`, AppImage) ship `devspaced`, `devspace` and `devspace-helper` next to the application. The Windows installer is per user and adds its directory to the user's `PATH`; the Linux packages install the programs in `/usr/bin`. |
| F-7.2 | On macOS, `devspace system setup` first installs a copy of the helper owned by the administrator, `/Library/PrivilegedHelperTools/me.romainalcon.devspace.helper`, after checking that the bytes copied have the SHA-256 shown to the user; every other operation runs that copy, and the helper refuses to perform them from anywhere else. A copy that differs from the helper of the running CLI is replaced by the next setup. |
| F-7.3 | On macOS, when the CLI is that of an application bundle, `system setup` links `/usr/local/bin/devspace` to it; a file or link there that does not point to the CLI of an application bundle blocks the setup and is never modified. `system teardown` removes the link and the installed helper. |
| F-7.4 | `system setup|teardown --dry-run` only prints the changes; `--graphical` asks for the administrator password once, with the system dialog, for every change (macOS). |
| F-7.5 | The application's Overview runs `system setup|teardown` (F-3.7): it shows the changes printed by the CLI, and applies them once the user approved, with the graphical elevation. |
| F-7.6 | The application checks for an update when it starts and every 6 hours (`latest.json` of the latest GitHub release), and offers to install it: the update is downloaded and its signature checked against the public key of the build, then the daemon is stopped, the update installed and the application restarted. A daemon of another version is restarted with the one of the application when the application starts. |
| F-7.7 | The daemon downloads the package index published with the releases (assets `index.json` and `index.json.minisig` of the release `index`) at startup and every 6 hours, and with `devspace runtime refresh` (or "Check for new versions" in the application). It is used only if its signature matches the embedded public key and its serial is higher than that of the index in use; it is kept in `<data>/index/` and used at the next startups while it is newer than the embedded one. `runtime list` shows the serial, where the index comes from (`embedded` or `downloaded`) and why the last download failed. `packages.remote_index = false` disables the downloads. |
| F-7.8 | Valkey (`valkey`, series `MAJOR.MINOR`, port `services.valkey_port`, 6379) is a service like PostgreSQL and MySQL (F-5.2 to F-5.11), built by DevSpace from the official sources for macOS and Linux, without TLS. It runs with no configuration file, on `web.address` and a socket of the private runtime directory, without password (`default` user), and saves its data in its data directory when it stops. `devspace exec` sets no variable for it. Not available on Windows. |
| F-7.9 | MySQL is also published for Linux ARM64 (Oracle's full archive). |
| F-7.10 | On Windows, PostgreSQL and MySQL are stopped with `pg_ctl stop -m fast` and `mysqladmin shutdown`, then killed after 30 s. |

### Planned

| ID | Milestone | Requirement |
|---|---|---|
| F-3.10 | M3+ | System integrations on Linux and Windows. |
| F-4.10 | M4+ | PHP sites on Windows. |
| F-5.12 | — | MySQL on macOS x86_64 (Oracle publishes no build). |
| F-7.11 | — | Developer ID signing and notarisation (macOS), Authenticode signing (Windows): ready in the release workflow, enabled once certificates exist. |

## Configuration reference (schema version 1)

```toml
schema_version = 1        # required to equal 1 if present

[logging]
level = "info"            # error | warn | info | debug | trace
format = "compact"        # compact | pretty

[web]
tld = "internal"          # sites at https://<project>.internal; not local/localhost
address = "127.0.0.2"     # loopback address of the sites (127.0.0.0/8)
http_port = 47080         # web server on that address, at least 1024
https_port = 47443        # must differ from http_port
dns_port = 47053          # DNS server on 127.0.0.1 for *.<tld> (UDP), at least 1024

[services]                # on web.address; at least 1024, distinct, not the web ports
mysql_port = 3306
postgresql_port = 5432
valkey_port = 6379

[packages]
remote_index = true       # download the signed index published with the releases
```

Rules: all keys optional, unknown keys rejected, file never overwritten by
DevSpace. Changing a `web` setting requires restarting the daemon, then
running `devspace system setup` again (the resolver or forwarding becomes
`outdated` or `missing`); run `devspace system teardown` *before* changing
`tld` or `address`, so that the old resolver file and alias are removed.
The defaults do not collide with Laravel Herd or Valet (`*.test` on
`127.0.0.1`).

## Project manifest reference (schema version 1)

File `devspace.toml` at the root of a project, optional, at most 64 KiB:

```toml
schema_version = 1        # required to equal 1 if present

[project]
name = "my-app"           # optional; see project names below

[web]                     # optional; at most one of root and proxy
root = "public"           # files; relative, no `..`; default "public"
proxy = "127.0.0.1:3000"  # or a development server: 127.0.0.1, [::1] or localhost

[runtimes]                # optional; MAJOR, MAJOR.MINOR or MAJOR.MINOR.PATCH
php = "8.4"               # newest installed match; newest installed if absent
node = "24"

[services]                # optional; the daemon runs these while the project is registered
postgresql = "18"         # the series of the newest installed match
mysql = "8.4"
valkey = "9.1"            # MAJOR.MINOR
```

Rules: all keys optional, unknown keys rejected, file only read by DevSpace
(never written, nothing in it is executed). `project.name` is read when
the project is added; `[web]` and `[runtimes]` whenever the web server
(re)starts, `[runtimes]` and `[services]` at every `devspace exec`, and
`[services]` whenever the services are re-planned (daemon start, project,
installation or choice changes).

Project names: 1–63 characters among lowercase ASCII letters, digits and
`-`, not starting or ending with `-` (a DNS label, for the `*.<tld>`
domains of M3).

## IPC protocol reference (version 6)

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
| `project_add` with `path` (absolute), optional `name` | `project_added` with `name`, `path`, `added_at` |
| `project_list` | `projects` with `projects: [{name, path, added_at}]` sorted by name |
| `project_remove` with `name` | `project_removed` with `name`, `path`, `added_at` |
| `web_install`, `web_start`, `web_stop`, `web_restart`, `web_status` | `web_status` (below) |
| `runtime_list` | `runtimes` with `index_serial`, `index_source` (`embedded`/`downloaded`), optional `index_error`, `platform`, `versions: [{name, version, available, path}]` (`path` `null` when not installed) |
| `index_refresh` | `runtimes`, after downloading the published index |
| `runtime_install` with `name`, optional `version` (requirement) | `runtime_installed` with `name`, `version`, `path`, `changed` (`false`: already installed) |
| `runtime_remove` with `name`, `version` (exact) | `runtime_removed` with the same fields |
| `runtime_env` with `directory` (absolute) | `runtime_env` with `project` (or `null`), `runtimes: [{name, version, bin_dirs, variables: [[name, value]]}]`, `missing: [message]` |
| `service_list` | `services` (below) |
| `service_start` with `name`, optional `series` | `services` |
| `service_stop` with `name`, optional `series` | `services` |

`services` fields: `address`, `instances` (`[{name, series, version,
port, user, mode, required_by, state, blocked, data_dir, log_file}]`,
`version` and `blocked` possibly `null`, `mode`
`automatic|started|stopped`, `state` shaped like `server` below),
`missing` (`[message]`).

`web_status` fields: `caddy_version`, `caddy_installed`, `tld`,
`address`, `server`
(`{state: stopped|starting|failed, message?}` or `{state: running, pid}`),
`http_port`, `https_port`, `dns_port`, `dns` (`{state: listening}` or
`{state: failed, message}`), `certificate_authority` (`null` or
`{certificate_pem, sha256}`), `system` (`resolver`, `port_forwarding`,
`certificate_trust`: `installed|missing|outdated|conflict|unsupported|unknown`;
`forwarding_active`: boolean or `null`), `sites` (`[{project, domain,
target}]` with `target` `{type: static, root}`, `{type: proxy, upstream}` or
`{type: unavailable, reason}`, or `{type: php, root, version}`), `php`
(`[{version, state}]`, `state` shaped like `server`).

Any request may instead be answered with `error`: `code` (`invalid_request`,
`already_exists`, `not_found`, `unavailable`, `failed`, `internal`) and a
human-readable `message`; the connection stays open. `added_at` is in
seconds since the Unix epoch. `web_install` and `runtime_install` may take
minutes; every client waits the time given by `Request::time_limit` (from
10 s to 45 min, see `ARCHITECTURE.md`).

Version history: 1 (M1) `ping`, `status`, `shutdown`; 2 (M2) project
requests and `error` responses; 3 (M3) web requests, `unavailable` and
`failed` codes; 4 (M4) runtime requests, PHP sites and PHP-FPM states;
5 (M5) service requests; 6 (M7) `index_refresh`, `index_source` and
`index_error`, service `valkey`.

## Privileged helper reference

`devspace-helper` is shipped next to `devspace` and performs one operation
per run, through `sudo` (or the graphical elevation, F-7.4). On macOS only
`install-helper` runs from there; the other operations run from the
installed copy (F-7.2).

| Operation | Effect (macOS) |
|---|---|
| `install-resolver --tld T --port P` | writes `/etc/resolver/T` (`nameserver 127.0.0.1`, `port P`) |
| `remove-resolver --tld T` | deletes it |
| `install-port-forwarding --address A --http-port H --https-port S` | adds `A` to `lo0` unless it is `127.0.0.1`, writes `/etc/pf.anchors/devspace` (rdr on `lo0` for `A` only: 80→H, 443→S) and a launch daemon restoring the alias and rules at boot, loads the anchor `com.apple/devspace` and enables pf; `/etc/pf.conf` and other addresses are untouched |
| `remove-port-forwarding --address A` | flushes the anchor, deletes both files, removes the alias `A` (never `127.0.0.1`; pf stays enabled) |
| `trust-certificate --sha256 F` | reads a PEM certificate on stdin, checks its SHA-256 is `F`, adds it to the System keychain as a trusted SSL root |
| `untrust-certificate --sha256 F` | same check, removes the trust setting and the certificate |
| `install-helper --sha256 F` | reads the running helper, checks its SHA-256 is `F`, writes those bytes to `/Library/PrivilegedHelperTools/me.romainalcon.devspace.helper` (`0755`, owned by root; the directory must be writable by root only) |
| `remove-helper` | deletes the installed helper |
| `link-cli --target P` | links `/usr/local/bin/devspace` to `P`, which must be `…/<name>.app/Contents/MacOS/devspace`; only replaces a link to such a CLI |
| `unlink-cli` | deletes that link if it points to the CLI of an application bundle |

Exit status: `0` success, `1` failure, `2` invalid command line. The helper
refuses to run without administrator rights and never modifies a file that
lacks the "Managed by DevSpace" marker.

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
- Privileged operations are isolated in a minimal, one-shot helper and
  require explicit user approval (confirmation, then `sudo`) (M3). In
  installed layouts the helper that runs elevated is owned by the
  administrator; the copy shipped in the user-writable bundle only runs
  elevated to install it, after the user approved its SHA-256 (M7).
- Application updates are verified against the updater public key of the
  build; the remote package index against the index key, and never
  replaces a newer index (M7).
- The web, DNS and Caddy admin surfaces are never exposed beyond the
  loopback interface; Caddy's admin API is disabled (M3).
- The local CA key is readable by the current user only; as with any local
  development CA, code running as this user could issue trusted
  certificates (M3).
- IPC clients are authenticated with OS-level permissions and peer identity
  (M1); the daemon is also authenticated by its clients.
- Downloaded packages are verified (signed index, then SHA-256 of each
  archive) before use; archives are extracted without path traversal or
  links leaving the installation (M4).
- PHP-FPM listens on a Unix socket readable by the user only, never on a
  TCP port (M4).
- Services listen on the loopback sites address only, and on Unix sockets
  in the private runtime directory; MySQL reads no option file and
  disables file import/export (`secure_file_priv`). Their administrators
  have no password: any local user can connect through the loopback
  address (M5).
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
