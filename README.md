# Switchyard

A desktop app for running several local AI coding agents in parallel, each in its own git worktree. Vocabulary: `GLOSSARY.md`. Decisions: `docs/adr/`.

## Layout

| Path | What |
| --- | --- |
| `crates/switchyard-core` | The core: git, store, (later) processes, policy, session state machine. No Tauri types (ADR-0001). |
| `crates/switchyard-schema` | Every `Command` and `Event` between core and renderer. TypeScript is generated from it into `ui/src/schema/`. |
| `src-tauri` | The desktop shell (crate `switchyard-app`). Only bridges the core's stream onto Tauri IPC. |
| `ui/` | The React + TypeScript renderer (Vite root). Talks to the core only through `ui/src/core.ts`. |

The renderer lives in `ui/`, not Tauri's default `src/`.

## Develop

Requires Rust (stable), Node 24 and pnpm.

```sh
pnpm install
pnpm tauri dev                                   # run the app
cargo test --workspace --exclude switchyard-app  # core + schema tests (seam 1)
pnpm test                                        # renderer tests against a fake core
pnpm tauri build --no-bundle                     # build the app
```

After changing `crates/switchyard-schema`, run `cargo test -p switchyard-schema` to regenerate `ui/src/schema/` and commit the result; CI fails if it is stale.
