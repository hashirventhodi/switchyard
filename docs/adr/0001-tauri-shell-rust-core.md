# Tauri shell with a Rust core, not Electron

Switchyard needs a desktop shell, a long-running local core that owns PTYs and git, and a renderer. Electron would give node-pty and xterm.js with one language end to end; we chose Tauri with the core as a Rust crate (`switchyard-core`, no Tauri types inside it) because the core is the durable, replaceable layer and Rust's process and git tooling (portable-pty, libgit2) suits it, while the renderer stays React + TypeScript over one typed event stream. The cost accepted: the Claude Code adapter runs in a bundled Node sidecar (see the handoff), so the bundle carries two runtimes.

## Consequences

- Event and command types live in a schema crate with generated TypeScript; wrapping the core in a standalone binary later is a build target, not a refactor.
- xterm.js is used only for shell tabs; agent output is rendered from events, so node-pty's maturity matters less than it would under the handoff's original design.
