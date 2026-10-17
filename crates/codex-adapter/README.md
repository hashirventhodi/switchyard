# codex-adapter

The Switchyard adapter for OpenAI Codex, over `codex app-server` (JSON-RPC, one JSON message per line on stdio). It implements `adapter_protocol::Adapter`; the host (`switchyard-core`'s `codex::CodexLauncher`) runs the process and moves lines. See `src/lib.rs` for the session flow and `src/mode.rs` for the mode table.

## Models

- A Session's Model rides on `thread/start` (`Launch.model`) or `thread/resume` (`resume`'s `model`); none means Default, and the thread's result names the Model it runs on (`session_init.model`, every `token_usage`).
- `list_models` is `model/list` (every page), answered by `models_listed` (`id` is the Model's `model` slug) or, on an error, `cannot_list_models`. It is held until the thread exists, so the handshake's request ids never move.
- `switch_model_live` is **false**: the app-server reports no Model per request, so a live switch could not be confirmed. The host applies a switch on resume. An error naming the Model a thread was resumed on, before any of its requests ran, is `model_change_failed` (Codex has no error code for it), so the host can fall back to Default.

## Fixtures

`crates/adapter-protocol/fixtures/codex/<scenario>/`, replayed by `tests/conformance.rs`. Besides the standard files, each has `launch.json` (the `Launch` the adapter is started with) and `writes.jsonl` (the JSON-RPC the adapter writes on its own: handshake, thread and turn starts, error answers).

The current fixtures are synthesised from the schema codex-cli 0.144.1 generates (`codex app-server generate-json-schema`), not captured from a live run.

## Recording a real session (manual, needs a Codex login and makes model calls)

1. Run the app with `SWITCHYARD_RECORD_DIR=/some/dir pnpm tauri dev` (optionally `SWITCHYARD_CODEX=/path/to/codex`).
2. Start an OpenAI Codex session and walk the scenario (approve, skip, answer a question, stop, resume...).
3. `/some/dir/codex-<pid>.input.jsonl` is everything Codex wrote (the fixture's `input.jsonl`); `codex-<pid>.writes.jsonl` is everything written to it (split it into `commands.jsonl` responses and `writes.jsonl`).
4. Write `expected-events.jsonl` from what the session should show, add `meta.json` with the exact `codex --version`, and widen `crates/switchyard-core/compat.json` if the version is new.

## Manual gate check

One Claude Code session and one Codex session in two hand-made worktrees (`git worktree add`), each in Ask first with a prompt that makes it run a shell command: both cards appear on the Dashboard with their own hues and statuses, each request toast's Approve answers its own agent, and both turns finish.
