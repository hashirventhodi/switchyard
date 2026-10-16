// Grants in the renderer, driven through a fake core: "Always allow <pattern> in this repo"
// on a request card, and the session's Context listing every Grant that applies with its
// layer, with Revoke on the user's local ones.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Entry } from "../schema/Entry";
import type { Event } from "../schema/Event";
import type { Policy } from "../schema/Policy";
import type { Request } from "../schema/Request";
import type { RequestState } from "../schema/RequestState";
import type { Session } from "../schema/Session";
import { suggestGrant } from "../grants";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const fix: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "running",
  startedAtMs: Date.now(),
};
const entry = (index: number, e: Entry): Event => ({ type: "transcriptEntry", sessionId: "session-1", index, entry: e });
const card = (request: Request, state: RequestState = "pending", answer: string | null = null): Entry => ({
  type: "request",
  requestId: "req-1",
  request,
  reason: null,
  state,
  answer,
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
      { type: "sessionStarted", session: fix },
      entry(0, { type: "user", text: "Run the tests" }),
      ...events,
    ),
  );
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return fake;
}

const requestCard = () => screen.getByRole("group", { name: /Permission request/ });

test.each<[string, string]>([
  ["pnpm test --filter api", "pnpm test *"],
  ["cargo test -p switchyard-core", "cargo test *"],
  ["ls", "ls *"],
  ["rm -rf dist", "rm *"],
  ["npx prettier --write .", "npx prettier --write ."],
  ["bash -c 'make'", "bash -c 'make'"],
  ["git status && git push", "git status && git push"],
  // Dangerous: the pattern names the class, or the guard would never let it answer.
  ["npm run db:migrate", "npm run db:migrate *"],
  ["git push origin main", "git push *"],
  ["pnpm exec curl https://example.com", "pnpm exec curl *"],
])("the suggested grant for %s is %s (the core's suggestion)", (command, pattern) => {
  expect(suggestGrant({ type: "command", command, cwd: null })).toBe(pattern);
});

test("Always allow on a command card grants the suggested pattern in this repo", () => {
  const { sent, emit } = launch(entry(1, card({ type: "command", command: "pnpm test --filter api", cwd: null })));

  const always = within(requestCard()).getByRole("button", { name: "Always allow pnpm test * in this repo" });
  expect(within(always).getByText("pnpm test *").tagName).toBe("CODE");
  fireEvent.click(always);

  expect(sent).toContainEqual({
    type: "answerRequest",
    sessionId: "session-1",
    requestId: "req-1",
    verdict: { type: "allowAlways", pattern: "pnpm test *" },
  });
  const granted = card({ type: "command", command: "pnpm test --filter api", cwd: null }, "alwaysAllowed", "pnpm test *");
  act(() => emit(entry(1, granted)));
  expect(requestCard().textContent).toContain("Always allowed in this repo: pnpm test *");
});

test.each<[string, Request, string]>([
  ["network", { type: "network", domain: "registry.npmjs.org", url: null }, "Always allow registry.npmjs.org in this repo"],
  ["MCP tool", { type: "mcpTool", server: "github", tool: "get_issue", input: "{}" }, "Always allow github · get_issue in this repo"],
])("a %s card offers Always allow for its domain or server and tool", (_, request, name) => {
  const { sent } = launch(entry(1, card(request)));

  fireEvent.click(within(requestCard()).getByRole("button", { name }));

  expect(sent).toContainEqual({ type: "answerRequest", sessionId: "session-1", requestId: "req-1", verdict: { type: "allowAlways", pattern: null } });
});

test("a file write never offers Always allow", () => {
  launch(entry(1, card({ type: "fileWrite", path: "README.md", diff: null })));

  expect(within(requestCard()).getAllByRole("button").map((b) => b.textContent)).toEqual(["Run once", "Skip"]);
});

const policy: Policy = {
  defaultMode: "autoEdit",
  ceiling: "autoEdit",
  error: null,
  request: null,
  grants: [
    { id: "repo-allow-1", layer: "repo", effect: "allow", rule: { type: "command", pattern: "pnpm *" } },
    { id: "repo-deny-1", layer: "repo", effect: "deny", rule: { type: "command", pattern: "git push *" } },
    { id: "grant-1", layer: "local", effect: "allow", rule: { type: "network", domain: "registry.npmjs.org" } },
    { id: "grant-2", layer: "local", effect: "allow", rule: { type: "mcpTool", server: "github", tool: "get_issue" } },
  ],
};

test("Context reads the repository's policy and lists every grant with its layer; only local ones revoke", () => {
  const { sent, emit } = launch();

  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  expect(sent).toContainEqual({ type: "readPolicy", repositoryId: "repo-1" });
  act(() => emit({ type: "policyChanged", repositoryId: "repo-1", policy }));

  const grants = screen.getByRole("region", { name: "Grants" });
  const rows = within(grants).getAllByRole("listitem");
  expect(rows.map((r) => r.textContent?.replace("Revoke", "").trim())).toEqual([
    "Allowpnpm *Repo file",
    "Denygit push *Repo file",
    "Allowregistry.npmjs.orgLocal",
    "Allowgithub · get_issueLocal",
  ]);
  expect(rows.map((r) => within(r).queryByRole("button", { name: /Revoke/ }) !== null)).toEqual([false, false, true, true]);
  expect(grants.textContent).toContain("Default mode Auto edit");
  expect(grants.textContent).toContain("Ceiling Auto edit");

  fireEvent.click(within(rows[2]).getByRole("button", { name: "Revoke registry.npmjs.org" }));
  expect(sent).toContainEqual({ type: "revokeGrant", repositoryId: "repo-1", grantId: "grant-1" });
});

test("Context says when the policy file is broken and nothing is granted", () => {
  const { emit } = launch();
  fireEvent.click(screen.getByRole("tab", { name: "Context" }));

  act(() =>
    emit({
      type: "policyChanged",
      repositoryId: "repo-1",
      policy: { defaultMode: null, ceiling: null, grants: [], error: ".switchyard/policy.yml is not valid: oops", request: null },
    }),
  );

  const grants = screen.getByRole("region", { name: "Grants" });
  expect(grants.textContent).toContain(".switchyard/policy.yml is not valid: oops");
  expect(grants.textContent).toContain("Nothing is granted until it is fixed");
});

test("a pending policy request is shown with what it asks for and acknowledged from Context", () => {
  const { sent, emit } = launch();
  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  act(() =>
    emit({
      type: "policyChanged",
      repositoryId: "repo-1",
      policy: {
        defaultMode: "askFirst",
        ceiling: null,
        grants: [],
        error: null,
        request: {
          hash: "abc123",
          allow: [{ id: "repo-allow-1", layer: "repo", effect: "allow", rule: { type: "command", pattern: "pnpm *" } }],
          sandbox: true,
          defaultMode: "fullAuto",
          worktreesRoot: "../trees",
          sandboxEvidence: null,
        },
      },
    }),
  );
  const request = screen.getByRole("region", { name: "Policy request" });
  expect(request.textContent).toContain("pnpm *");
  expect(request.textContent).toContain("Default mode Full auto");
  expect(request.textContent).toContain("../trees");
  expect(request.textContent).toContain("Switchyard cannot verify this sandbox");
  fireEvent.click(within(request).getByRole("button", { name: "Acknowledge" }));
  expect(sent).toContainEqual({ type: "acknowledgePolicy", repositoryId: "repo-1", hash: "abc123" });
});
