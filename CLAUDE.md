# DevSpace

## Project overview

DevSpace is a cross-platform local development
environment manager.

Its goal is to provide a modern alternative to
Laravel Herd, with reproducible environments,
native runtimes and project-level orchestration.

## Technology stack

- Rust stable, edition 2024
- Cargo Workspace
- Tokio
- Tauri 2
- Vue 3
- TypeScript
- Tailwind CSS
- shadcn-vue
- SQLite
- Caddy

## Architecture

The core engine must remain completely independent
from Tauri and the graphical interface.

The daemon is the single source of truth for
running projects, services and processes.

Both the CLI and desktop application communicate
with the daemon through a shared IPC protocol.

Platform-specific implementations must be isolated
inside the platform crate.

## Target platforms

- macOS ARM64: initial development target
- macOS x86_64
- Windows x86_64
- Linux x86_64 and ARM64

Do not assume that macOS-specific commands work
on other platforms.

## Development principles

- Prefer simple and maintainable solutions.
- Avoid premature abstractions.
- Do not duplicate business logic.
- Use strongly typed interfaces.
- Use structured logging.
- Avoid unwrap and expect in production code.
- Handle errors explicitly.
- Write tests for critical business logic.
- Document public APIs.
- Never implement placeholder features as if
  they were functional.

## Security

- Never execute the main daemon as root.
- Isolate privileged operations.
- Authenticate local IPC clients using OS-level
  permissions and peer identity where available.
- Verify package checksums and signatures.
- Never expose local services publicly by default.
- Never log secrets or credentials.
- Prevent archive path traversal.
- Do not execute untrusted project commands
  without explicit user approval.
- Do not overwrite existing system configurations
  without user approval.

## Quality checks

Before completing a task, run:

- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace

Run frontend checks when modifying the desktop app.

## Workflow

Implement one milestone at a time.

Read the documentation before modifying the
architecture.

Do not implement features outside the current
milestone without explicit approval.

Update the roadmap and documentation when needed.

