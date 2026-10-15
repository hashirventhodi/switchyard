# Claude Code sidecar

The Claude Code adapter (ADR-0002): a Node script around the Agent SDK
(`@anthropic-ai/claude-agent-sdk` 0.3.300, paired with Claude Code 2.1.300) that speaks the
adapter protocol (`crates/adapter-protocol/README.md`) on stdio.

- `src/translate.ts`: the adapter proper, a pure translator from SDK messages to protocol
  events and from protocol commands to driver ops. No SDK at runtime.
- `src/driver.ts`: connects the translator to a live `query()` (streaming input).
- `src/main.ts`: the entry point, `node claude-sidecar.mjs --config <json>` (or `--manifest`,
  which prints the adapter manifest for the core's compat check), bundled by
  `pnpm build:sidecar` into `sidecar/dist/claude-sidecar.mjs`.
- `src/replay.ts`: the translator behind a request/reply loop, used by the conformance test
  (`cargo test -p switchyard-core --test claude_code_conformance`; Node >= 22.18 on PATH).

The core starts it in the worktree with `{"cwd", "permission_mode", "claude_path"}`; with no
`claude_path` it runs the `claude` found on PATH or in the usual install locations.

## Recording a fixture (manual; costs a model call)

The fixtures under `crates/adapter-protocol/fixtures/claude-code/` are synthesised from the
SDK's message types. To replace them with a real capture:

```sh
pnpm build:sidecar
mkdir -p /tmp/rec && cd /path/to/a/scratch/worktree
SWITCHYARD_RECORD_DIR=/tmp/rec node <repo>/sidecar/dist/claude-sidecar.mjs \
  --config '{"cwd":"'"$PWD"'","permission_mode":"ask_first","claude_path":null}'
{"type":"send_text","text":"List the files here, then stop."}
# wait for {"type":"turn_complete"}, optionally send an {"type":"interrupt"}, then Ctrl-D
```

Review `/tmp/rec/expected-events.jsonl`, then copy the directory to
`crates/adapter-protocol/fixtures/claude-code/<scenario>/`. A new CLI version means a new
recording, never an edited one, and an updated supported range in
`crates/switchyard-core/compat.json` (a core test fails if a fixture falls outside it).

## Manual end-to-end check (the one place a real CLI runs)

1. `pnpm tauri dev`, add a repository with a linked worktree (`git worktree add ../x -b x`).
2. New session: Claude Code, that worktree, Ask first, a prompt such as "List the files".
3. The session view streams prose and tool rows; the pill walks Running, then Needs you.
4. Send a follow-up from the composer, press Stop (or Esc) mid-turn: the pill shows Paused
   and `git status` in the worktree is unchanged.

## Manual compat check (real CLI, no model call)

1. `pnpm tauri dev` with `claude` on PATH in the supported range: the Dashboard shows no Setup
   block and New session is open.
2. Temporarily narrow `supported.below` in `crates/switchyard-core/compat.json` below the
   installed version, restart: the Dashboard's Setup block and the New session dialog both say
   "Claude Code x.y.z is not yet supported by this build of Switchyard…" with a link that opens
   the releases page in the browser; Start session is disabled.
3. Start the app with `claude` hidden (e.g. `PATH=/usr/bin:/bin`, no `~/.local/bin/claude`):
   Setup lists Claude Code as not installed with its install command. Restore it and press
   Check again: the block disappears.
4. `cat ~/Library/Application\ Support/dev.switchyard.app/compat-log.jsonl` after a failed start shows
   one JSON record per failure.
