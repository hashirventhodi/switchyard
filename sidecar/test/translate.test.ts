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
