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

## M4 — Runtime package manager 🔄 (awaiting validation)

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

## M5 — Services ⏳

- Supervised databases and caches, loopback-only by default.

## M6 — Desktop application ⏳

- Vue 3 + TypeScript + Tailwind CSS + shadcn-vue UI on the IPC API.

## M7 — Distribution ⏳

- Signed installers and updates for each platform.
