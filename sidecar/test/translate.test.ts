// @vitest-environment node
// The translator's request kinds beyond the recorded fixtures: every canUseTool call becomes
// one of the five Permission request kinds, and each verdict the exact SDK answer.
import { expect, test } from "vitest";
import { ClaudeTranslator, DECLINED } from "../src/translate.ts";

const ask = (t: ClaudeTranslator, tool: string, input: object, extra: object = {}) =>
  t.ingest(
    JSON.stringify({ type: "switchyard", subtype: "can_use_tool", request_id: "r", tool_name: tool, input, tool_use_id: "u", ...extra }),
  )[0];

test.each([
  ["Write", { file_path: "/wt/a.txt", content: "one\ntwo" }, { kind: "file_write", path: "/wt/a.txt", diff: "+one\n+two" }],
  ["Edit", { file_path: "/wt/a.txt", old_string: "a", new_string: "b" }, { kind: "file_write", path: "/wt/a.txt", diff: "-a\n+b" }],
  [
    "WebFetch",
    { url: "https://docs.example.com/page", prompt: "read" },
    { kind: "network", domain: "docs.example.com", url: "https://docs.example.com/page" },
  ],
  ["mcp__github__create_issue", { title: "x" }, { kind: "mcp_tool", server: "github", tool: "create_issue", input: { title: "x" } }],
  ["Read", { file_path: "/etc/hosts" }, { kind: "command", command: "Read(/etc/hosts)", cwd: null }],
])("%s asks as its kind", (tool, input, request) => {
  expect(ask(new ClaudeTranslator(), tool, input)).toEqual({ type: "permission_request", request_id: "r", request, reason: null });
});

test("a skip with the user's words passes them to the agent", () => {
  const t = new ClaudeTranslator();
  ask(t, "Bash", { command: "git push" });
  expect(t.command({ type: "answer_request", request_id: "r", verdict: { decision: "deny", message: "not yet" } })).toEqual([
    { op: "resolve_permission", request_id: "r", result: { behavior: "deny", message: "not yet" } },
  ]);
  ask(t, "Bash", { command: "git push" });
  expect(t.command({ type: "answer_request", request_id: "r", verdict: { decision: "deny" } })[0]).toMatchObject({
    result: { behavior: "deny", message: DECLINED },
  });
});

test("several questions are asked as one and answered as a free-form response", () => {
  const t = new ClaudeTranslator();
  const input = {
    questions: [
      { question: "Which runner?", header: "a", options: [{ label: "vitest" }, { label: "jest" }], multiSelect: false },
      { question: "Watch mode?", header: "b", options: [{ label: "yes" }, { label: "no" }], multiSelect: false },
    ],
  };
  expect(ask(t, "AskUserQuestion", input)).toMatchObject({
    request: { kind: "question", question: "Which runner?\nWatch mode?", options: [] },
  });
  expect(t.command({ type: "answer_request", request_id: "r", verdict: { decision: "answer", text: "vitest, no" } })).toEqual([
    { op: "resolve_permission", request_id: "r", result: { behavior: "allow", updatedInput: { ...input, response: "vitest, no" } } },
  ]);
});

test("answering an unknown request is an adapter error", () => {
  expect(() => new ClaudeTranslator().command({ type: "answer_request", request_id: "nope", verdict: { decision: "allow_once" } })).toThrow(
    "no pending request nope",
  );
});

test("resume by the CLI's session id becomes a resume op", () => {
  expect(new ClaudeTranslator().command({ type: "resume", external_session_id: "sess-9" })).toEqual([
    { op: "resume", session_id: "sess-9" },
  ]);
});

test("a result reports each Model's usage separately, with that Model's own cost", () => {
  const t = new ClaudeTranslator();
  const result = {
    type: "result",
    subtype: "success",
    is_error: false,
    result: "ok",
    total_cost_usd: 0.05,
    modelUsage: {
      a: { inputTokens: 10, outputTokens: 20, cacheReadInputTokens: 300, cacheCreationInputTokens: 4000, costUSD: 0.04 },
      b: { inputTokens: 1, outputTokens: 2, cacheReadInputTokens: 3, cacheCreationInputTokens: 0, costUSD: 0.01 },
    },
  };
  expect(t.ingest(JSON.stringify(result)).slice(0, 2)).toEqual([
    { type: "token_usage", model: "a", input_tokens: 10, output_tokens: 20, cache_read_tokens: 300, cache_write_tokens: 4000, cost_usd: 0.04 },
    { type: "token_usage", model: "b", input_tokens: 1, output_tokens: 2, cache_read_tokens: 3, cost_usd: 0.01 },
  ]);
});

test("a lone Model without its own cost carries the result's total; one of several gets none", () => {
  const usage = (modelUsage: object) =>
    new ClaudeTranslator()
      .ingest(JSON.stringify({ type: "result", subtype: "success", is_error: false, total_cost_usd: 0.05, modelUsage }))
      .filter((e) => e.type === "token_usage")
      .map((e) => e.type === "token_usage" && [e.model, e.cost_usd]);
  expect(usage({ a: { inputTokens: 1 } })).toEqual([["a", 0.05]]);
  expect(usage({ a: { inputTokens: 1 }, b: { inputTokens: 1, costUSD: 0.01 } })).toEqual([
    ["a", null],
    ["b", 0.01],
  ]);
});

test("a switch back to the Model the main agent runs on withdraws the check", () => {
  const t = new ClaudeTranslator();
  const said = (id: string, model: string) =>
    t.ingest(JSON.stringify({ type: "assistant", message: { id, model, content: [] }, parent_tool_use_id: null }));
  said("msg_01", "claude-opus-5-5");
  t.command({ type: "set_model", model: "claude-sonnet-5-5" });
  t.command({ type: "set_model", model: "claude-opus-5-5" });
  expect(said("msg_02", "claude-opus-5-5")).toEqual([]);
});

test("the CLI's message for a failed request is never a Model change", () => {
  const t = new ClaudeTranslator();
  const said = (id: string, model: string, error?: string) =>
    t.ingest(
      JSON.stringify({
        type: "assistant",
        message: { id, model, content: [{ type: "text", text: `${id} said` }] },
        parent_tool_use_id: null,
        ...(error ? { error } : {}),
      }),
    );
  said("msg_01", "claude-opus-5-5");
  expect(said("msg_02", "<synthetic>", "rate_limit")).toEqual([{ type: "agent_text", text: "msg_02 said" }]);
  expect(said("msg_03", "claude-opus-5-5")).toEqual([{ type: "agent_text", text: "msg_03 said" }]);
});

test("a live switch to a Model the CLI then cannot find fails it with the CLI's message", () => {
  const t = new ClaudeTranslator();
  t.command({ type: "set_model", model: "claude-opus-4-1" });
  const failed = t.ingest(
    JSON.stringify({
      type: "assistant",
      message: { id: "msg_01", model: "<synthetic>", content: [{ type: "text", text: "There's an issue with the selected model." }] },
      parent_tool_use_id: null,
      error: "model_not_found",
    }),
  );
  expect(failed).toEqual([{ type: "model_change_failed", model: "claude-opus-4-1", reason: "There's an issue with the selected model." }]);
});
