# DevSpace package index

`index.json` lists the runtime archives DevSpace may install. It is
embedded in DevSpace together with its signature `index.json.minisig` and
the public key `devspace.pub`; DevSpace refuses to use an index whose
signature does not match that key, then checks the SHA-256 of every archive
before extracting it. See `crates/core/src/runtime/index.rs` for the format.

## Updating the index

1. Edit `index.json`: add versions, increase `serial`.
   - Node.js: URLs and SHA-256 from `https://nodejs.org/dist/vX.Y.Z/SHASUMS256.txt`
     (`.tar.gz` for macOS and Linux with `strip_components: 1`, `.zip` for
     Windows; `bin` is `["bin"]`, `["."]` on Windows).
   - PHP on macOS and Linux: the `cli` and `fpm` archives of
     `https://dl.static-php.dev/static-php-cli/common/` (`destination: "bin"`).
     static-php-cli publishes no checksums: download each archive and hash
     it (`shasum -a 256`).
   - PHP on Windows: the NTS x64 zip of `https://downloads.php.net/~windows/releases/releases.json`
     (its `sha256` field); `bin` is `["."]`.
   - PostgreSQL: the archives of `https://github.com/theseus-rs/postgresql-binaries/releases`
     (`postgresql-X.Y.0-<target>.tar.gz`, `.zip` for Windows,
     `strip_components: 1`, `bin` is `["bin"]`); SHA-256 in the `.sha256`
     file next to each archive (also GitHub's asset digest). Targets:
     `aarch64-apple-darwin`, `x86_64-apple-darwin`,
     `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
     `x86_64-pc-windows-msvc`.
   - MySQL: `https://cdn.mysql.com/Downloads/MySQL-<series>/` —
     `mysql-<v>-macos15-arm64.tar.gz`,
     `mysql-<v>-linux-glibc2.28-x86_64-minimal.tar.xz`,
     `mysql-<v>-linux-glibc2.28-aarch64.tar.xz` (full archive, no minimal
     one exists for ARM64), `mysql-<v>-winx64.zip` (`strip_components: 1`,
     `bin` is `["bin"]`).
     Oracle publishes MD5 only: download each archive, compare its MD5
     with the download page, and hash it (`shasum -a 256`).
   - Valkey (macOS and Linux only): built by DevSpace. Pin the source in
     `scripts/packages/valkey-sources.txt` (SHA-256 from
     `https://github.com/valkey-io/valkey-hashes`, checked by downloading
     the archive), run the `Packages` workflow with that version, and take
     URLs, sizes and SHA-256 from its summary:
     `https://github.com/romainalcon/devspace/releases/download/valkey-<v>/valkey-<v>-<platform>.tar.gz`
     (`strip_components: 1`, `bin` is `["bin"]`).
   - `size` is the exact size in bytes of each archive (at most 1 GiB).
2. Sign it with the maintainer key (asks for its password):

   ```sh
   cargo run -q -p devspace-registry -- sign \
     --secret-key ~/.devspace-keys/index.key \
     --public-key registry/devspace.pub registry/index.json
   ```
3. `cargo test -p devspace-core runtime::index` checks the signature of
   the embedded index.
4. Publish it to the installed DevSpace builds (they download it every
   6 hours and use it if its serial is higher than theirs):

   ```sh
   gh release view index >/dev/null 2>&1 || \
     gh release create index --title "Package index" --latest=false \
       --notes "Signed package index downloaded by DevSpace."
   gh release upload index registry/index.json registry/index.json.minisig --clobber
   ```

   Never publish an index with a serial lower than or equal to the
   previous one: DevSpace ignores it.

The secret key never goes into the repository. It was created with
`devspace-registry keygen --secret-key <path> --public-key registry/devspace.pub`;
losing it means publishing a new public key with a new DevSpace build.
