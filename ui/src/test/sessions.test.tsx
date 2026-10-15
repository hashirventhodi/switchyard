import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, onTestFinished, test, vi } from "vitest";
import { App } from "../App";
import type { Entry } from "../schema/Entry";
import type { Event } from "../schema/Event";
import type { ToolState } from "../schema/ToolState";
import type { Session } from "../schema/Session";
import type { SessionStatus } from "../schema/SessionStatus";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/feat", branch: "feat/login", primary: false } },
];
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: Date.now() - 65_000,
};

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

const status = (s: SessionStatus): Event => ({ type: "sessionStatusChanged", sessionId: "session-1", status: s });
const openSession = () => fireEvent.click(screen.getByRole("button", { name: /fix\/readme/ }));

test("New session can start Claude Code on an existing free worktree in Ask first with the prompt", () => {
  const { sent, emit } = launch();

  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  fireEvent.click(within(dialog).getByRole("radio", { name: /Use existing/ }));
  const worktree = within(dialog).getByLabelText("Worktree") as HTMLSelectElement;
  const options = [...worktree.options].map((o) => [o.textContent, o.disabled]);
  expect(options).toEqual([
    ["acme / main (primary: Explore only)", true],
    ["acme / fix/readme", false],
    ["acme / feat/login", false],
  ]);
  fireEvent.change(worktree, { target: { value: "/wt/acme/feat" } });
  expect((within(dialog).getByLabelText("Agent") as HTMLSelectElement).value).toBe("claudeCode");
  expect(within(dialog).getByRole("radio", { name: "Ask first" })).toHaveProperty("checked", true);
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "Add a login form" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Start session" }));

  expect(sent).toContainEqual({
    type: "startSession",
    worktreePath: "/wt/acme/feat",
    agent: "claudeCode",
    permissionMode: "askFirst",
    prompt: "Add a login form",
  });
  expect(screen.queryByRole("dialog", { name: "New session" })).toBeNull();

  // When the core confirms, the new session opens.
  act(() => emit({ type: "sessionStarted", session: { ...session, id: "session-2", worktreePath: "/wt/acme/feat", branch: "feat/login" } }));
  expect(screen.getByRole("region", { name: "Session feat/login" })).toBeTruthy();
});

test("a worktree occupied by a session is not offered for a new one", () => {
  launch({ type: "sessionStarted", session });

  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  fireEvent.click(screen.getByRole("radio", { name: /Use existing/ }));

  const worktree = screen.getByLabelText("Worktree") as HTMLSelectElement;
  const fix = [...worktree.options].find((o) => o.value === "/wt/acme/fix")!;
  expect(fix.disabled).toBe(true);
  expect(fix.textContent).toBe("acme / fix/readme (Running)");
});

const LABELS: [SessionStatus, string][] = [
  ["running", "Running"],
  ["waiting", "Needs you"],
  ["idle", "Paused"],
  ["done", "Finished"],
  ["failed", "Error"],
];

test.each(LABELS)("status %s looks the same on the pill, the sidebar row and the dock", (s, label) => {
  launch({ type: "sessionStarted", session }, status(s));
  openSession();

  const pill = within(screen.getByRole("region", { name: "Session fix/readme" })).getByTestId("status");
  const row = within(screen.getByRole("complementary", { name: "Repositories" })).getByTestId("status");
  const dock = within(screen.getByRole("region", { name: "Terminals" })).getByTestId("status");
  for (const surface of [pill, row, dock]) {
    expect(surface.textContent).toBe(label);
    expect(surface.dataset.status).toBe(s);
    expect(surface.className).toContain(`st-${s}`);
  }
});

test("a finished turn awaiting a prompt shows Needs you, never Paused", () => {
  launch({ type: "sessionStarted", session }, status("waiting"));
  openSession();

  const pill = within(screen.getByRole("region", { name: "Session fix/readme" })).getByTestId("status");
  expect(pill.textContent).toBe("Needs you");
  expect(screen.queryByText("Paused")).toBeNull();
});

const entry = (index: number, e: Entry): Event => ({ type: "transcriptEntry", sessionId: "session-1", index, entry: e });
const tool = (callId: string, name: string, summary: string, state: ToolState, output: string | null): Entry => ({
  type: "tool",
  callId,
  tool: name,
  summary,
  state,
  output,
});

test("the transcript shows prose and tool rows: shell runs and failures expanded, others collapsed", () => {
  launch(
    { type: "sessionStarted", session },
    entry(0, { type: "user", text: "Fix the broken link" }),
    entry(1, { type: "agent", text: "Looking at the README." }),
    entry(2, tool("t1", "Bash", "ls", "running", null)),
    entry(3, tool("t2", "Read", "README.md", "running", null)),
    entry(2, tool("t1", "Bash", "ls", "ok", "README.md\nsrc")),
    entry(3, tool("t2", "Read", "README.md", "ok", "# acme")),
    entry(4, tool("t3", "Read", "docs/missing.md", "failed", "File does not exist.")),
    entry(5, { type: "system", text: "Stopped by you." }),
  );
  openSession();

  const log = screen.getByRole("log", { name: "Transcript" });
  expect(within(log).getByText("Fix the broken link")).toBeTruthy();
  expect(within(log).getByText("Looking at the README.")).toBeTruthy();
  const rows = within(log).getAllByRole("button");
  expect(rows.map((r) => [r.textContent, r.getAttribute("aria-expanded")])).toEqual([
    ["Bashlsok", "true"],
    ["ReadREADME.mdok", "false"],
    ["Readdocs/missing.mdfailed", "true"],
  ]);
  expect(within(log).getByText("README.md\nsrc", { normalizer: (s) => s })).toBeTruthy();
  expect(within(log).queryByText("# acme")).toBeNull();
  expect(within(log).getByText("File does not exist.")).toBeTruthy();

  fireEvent.click(rows[1]);
  expect(within(log).getByText("# acme")).toBeTruthy();
  expect(within(log).getByText("Stopped by you.")).toBeTruthy();
});

test("the summary strip shows agent, model, CLI version, path, mode, turns, cost and elapsed", () => {
  // A fixed clock: elapsed must not depend on how long the run took to get here.
  vi.useFakeTimers({ now: session.startedAtMs + 65_000 });
  onTestFinished(() => {
    vi.useRealTimers();
  });
  launch(
    { type: "sessionStarted", session },
    {
      type: "sessionFactsChanged",
      sessionId: "session-1",
      facts: { model: "claude-opus-5-5", cliVersion: "2.1.300", cliPermissionMode: "default", turns: 2 },
    },
  );
  openSession();

  const cells = within(screen.getByLabelText("Summary"))
    .getAllByText(/./, { selector: ".l" })
    .map((l) => [l.textContent, l.nextElementSibling?.textContent]);
  expect(cells).toEqual([
    ["Agent", "Claude Code"],
    ["Model", "claude-opus-5-5"],
    ["CLI", "2.1.300"],
    ["Worktree", "/wt/acme/fix"],
    ["Mode", "Ask first (CLI: default)"],
    ["Turns", "2"],
    ["Cost", "—"],
    ["Elapsed", "1:05"],
  ]);
});

test("Stop and Esc stop a running session; Esc does nothing once it is paused", () => {
  const { sent, emit } = launch({ type: "sessionStarted", session });
  openSession();

  fireEvent.click(screen.getByRole("button", { name: /Stop/ }));
  expect(sent.filter((c) => c.type === "stopSession")).toHaveLength(1);

  act(() => {
    fireEvent.keyDown(document.body, { key: "Escape" });
  });
  expect(sent.filter((c) => c.type === "stopSession")).toEqual([
    { type: "stopSession", sessionId: "session-1" },
    { type: "stopSession", sessionId: "session-1" },
  ]);

  act(() => emit(status("idle")));
  expect(screen.queryByRole("button", { name: /Stop/ })).toBeNull();
  act(() => {
    fireEvent.keyDown(document.body, { key: "Escape" });
  });
  expect(sent.filter((c) => c.type === "stopSession")).toHaveLength(2);
});

test("Esc with the New session modal open closes it and does not stop the agent", () => {
  const { sent } = launch({ type: "sessionStarted", session });
  openSession();
  fireEvent.click(screen.getByRole("button", { name: "New session" }));

  act(() => {
    fireEvent.keyDown(document.body, { key: "Escape" });
  });

  expect(screen.queryByRole("dialog", { name: "New session" })).toBeNull();
  expect(sent.filter((c) => c.type === "stopSession")).toEqual([]);
});

test("the composer sends the next prompt with Enter when the session needs you", () => {
  const { sent } = launch({ type: "sessionStarted", session }, status("waiting"));
  openSession();
  const composer = screen.getByLabelText("Message the agent");

  fireEvent.change(composer, { target: { value: "Also check the CHANGELOG." } });
  fireEvent.keyDown(composer, { key: "Enter" });

  expect(sent).toContainEqual({ type: "sendPrompt", sessionId: "session-1", text: "Also check the CHANGELOG." });
  expect((composer as HTMLTextAreaElement).value).toBe("");
});

test("the transcript stays responsive at 20k events: only a window of rows is in the DOM", () => {
  const { emit } = launch({ type: "sessionStarted", session });
  openSession();
  const events: Event[] = [];
  for (let i = 0; i < 20_000; i++)
    events.push(
      i % 2
        ? entry(i, { type: "agent", text: `Step ${i}` })
        : entry(i, tool(`t${i}`, "Read", `src/file${i}.ts`, "ok", "contents")),
    );

  const started = performance.now();
  act(() => emit(...events));
  const elapsed = performance.now() - started;

  const log = screen.getByRole("log", { name: "Transcript" });
  const rendered = log.querySelectorAll("[data-entry]").length;
  expect(rendered).toBeGreaterThan(0);
  expect(rendered).toBeLessThan(200);
  // Generous bound for slow CI; a quadratic reducer or an unvirtualised list takes far longer.
  expect(elapsed).toBeLessThan(3000);

  // Streaming one more event re-renders a window, not the whole list.
  act(() => emit(entry(20_000, { type: "agent", text: "Done." })));
  expect(log.querySelectorAll("[data-entry]").length).toBeLessThan(200);
});
