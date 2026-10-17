// @vitest-environment node
// A live Model switch in the sidecar: set_model reaches the live query's setModel, and a
// switch the CLI refuses comes back as model_change_failed with the CLI's reason.
import { expect, test } from "vitest";
import { drive, type QueryFn } from "../src/driver.ts";
import type { ProtocolEvent } from "../src/protocol.ts";

function fakeQuery(refuse?: string) {
  const calls = { options: undefined as any, models: [] as (string | undefined)[] };
  const query: QueryFn = ({ options }) => {
    calls.options = options;
    async function* run() {}
    return Object.assign(run(), {
      interrupt: async () => undefined,
      setModel: async (model?: string) => {
        calls.models.push(model);
        if (refuse) throw new Error(refuse);
      },
    }) as any;
  };
  return { query, calls };
}

function commands(...lines: object[]) {
  return (async function* () {
    for (const line of lines) yield JSON.stringify(line);
    // Keep stdin open a moment, as the host does, so the query is still live.
    await new Promise((r) => setTimeout(r, 20));
  })();
}

async function run(refuse: string | undefined, ...cmds: object[]) {
  const { query, calls } = fakeQuery(refuse);
  const events: ProtocolEvent[] = [];
  await drive({
    config: { cwd: "/wt", permission_mode: "ask_first", claude_path: null },
    commands: commands({ type: "send_text", text: "Ship it." }, ...cmds),
    emit: (e) => events.push(e),
    query,
  });
  await new Promise((r) => setTimeout(r, 30));
  return { calls, events };
}

test("set_model switches the live query's Model", async () => {
  const { calls, events } = await run(undefined, { type: "set_model", model: "claude-sonnet-5-5" });
  expect(calls.models).toEqual(["claude-sonnet-5-5"]);
  expect(events.filter((e) => e.type.startsWith("model"))).toEqual([]);
});

test("a switch the CLI refuses is model_change_failed with its reason", async () => {
  const { events } = await run("Model 'nope' not found", { type: "set_model", model: "nope" });
  expect(events.filter((e) => e.type.startsWith("model"))).toEqual([{ type: "model_change_failed", model: "nope", reason: "Model 'nope' not found" }]);
});
