# DevSpace desktop

Tauri 2 shell (`src-tauri`) and Vue 3 / TypeScript / Tailwind CSS 4 /
shadcn-vue interface (`src`). The application only talks to the daemon
through the IPC protocol; see `ARCHITECTURE.md` (Desktop application).

## Development

```sh
cargo build -p devspace-daemon -p devspace-cli -p devspace-helper   # next to the app
cd apps/desktop
npm install
npx tauri dev
```

The application starts `devspaced` from its own directory (the workspace
`target/debug` in development). `DEVSPACE_HOME` and `DEVSPACE_LOG` are
honoured, and passed on to the daemon it starts.

## Bundles

```sh
cd apps/desktop
TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.devspace-keys/updater.key)" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… npm run bundle
# without the updater key:
npm run bundle -- --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

`npm run bundle` merges `src-tauri/tauri.bundle.conf.json`, which builds
`devspaced`, `devspace` and `devspace-helper` in release mode
(`scripts/sidecars.mjs`) and ships them next to the application. Releases
are built by `.github/workflows/release.yml` on a `vX.Y.Z` tag.

## Checks

```sh
npm run build        # vue-tsc type-check + vite build
cargo test -p devspace-desktop
```

Components in `src/components/ui` come from shadcn-vue (`components.json`,
style `new-york`, base colour `neutral`).
