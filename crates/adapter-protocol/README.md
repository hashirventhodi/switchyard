# adapter-protocol

The Switchyard adapter protocol, version **0.1.0**, and its conformance runner.

Switchyard drives agent CLIs (Claude Code, Codex, ...) only through their structured modes, never by scraping a terminal (ADR-0002). An **adapter** maps one CLI's structured output into this protocol's **events**, and maps this protocol's **commands** into that CLI's input. A **recorded fixture** pins that mapping for a specific CLI version, and the **conformance runner** checks an adapter against it.

This crate is self-contained (no dependency on the rest of Switchyard) so it can be published on its own.

## Versioning

- `PROTOCOL_VERSION` is the version this crate defines. It follows semver: while it is `0.x`, a minor bump is breaking.
- Every adapter declares the range it implements in its `AdapterManifest` (`implements`, a semver requirement such as `^0.1`). Hosts and the runner refuse an adapter whose range does not include `PROTOCOL_VERSION`.
- Every fixture records the protocol version its expected events are written in (`meta.json` `protocol`). The runner refuses a fixture whose version is not caret-compatible with its own (for example a `0.2.0` fixture under a `0.1.0` runner).

## Wire format

Events and commands are JSON objects, internally tagged by `type`, with snake_case names and fields. Optional fields are written as `null` (they are always present on output, and may be omitted on input). Hosts and out-of-process adapters exchange them in this form; the Rust types in `src/protocol.rs` are the reference.

### Events (adapter to host)

| `type` | Fields |
| --- | --- |
| `session_init` | `external_session_id`, `permission_mode` (the mode the CLI actually started in: `explore` / `ask_first` / `auto_edit` / `full_auto` / `bypass`), `cli_permission_mode` (the CLI's own name for it), `cli: {name, version}`, `model` (when the CLI says) |
| `agent_text` | `text` |
| `tool_call_started` | `call_id`, `tool`, `input` (any JSON) |
| `tool_call_finished` | `call_id`, `ok`, `output` |
| `permission_request` | `request_id`, `request` (below), `reason` |
| `turn_complete` | (none): the agent awaits the next prompt |
| `done` | `summary`: the agent declared its work complete |
| `error` | `message`, `fatal` |
| `token_usage` | `input_tokens` (excluding cache reads), `output_tokens`, `cache_read_tokens`, `cost_usd` (only when the CLI reports one): running totals for the whole session, each report superseding the last |
| `classifier_denial` | `tool`, `detail`, `reason`: never a Permission request |

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

A `verdict` is tagged by `decision`: `allow_once` (Run once), `allow_always` (Grant), `deny` with `message`, or `answer` with `text` (for `question` requests).

## Adapters

```rust
pub trait Adapter {
    fn manifest(&self) -> AdapterManifest;
    fn ingest(&mut self, raw: &str) -> Result<Vec<Event>, AdapterError>;
    fn command(&mut self, command: &Command) -> Result<Vec<String>, AdapterError>;
}
```

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
```

### `meta.json`

```json
{
  "adapter": "echo",
  "cli": { "name": "echo-cli", "version": "1.0.0" },
  "protocol": "0.1.0",
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
