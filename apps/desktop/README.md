# DevSpace desktop

Tauri 2 shell (`src-tauri`) and Vue 3 / TypeScript / Tailwind CSS 4 /
shadcn-vue interface (`src`). The application only talks to the daemon
through the IPC protocol; see `ARCHITECTURE.md` (Desktop application).

## Development

```sh
cargo build -p devspace-daemon -p devspace-cli   # devspaced next to the app
cd apps/desktop
npm install
npx tauri dev
```

The application starts `devspaced` from its own directory (the workspace
`target/debug` in development). `DEVSPACE_HOME` and `DEVSPACE_LOG` are
honoured, and passed on to the daemon it starts.

## Checks

```sh
npm run build        # vue-tsc type-check + vite build
cargo test -p devspace-desktop
```

Components in `src/components/ui` come from shadcn-vue (`components.json`,
style `new-york`, base colour `neutral`).
