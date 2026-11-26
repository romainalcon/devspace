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

## M2 — Project registry ✅

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
- ✅ Validated; M3 started on 2026-11-25.
- ⏳ Execution on Windows and Linux (CI, once a remote exists).

## M3 — Web server, domains & privileged helper ✅

Design choices approved on 2026-11-25: pinned Caddy download, embedded DNS
server plus resolver file, one-shot helper through `sudo`, static files or
loopback proxy per project, `pf` forwarding of ports 80/443; then (same day)
configurable domain and address, `*.internal` on `127.0.0.2` by default, to
coexist with Laravel Herd (`*.test` on `127.0.0.1`).

- ✅ Pinned Caddy v2.11.9 downloaded by the daemon over HTTPS only, verified
  against embedded SHA-256 checksums (cross-checked with the release's
  SHA-512 file), extracted without path traversal, version-checked,
  installed atomically (`devspace web install`).
- ✅ Caddy supervised by the daemon as the current user: generated JSON
  configuration, admin API disabled, loopback-only listeners on
  unprivileged ports (47080/47443 by default), automatic start with the
  daemon, restart on project changes, graceful stop, failure reporting,
  orphan cleanup after a daemon crash (pid file + executable check).
- ✅ `web.tld` (default `internal`) and `web.address` (default `127.0.0.2`,
  any `127.0.0.0/8` address) in the configuration; the web server only
  listens there, over IPv4.
- ✅ `https://<project>.<tld>` and its subdomains: static files (`[web] root`,
  default `public/`) confined to the project, or a loopback development
  server (`[web] proxy`); HTTP redirects to HTTPS.
- ✅ Local CA created by Caddy in the DevSpace data directory; fingerprint
  reported by `devspace web status`.
- ✅ DNS server in the daemon on `127.0.0.1:47053` (UDP) answering
  `*.<tld>` with the sites address, refusing everything else.
- ✅ `devspace-helper`: one-shot privileged helper with six typed
  operations; refuses to run unprivileged; never touches files without the
  DevSpace marker; trusts only the certificate whose fingerprint was
  approved.
- ✅ `devspace system setup|teardown`: plan shown, confirmation, then `sudo`
  per change; macOS implementation (resolver file, `lo0` alias, `pf` anchor
  and launch daemon restoring both at boot, System keychain). When the CA
  does not exist yet, the trust is approved in a second step, once the web
  server could start on the new address. Conflicts (e.g. another tool's
  resolver file) abort the setup before any change.
- ✅ IPC protocol version 3: `web_install|start|stop|restart|status`,
  `unavailable` and `failed` error codes.
- ✅ Unit tests (archives incl. hostile entries, checksums, Caddy
  configuration, sites, DNS, helper operations and certificates, managed
  files, plans) and an end-to-end test (DNS over UDP, web status).
- ✅ Manual run on macOS ARM64 without privileges: download, start, HTTPS
  with the local CA, redirects, wildcard subdomains, proxy, DNS, reload on
  project change, recovery after `kill -9` of the daemon, conflict
  detection with Herd (`*.test`); with the new defaults, DNS answering
  `127.0.0.2`, refusal to start before the alias exists, setup plan
  displayed without conflict.
- ✅ First privileged run by the user: resolver for `*.internal`, alias
  `127.0.0.2` and pf forwarding, CA trust installed with Herd running;
  `https://<project>.internal` reached through port 443.
- ✅ Fixes from that run: CA trust check used the `basic` policy instead of
  `ssl` (reported "missing"); readiness check stalled up to ~70 s once pf
  was active (now a bind check; restarts take ~0.1 s); PHP projects served
  as empty static sites (now 503 with an explanation); empty error pages.
- ✅ Validated by the user on 2026-11-26 (M4 started), with the two
  points below still open.
- ⏳ `devspace system teardown` with sudo; persistence across a reboot.
- ⏳ Linux and Windows system integrations (resolver, port forwarding,
  trust store, elevation) report "not supported" for now. The web server,
  DNS server and CA code is portable but has not been built or run there
  (CI, once a remote exists); `platform` and `helper` are cross-checked
  with clippy for Windows and Linux.

## M4 — Runtime package manager ✅

Design choices approved on 2026-11-26: PHP and Node.js; PHP binaries from
static-php-cli (macOS, Linux) and windows.php.net (Windows), Node.js from
nodejs.org; authenticity through a DevSpace index signed with the
maintainer's Ed25519 key (minisign format), checksums per archive.

- ✅ Package index `registry/index.json` (schema v1): every runtime version
  per platform with archive URLs (HTTPS only), sizes and SHA-256; strict
  parsing, paths confined to the installation. Embedded in DevSpace with
  its signature, verified against the embedded public key before parsing.
  Index #1: Node.js 22.23.5, 24.22.3, 26.13.1; PHP 8.4.23 and 8.5.8
  (macOS, Linux), 8.4.28 and 8.5.13 (Windows).
- ✅ `devspace-registry` maintainer tool: key generation (password-encrypted
  secret key), signing, verification; compatible with `minisign`.
- ✅ Safe extraction of whole archives (`archive::extract_tree`): no
  absolute or `..` names, no drive or stream separators, links created
  last and resolved inside the destination, no hard links or devices,
  executable bit only (no setuid), size and entry limits.
- ✅ Installation in a staging directory: download of at most the declared
  size, SHA-256 check, extraction, version check of every program
  (`node --version`, `php -v`, `php-fpm -v`), receipt, atomic rename;
  nothing left on failure. Several versions side by side in
  `<data>/runtimes/<name>/<version>/`; removal; leftovers cleaned at
  daemon start.
- ✅ Per-project selection: `[runtimes] php = "8.4"`, `node = "24"` in
  `devspace.toml` (`MAJOR`, `MAJOR.MINOR` or exact), newest installed match;
  newest installed version when the manifest does not say.
- ✅ PHP sites: a root with `index.php` and no `index.html` is served by
  Caddy (`php_fastcgi` equivalent) through one PHP-FPM master per version
  in use, supervised by the daemon (private Unix socket `0600`, generated
  `php-fpm.conf` and development `php.ini`, crash recovery by pid file),
  started and stopped with the sites; installing or removing a version
  reloads them.
- ✅ `devspace runtime list|install|remove`, `devspace exec -- <command>`
  (project versions first in `PATH`, `PHPRC` for the CLI `php.ini`).
- ✅ IPC protocol version 4: `runtime_list|install|remove|env`, PHP sites
  and PHP-FPM states in `web_status`.
- ✅ Unit tests (versions, index and signatures incl. tampering, hostile
  archives, installation with a local package incl. checksum and version
  failures, PHP-FPM configuration, Caddy PHP route, PHP site selection)
  and an end-to-end test (index listing, project environment, refusals).
- ✅ Manual run on macOS ARM64 in an isolated `DEVSPACE_HOME`: real
  installation of PHP 8.4.23, 8.5.8 and Node.js 24.22.3 (npm links),
  `devspace exec` (php, node, npm, exit codes), two PHP projects on two
  versions at once over HTTPS, hot reload after an installation, removal
  of a version in use, recovery after `kill -9` of the daemon, clean stop.
- ✅ Index #1 signed with the maintainer key (`registry/devspace.pub`, key id
  `772CAD8DC23A3EEE`) on 2026-11-26.
- ⏳ PHP sites on Windows (no PHP-FPM; `php-cgi` would be needed): reported
  as not served; installation and `devspace exec` are portable but have
  not been run on Windows or Linux (CI, once a remote exists).
- ⚠️ static-php-cli publishes no checksums or signatures: its archives were
  hashed when pinned (trust on first download, then fixed by the signed
  index), and lag a few patch releases behind php.net. Node.js checksums
  come from `SHASUMS256.txt` (its GPG signature was not checked: no gpg on
  the pinning machine); Windows PHP checksums from `releases.json`.
- ⚠️ No automatic index update: a new index ships with a new DevSpace
  build until a hosting exists (M7).
- ✅ Validated by the user on 2026-11-26 (M5 started).

## M5 — Services ✅

Design choices approved on 2026-11-26: PostgreSQL and MySQL; instances
shared by every project, one per service and series (major version of
PostgreSQL, `MAJOR.MINOR` of MySQL), started when a project requires them;
standard ports on the sites address (`127.0.0.2:5432`, `127.0.0.2:3306`),
next to Laravel Herd's on `127.0.0.1`; administrators without a password
(as Herd). Redis/Valkey postponed to M7: neither publishes macOS or
Windows binaries, and building them needs a hosting.

- ✅ Packages `postgresql` and `mysql` in the signed index (#2):
  PostgreSQL 17.12, 18.7 and 19.0 from theseus-rs/postgresql-binaries
  (SHA-256 published with each archive, cross-checked with GitHub's
  digests); MySQL 8.4.13.1 and 9.7.4.1 LTS from cdn.mysql.com (SHA-256
  computed at pin time, MD5 cross-checked with Oracle's download pages).
- ✅ Four-component versions (`8.4.13.1`); `.tar.xz` archives (liblzma,
  statically linked); tar hard links recreated as copies of a file
  extracted before them by the same archive.
- ✅ `[services]` in `devspace.toml` (`postgresql = "18"`, `mysql = "8.4"`);
  `[services]` in `config.toml` (`mysql_port`, `postgresql_port`).
- ✅ Plan of the instances: required by a project or started by the user,
  unless stopped by the user; one series per service at a time (they share
  its port); choices stored in the database (schema v2).
- ✅ Data directory created atomically at the first start (`initdb
  --auth=trust`, `mysqld --initialize-insecure` plus `root@127.%`);
  servers supervised by the daemon (foreground, own session, pid files,
  orphan cleanup, `SIGINT` fast shutdown for PostgreSQL, 30 s stop
  timeout); Unix sockets in the private runtime directory.
- ✅ IPC protocol version 5: `service_list|start|stop`.
- ✅ `devspace service list|install|start|stop`; `devspace exec` points
  `psql` and `mysql` at the instances (`PGHOST`, `PGPORT`, `PGUSER`,
  `MYSQL_HOST`, `MYSQL_TCP_PORT`).
- ✅ Unit tests (versions, archives, series, plan, choices, command lines,
  initialisation with fake tools, CLI output) and an end-to-end test
  (requirements, refusals, choices without installation).
- ✅ Index #2 signed with the maintainer key on 2026-11-26.
- ✅ Manual run on macOS ARM64 in an isolated `DEVSPACE_HOME`: real
  installation of PostgreSQL 17.12 and 18.7 and MySQL 8.4.13.1; first
  start with initialisation (< 1 s); TCP on `127.0.0.2` and socket
  connections; `devspace exec` with `psql` and `mysql`; automatic start
  of an instance required by a project; stop remembered across daemon
  restarts; restart of a `started` instance with the daemon; data kept;
  recovery after `kill -9` of the daemon (orphans stopped) and of a
  server (reported, restarted by `start`); switch between two PostgreSQL
  series on the same port; removal of a version in use; project removal;
  busy port reported; nothing left running after `daemon stop`.
- ✅ Fixes from that run: `initdb` only trusts `127.0.0.1` (DevSpace now
  writes its own `pg_hba.conf`, passed as `hba_file`); the socket
  directory was not private; MySQL prints `8.4.13` for the `8.4.13.1`
  rebuild (version checks ignore the rebuild number); an unsatisfied
  requirement naming a series now creates the instance, so that choices
  apply to it before installation.
- ⚠️ On macOS, a server listening on every address (`0.0.0.0:5432`) does
  not prevent DevSpace from listening on `127.0.0.2:5432`; connections to
  `127.0.0.2` reach DevSpace.
- ⏳ MySQL 9.7 and PostgreSQL 19 pinned and checksummed, not run.
- ✅ Validated by the user on 2026-11-26.
- ⚠️ MySQL publishes no macOS x86_64 build of 8.4/9.7, and only a full
  Linux ARM64 archive (≈ 820 MB, above the index limit): not available on
  these two platforms.
- ⚠️ Windows: the servers run, but are stopped by `TerminateProcess` (no
  graceful stop yet), and nothing has been run there (CI, once a remote
  exists).

## M6 — Desktop application ✅

Design choices approved on 2026-11-26: parity with the CLI except
`system setup|teardown` (the application shows the integrations and the
command to run); state refreshed by polling (no protocol change); window
plus a menu bar icon, closing the window keeps the application; the
daemon is started with the application.

- ✅ `devspace-ipc::launcher`: start, stop and status of the daemon, moved
  from the CLI and shared with the desktop application;
  `Request::time_limit` replaces the per-command timeouts of the CLI; the
  single-instance lock moved from `core` to `platform`.
- ✅ Tauri shell: commands `daemon_status|start|stop`, `daemon_request`
  (whole protocol except `shutdown`), `open_site`, `reveal_project`;
  daemon autostart; menu bar icon (state, start/stop, open, quit); window
  hidden on close; refusal to run elevated; strict CSP, capabilities
  reduced to the folder picker.
- ✅ Interface (Vue 3, TypeScript, Tailwind CSS 4, shadcn-vue): Overview,
  Projects, Runtimes, Services; polling every 2 s / 10 s while visible;
  notifications for long actions; light and dark appearance.
- ✅ Unit tests (launcher incl. early exit and unresponsive daemon, time
  limits, desktop state and errors); `vue-tsc` and `vite build`.
- ✅ Manual run on macOS ARM64 (`npx tauri dev`, real environment): daemon
  started by the application, interface polling observed in the daemon
  debug log (status, projects, services every 2 s; web and runtimes every
  10 s), daemon still running after the application process exits.
- ✅ Fix from that run: the first tray refresh raced with the autostart
  (the lock probe made the start report an unresponsive daemon); probes,
  starts and stops are now serialised in the application.
- ✅ Fix from the user's first run: installing an exact four-component
  version (MySQL `9.7.4.1`, from the Services page or `devspace service
  install mysql 9.7.4.1`) was refused; version requirements now accept
  `MAJOR.MINOR.PATCH.BUILD` (M5 bug).
- ⚠️ No visual review of the interface from the session (screenshots
  were not possible); the user validated the milestone without a
  dedicated review pass.
- ✅ Validated by the user on 2026-11-26; M7 started the same day.
- ⚠️ The interface was not run on Windows or Linux. The tray icon uses the
  application icon (no monochrome menu bar template yet).
- ⚠️ No bundle including `devspaced` yet: a built application looks for
  `devspaced` next to its executable and reports it missing (M7).

## M7 — Distribution 🔄

Design choices approved on 2026-11-26: releases, application updates and
DevSpace-built packages hosted on GitHub Releases (a GitHub remote is
created); no signing certificates yet (macOS ad-hoc signature, Windows
unsigned; the signing steps are ready and activate when the secrets
exist); the whole scope below stays in M7. Then (same day): public
repository `romainalcon/devspace`; on macOS `system setup` first installs
a root-owned copy of the helper in `/Library/PrivilegedHelperTools/` and
only runs that copy, and links `/usr/local/bin/devspace` to the bundled
CLI; MySQL on Linux ARM64 from Oracle's full archive (index size limit
raised), still none on macOS x86_64.

- ✅ Bundles shipping `devspaced`, `devspace` and `devspace-helper`
  (`npm run bundle`, `tauri.bundle.conf.json`, `scripts/sidecars.mjs`):
  macOS `.app`/`.dmg` (ad hoc signature), Windows NSIS per user with the
  `PATH` updated, Linux `.deb`/`.rpm`/AppImage.
- ✅ macOS: administrator-owned copy of the helper installed by `system
  setup` after a SHA-256 check, only copy allowed to run the other
  operations; `/usr/local/bin/devspace` link; `--dry-run`, `--graphical`.
- ✅ `system setup|teardown` from the application (plan, approval, system
  password dialog).
- ✅ Application updates (`tauri-plugin-updater`, updater key generated by
  the maintainer); daemon of another version restarted at startup.
- ✅ Remote package index (release `index`, signature and strictly higher
  serial), `devspace runtime refresh`, `packages.remote_index`; IPC
  protocol version 6.
- ✅ Valkey service (8.1, 9.1; `services.valkey_port`), build script and
  `Packages` workflow; Windows graceful stop of PostgreSQL and MySQL;
  archive limit 1 GiB for MySQL Linux ARM64 (archives hashed, MD5 checked
  against Oracle's page).
- ✅ `Release` workflow (five targets, draft release, `latest.json`,
  signing enabled by secrets).
- ✅ Fix: release builds failed on macOS (stripped proc-macro dylibs).
- ✅ Manual run on macOS ARM64: bundle built and started in an isolated
  `DEVSPACE_HOME` (daemon started from the bundle, `system setup|teardown
  --dry-run` plans with the helper installation and the link); Valkey
  9.1.4 built locally and supervised by the daemon (start, `PING` through
  `devspace exec`, private socket, data saved at stop, nothing left
  running); remote index download reported (404 before publication).
- ✅ Public repository `romainalcon/devspace` created and pushed; updater
  key added to its secrets.
- ✅ Valkey 8.1.12 and 9.1.4 built by the `Packages` workflow for macOS
  and Linux (ARM64, x86_64); index #3 (Valkey, MySQL Linux ARM64) signed
  by the maintainer on 2026-11-26 and published as the release `index`.
- ✅ Real run on macOS ARM64 with index #3: Valkey 9.1.4 and 8.1.12
  installed from the releases (download, SHA-256, version checks),
  started by the daemon, answering through `devspace exec -- valkey-cli`,
  nothing left after `daemon stop`.
- ✅ Fixes from the first runs: the first Valkey archives aborted at
  startup (a full `strip` removed the symbols Valkey 9 loads its Lua
  engine with; deleted before any index referenced them, rebuilt with
  `strip -S`, and the build now checks that the server answers `PING`);
  clippy 1.100 (`chunks_exact_to_as_chunks`); test helpers only used by
  macOS/Unix tests failed on Linux and Windows (dead code, M3–M7, never
  run before); CI clippy runs with `--keep-going`.
- ⏳ First `Release` run (tag `v0.1.0`).
- ⏳ Real `system setup|teardown` with sudo and from the application
  (needs the user's password); update installed from a published release.
- ✅ First CI runs on the five platforms: clippy and tests pass on macOS
  ARM64/x86_64, Linux x86_64/ARM64 and Windows x86_64. Fixes from the
  Windows run: the signed index was converted to CRLF by Git checkouts
  (its signature failed: every Windows build would have had an unusable
  package manager; `.gitattributes`); Unix-only tests (tar symbolic links,
  PHP-FPM configuration, `link-cli` path); Windows-only dead test helpers;
  `RunEvent::Reopen` only exists on macOS.
- ⚠️ The Windows runners of GitHub Actions are elevated, and the daemon
  refuses to run elevated: the end-to-end tests of the daemon are skipped
  there, so the daemon has still never run on Windows. The M1 to M5
  Windows warnings (named pipe ACL, servers, stop) stand.
- ⚠️ Windows and Linux bundles, the NSIS `PATH` hooks and the Windows stop
  commands have never run (first `Release` run pending). The Valkey Linux
  builds start and answer `PING` in the `Packages` workflow.
- ⚠️ `system setup` dry-run refuses while Caddy is not installed (same
  rule as setup, M3).
