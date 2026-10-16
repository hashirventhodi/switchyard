# adapter-protocol

The Switchyard adapter protocol, version **0.2.0**, and its conformance runner.

Switchyard drives agent CLIs (Claude Code, Codex, ...) only through their structured modes, never by scraping a terminal (ADR-0002). An **adapter** maps one CLI's structured output into this protocol's **events**, and maps this protocol's **commands** into that CLI's input. A **recorded fixture** pins that mapping for a specific CLI version, and the **conformance runner** checks an adapter against it.

This crate is self-contained (no dependency on the rest of Switchyard) so it can be published on its own.

## Versioning

- `PROTOCOL_VERSION` is the version this crate defines. It follows semver: while it is `0.x`, a minor bump is breaking.
- Every adapter declares the range it implements in its `AdapterManifest` (`implements`, a semver requirement such as `^0.2`). Hosts and the runner refuse an adapter whose range does not include `PROTOCOL_VERSION`.
- The manifest also declares `switch_model_live`: true when the adapter switches its CLI's Model live (`set_model`) and can tell the main agent's requests from subagent and background calls, so its `model_changed` is truthful. An adapter that cannot make that distinction must declare false (the default) and never send `model_changed`; the host then applies a switch only when the Session resumes.
- Every fixture records the protocol version its expected events are written in (`meta.json` `protocol`). The runner refuses a fixture whose version is not caret-compatible with its own (for example a `0.3.0` fixture under a `0.2.0` runner).

## Changes

- **0.2.0**: `token_usage` is per Model (`model`). A 0.1 message without it still parses, as usage on the Session's Model. Live Model switching: the `set_model` command, the `model_changed` and `model_change_failed` events, and the manifest's `switch_model_live` (false when left out, as from a 0.1 adapter).
- **0.1.0**: first version.

## Wire format

Events and commands are JSON objects, internally tagged by `type`, with snake_case names and fields. Optional fields are written as `null` (they are always present on output, and may be omitted on input). Hosts and out-of-process adapters exchange them in this form; the Rust types in `src/protocol.rs` are the reference.

### Events (adapter to host)

| `type` | Fields |
| --- | --- |
| `session_init` | `external_session_id`, `permission_mode` (the mode the CLI actually started in: `explore` / `ask_first` / `auto_edit` / `full_auto` / `bypass`), `cli_permission_mode` (the CLI's own name for it), `cli: {name, version}`, `model` (when the CLI says), `mcp_servers` (optional: each MCP server the CLI was given, `{name, state, error}`, `state` one of `connected` / `pending` / `failed` / `needs_auth` / `disabled`) |
| `mcp_status` | `servers`: MCP servers whose state changed since last reported (same shape); unlisted servers are unchanged. An adapter whose CLI reports servers one at a time (Codex `mcpServer/startupStatus/updated`) sends one per change |
| `agent_text` | `text` |
| `tool_call_started` | `call_id`, `tool`, `input` (any JSON) |
| `tool_call_finished` | `call_id`, `ok`, `output` |
| `permission_request` | `request_id`, `request` (below), `reason` |
| `turn_complete` | (none): the agent awaits the next prompt |
| `done` | `summary`: the agent declared its work complete |
| `error` | `message`, `fatal` |
| `token_usage` | `model` (the Model these tokens were spent on; absent or `null` means the Session's Model, as in 0.1), `input_tokens` (excluding cache reads and writes), `output_tokens`, `cache_read_tokens`, `cache_write_tokens` (cache creation; optional, 0 when absent), `cost_usd` (the CLI's cost for these tokens, only when it reports one): running totals for everything the session has spent on that Model, each report superseding the last for the same Model. A CLI that uses several Models (its own subagent or background calls, a switch) sends one per Model, never one summed total |
| `classifier_denial` | `tool`, `detail`, `reason`: never a Permission request |
| `model_changed` | `model`: the main agent's own requests now run on this Model, a different one from its last request (a `set_model` took, or the CLI moved on its own). Never sent for a subagent's, a compaction's or another background call's Model, whose usage still comes as `token_usage` on that Model. An adapter that cannot tell the main agent's requests apart never sends it |
| `model_change_failed` | `model` (the Model `set_model` asked for), `reason` (the CLI's, when it gives one): the switch did not take, because the CLI refused it or the main agent's next request was still on the old Model. The CLI carries on as it was |

A `permission_request`'s `request` is tagged by `kind`, one of exactly five:

| `kind` | Fields |
| --- | --- |
| `command` | `command`, `cwd` |
| `file_write` | `path`, `diff` |
| `network` | `domain`, `url` |
| `mcp_tool` | `server`, `tool`, `input` |
| `question` | `question`, `options` |

### Commands (host to adapter)

| `type` | Fields |
| --- | --- |
| `send_text` | `text` |
| `answer_request` | `request_id`, `verdict` (below) |
| `interrupt` | (none) |
| `set_permission_mode` | `permission_mode` (switch the live CLI to another mode; added in 0.1 without a version bump) |
| `resume` | `external_session_id` |
| `set_model` | `model` (a Model id): switch the live CLI to this Model from the main agent's next request on, leaving the request in flight alone. Answered by `model_changed` or `model_change_failed`. Only sent to an adapter whose manifest has `switch_model_live` |

`resume` is only ever the first command a freshly started adapter receives: the host relaunched it for a Session whose process is gone (after a crash or quit) and the CLI must reload that conversation by its own session id (from `session_init`) so the agent keeps its context. A `send_text` follows it.

A `verdict` is tagged by `decision`: `allow_once` (Run once), `allow_always` (Grant), `deny` with `message`, or `answer` with `text` (for `question` requests).

## Adapters

```rust
pub trait Adapter {
    fn manifest(&self) -> AdapterManifest;
    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError>;
    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError>;
    /// Raw messages to write to the CLI on the adapter's own account (default: none).
    fn replies(&mut self) -> Vec<String> { Vec::new() }
}
```

`replies` exists for JSON-RPC CLIs, where the adapter must answer the CLI rather than the host: send `initialized` and start a thread once `initialize` returns, start the queued turn once the thread exists, answer a server request it cannot handle with an error. Hosts call it after every `ingest` and write what it returns, in order.

An adapter is a translator and never touches a process. The host feeds it the CLI's raw output one message at a time (for JSON-lines and JSON-RPC CLIs, one line) and writes the raw messages it returns back to the CLI. That keeps adapters deterministic under recordings, and lets an out-of-process adapter (a Node sidecar, a bridge to `codex app-server`) implement the trait over its own transport.

`echo::EchoAdapter` is the smallest example: its imaginary CLI writes `{"echo": <event>}` and reads `{"echo": <command>}`; any other message yields no events.

## Recorded fixtures

A fixture is a directory, named for the scenario, under `fixtures/<adapter>/`:

```
fixtures/
  echo/
    basic-session/
      meta.json
      input.jsonl
      expected-events.jsonl
      commands.jsonl        (optional)
      writes.jsonl          (optional)
```

### `meta.json`

```json
{
  "adapter": "echo",
  "cli": { "name": "echo-cli", "version": "1.0.0" },
  "protocol": "0.2.0",
  "description": "What this recording covers."
}
```

`cli.version` is the exact CLI version the recording was made against; a new CLI version means a new recording (and an updated compatibility range), never an edited one. `protocol` is the protocol version `expected-events.jsonl` and `commands.jsonl` are written in.

### `input.jsonl`: the raw input stream

The CLI's raw output, verbatim, one message per line, in the order the CLI produced it. Each line is passed to `Adapter::ingest` as-is. Blank lines are ignored. Lines are not required to be JSON, but for structured CLIs they are.

### `expected-events.jsonl`: expected events

The protocol events the adapter must emit over the whole run, one per line, in order. Events are compared as values (field order and whitespace do not matter).

### `commands.jsonl`: verdicts and expected responses

Commands the host sends during the run, each with the exact raw messages the CLI must receive in response. This is where every verdict lives: an `answer_request` command plus the CLI-side reply it must translate into.

```json
{"after_input": 7, "command": {"type": "answer_request", "request_id": "r2", "verdict": {"decision": "deny", "message": "Not that file"}}, "expect_responses": [{"jsonrpc": "2.0", "id": 12, "result": {"decision": "decline"}}]}
```

- `after_input`: send the command once this many lines of `input.jsonl` have been ingested (`0` means before the first line; at most the number of input lines, meaning after the last). Lines must be in non-decreasing `after_input` order; commands with the same value are sent in file order.
- `command`: a protocol command.
- `expect_responses`: the raw messages the adapter must return, in order. A raw message that parses as JSON is compared as JSON; anything else is compared as a JSON string. `[]` asserts the adapter writes nothing.

### `writes.jsonl`: what the adapter writes on its own

Every raw message the adapter returns from `replies`, in order, each with the input line whose ingestion produced it (1-based). A missing file means the adapter must write nothing on its own.

```json
{"after_input": 1, "message": {"method": "initialized"}}
```

Fixtures may carry other files the adapter's own test reads (the Codex adapter's `launch.json`: the working directory, permission mode and config it is started with).

## Running the conformance suite

```sh
cargo test -p adapter-protocol
```

The runner is a library function, so each adapter crate runs its own fixtures from a test:

```rust
use adapter_protocol::conformance::{run, Fixture};

for fixture in Fixture::load_all("fixtures/my-cli")? {
    run(&mut MyAdapter::new(), &fixture).unwrap_or_else(|f| panic!("{}:\n{f}", fixture.name));
}
```

`run` checks the adapter's range and the fixture's protocol version first, then interleaves input lines and commands, and finally compares. On failure it reports every mismatch as a unified diff, one compact JSON value per line, with three lines of context:

```
events differ:
--- expected
+++ actual
@@ -1,5 +1,5 @@
 {"type":"session_init","external_session_id":"echo-sess-1",...}
-{"type":"agent_text","text":"Looking at the tests."}
+{"type":"agent_text","text":"Looking at the build."}
 {"type":"tool_call_started","call_id":"t1","tool":"Read","input":{"path":"Cargo.toml"}}
 ...
responses to command #3 {"type":"answer_request","request_id":"r2",...} differ:
--- expected
+++ actual
...
```
