// The permission relay in the renderer, driven through a fake core: a request turns every
// surface amber from the core's events, and the toast, inbox, ⌘J and the session's request
// card all answer the same request with the same command.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import type { Entry } from "../schema/Entry";
import type { Event } from "../schema/Event";
import type { Request } from "../schema/Request";
import type { RequestState } from "../schema/RequestState";
import type { Session } from "../schema/Session";
import type { SessionStatus } from "../schema/SessionStatus";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/feat", branch: "feat/login", primary: false } },
];
const session = (id: string, branch: string, path: string): Session => ({
  id,
  repositoryId: "repo-1",
  worktreePath: path,
  branch,
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: Date.now(),
});
const fix = session("session-1", "fix/readme", "/wt/acme/fix");
const feat = session("session-2", "feat/login", "/wt/acme/feat");

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(...repoEvents, { type: "sessionStarted", session: fix }, entry(0, { type: "user", text: "Run the tests" }), ...events),
  );
  return fake;
}

const pnpmTest: Request = { type: "command", command: "pnpm test", cwd: null };
const distQuestion: Request = { type: "question", question: "Should dist/ stay in git?", options: ["Keep it", "Ignore it"] };
const card = (requestId: string, request: Request, state: RequestState = "pending", answer: string | null = null): Entry => ({
  type: "request",
  requestId,
  request,
  reason: "Run the test suite",
  state,
  answer,
});
const entry = (index: number, e: Entry, sessionId = "session-1"): Event => ({ type: "transcriptEntry", sessionId, index, entry: e });
const status = (s: SessionStatus, sessionId = "session-1"): Event => ({ type: "sessionStatusChanged", sessionId, status: s });
/** What the core emits when the agent asks: the card, then Needs you. */
const asks = (request: Request = pnpmTest, requestId = "req-1", sessionId = "session-1", index = 1): Event[] => [
  entry(index, card(requestId, request), sessionId),
  status("waiting", sessionId),
];
const answered = (requestId = "req-1", state: RequestState = "allowed", request: Request = pnpmTest, answer: string | null = null) => [
  entry(1, card(requestId, request, state, answer)),
  status("running"),
];

const toasts = () => screen.getByRole("status").querySelectorAll(".toast");
const openSession = () => fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
const requestCard = () => screen.getByRole("group", { name: /Approval needed|Question for you/ });
const currentView = () =>
  screen.getByRole("navigation", { name: "Views" }).querySelector("[aria-current=page]")?.getAttribute("aria-label");

/** Every surface that counts what needs the user. */
function surfaces() {
  return {
    inbox: within(screen.getByRole("banner")).getByRole("button", { name: /^Needs you/ }).textContent,
    kpi: screen.getByRole("button", { name: /^Needs you: \d+$/ }).querySelector(".v")?.textContent,
    statusbar: within(screen.getByRole("contentinfo")).getByText(/waiting/).textContent,
    sidebar: within(screen.getByRole("complementary", { name: "Repositories" })).getByTestId("status").dataset.status,
  };
}

test("a request turns every surface amber at once and raises one toast with agent, branch and command", () => {
  const { emit } = launch();

  act(() => emit(...asks()));

  const s = surfaces();
  expect([s.inbox, s.kpi, s.statusbar, s.sidebar]).toEqual(["Needs you1", "1", "1 waiting", "waiting"]);
  const needsYou = screen.getByRole("region", { name: "Needs you" });
  expect(within(needsYou).getByText("Wants to run: pnpm test")).toBeTruthy();
  expect(toasts()).toHaveLength(1);
  expect(toasts()[0].textContent).toContain("Claude Code on fix/readme wants to run");
  expect(toasts()[0].textContent).toContain("pnpm test");
});

test("Approve on the toast answers the request without changing view; the answer clears every surface", () => {
  const { sent, emit } = launch(...asks());

  fireEvent.click(within(toasts()[0] as HTMLElement).getByRole("button", { name: "Approve" }));

  expect(sent).toContainEqual({ type: "answerRequest", sessionId: "session-1", requestId: "req-1", verdict: { type: "allowOnce" } });
  expect(currentView()).toBe("Dashboard");
  act(() => emit(...answered()));
  const s = surfaces();
  expect([s.inbox, s.kpi, s.statusbar, s.sidebar]).toEqual(["Needs you", "0", "0 waiting", "running"]);
  expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
});

test("the request card in the session offers Run once, Always allow and Skip, then shows the verdict line", () => {
  const { sent, emit } = launch(...asks());
  openSession();

  const pending = requestCard();
  expect(pending.className).toContain("perm");
  expect(within(pending).getByText("pnpm test").tagName).toBe("CODE");
  expect(within(pending).getByText("Run the test suite")).toBeTruthy();
  expect(within(pending).getAllByRole("button").map((b) => b.textContent)).toEqual(["Run once", "Always allow pnpm test * in this repo", "Skip"]);
  fireEvent.click(within(pending).getByRole("button", { name: "Skip" }));
  expect(sent).toContainEqual({ type: "answerRequest", sessionId: "session-1", requestId: "req-1", verdict: { type: "deny" } });

  act(() => emit(...answered("req-1", "denied")));
  const resolved = requestCard();
  expect(resolved.className).toContain("resolved");
  expect(within(resolved).queryAllByRole("button")).toEqual([]);
  expect(resolved.textContent).toContain("Skipped. The agent continues without it.");
});

const always = (grant: string) => `Always allow ${grant} in this repo`;
test.each<[string, Request, string, string, string[]]>([
  ["command", pnpmTest, "Approval needed: command", "pnpm test", ["Run once", always("pnpm test *"), "Skip"]],
  ["file write", { type: "fileWrite", path: "README.md", diff: "+hello" }, "Approval needed: file write", "README.md", ["Run once", "Skip"]],
  ["network", { type: "network", domain: "docs.example.com", url: "https://docs.example.com/x" }, "Approval needed: network", "https://docs.example.com/x", ["Run once", always("docs.example.com"), "Skip"]],
  ["MCP tool", { type: "mcpTool", server: "github", tool: "create_issue", input: '{"title":"x"}' }, "Approval needed: MCP tool", "github · create_issue", ["Run once", always("github · create_issue"), "Skip"]],
])("a %s request renders its detail in a mono block with its answers", (_, request, heading, detail, buttons) => {
  launch(...asks(request));
  openSession();

  const pending = requestCard();
  expect(within(pending).getByText(heading)).toBeTruthy();
  expect(within(pending).getAllByText(detail)[0].tagName).toBe("CODE");
  expect(within(pending).getAllByRole("button").map((b) => b.textContent)).toEqual(buttons);
});

test("a question offers its options and Answer in chat; an option answers it", () => {
  const { sent } = launch(...asks(distQuestion));
  openSession();

  const pending = requestCard();
  expect(within(pending).getByText("Question for you")).toBeTruthy();
  expect(within(pending).getAllByRole("button").map((b) => b.textContent)).toEqual(["Keep it", "Ignore it", "Answer in chat"]);
  fireEvent.click(within(pending).getByRole("button", { name: "Ignore it" }));

  expect(sent).toContainEqual({
    type: "answerRequest",
    sessionId: "session-1",
    requestId: "req-1",
    verdict: { type: "answer", text: "Ignore it" },
  });
});

test("Answer in chat focuses the composer, and what is typed there answers the question", () => {
  const { sent, emit } = launch(...asks(distQuestion));
  openSession();

  fireEvent.click(within(requestCard()).getByRole("button", { name: "Answer in chat" }));
  const composer = screen.getByLabelText("Message the agent") as HTMLTextAreaElement;
  expect(document.activeElement).toBe(composer);
  fireEvent.change(composer, { target: { value: "Ignore it and add a note" } });
  fireEvent.keyDown(composer, { key: "Enter" });

  expect(sent).toContainEqual({ type: "sendPrompt", sessionId: "session-1", text: "Ignore it and add a note" });
  act(() => emit(...answered("req-1", "answered", distQuestion, "Ignore it and add a note")));
  expect(requestCard().textContent).toContain("You answered: Ignore it and add a note");
});

test("while a permission request is pending the composer waits for it", () => {
  launch(...asks());
  openSession();

  const composer = screen.getByLabelText("Message the agent") as HTMLTextAreaElement;
  expect(composer.disabled).toBe(true);
  expect(composer.placeholder).toBe("Answer the request above first.");
});

test("the inbox lists every blocked session with what it wants, and answers from there", () => {
  const { sent } = launch({ type: "sessionStarted", session: feat }, ...asks(), status("waiting", "session-2"));

  fireEvent.click(within(screen.getByRole("banner")).getByRole("button", { name: /^Needs you/ }));

  const inbox = screen.getByRole("dialog", { name: "Needs you" });
  const items = within(inbox).getAllByRole("listitem");
  expect(items.map((i) => within(i).getByText(/^Wants|^Waiting|^Asks/).textContent)).toEqual([
    "Wants to run: pnpm test",
    "Waiting for your next prompt",
  ]);
  fireEvent.click(within(items[0]).getByRole("button", { name: "Run once" }));
  expect(sent).toContainEqual({ type: "answerRequest", sessionId: "session-1", requestId: "req-1", verdict: { type: "allowOnce" } });

  fireEvent.click(within(items[1]).getByRole("button", { name: /feat\/login/ }));
  expect(screen.queryByRole("dialog", { name: "Needs you" })).toBeNull();
  expect(screen.getByRole("region", { name: "Session feat/login" })).toBeTruthy();
});

test("⌘J jumps to the next waiting session in turn, and says so when nothing waits", () => {
  const { emit } = launch({ type: "sessionStarted", session: feat }, ...asks(), ...asks(pnpmTest, "req-7", "session-2", 0));
  const cmdJ = () =>
    act(() => {
      fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    });

  cmdJ();
  expect(screen.getByRole("region", { name: "Session fix/readme" })).toBeTruthy();
  expect(requestCard()).toBeTruthy();
  cmdJ();
  expect(screen.getByRole("region", { name: "Session feat/login" })).toBeTruthy();
  cmdJ();
  expect(screen.getByRole("region", { name: "Session fix/readme" })).toBeTruthy();

  act(() => emit(status("running"), status("running", "session-2")));
  const before = toasts().length;
  cmdJ();
  expect(toasts()).toHaveLength(before + 1);
  expect(toasts()[before].textContent).toContain("Nothing is waiting on you.");
});

test("only requests toast (20 s) and results of the user's own actions (4 s); nothing else does", () => {
  vi.useFakeTimers();
  const { emit } = launch(
    status("waiting"),
    entry(1, { type: "agent", text: "Done for now." }),
    { type: "sessionFactsChanged", sessionId: "session-1", facts: { model: "m", cliVersion: "1", cliPermissionMode: "default", turns: 1, modeWarning: null, classifierDenials: 0 } },
    status("running"),
  );
  expect(toasts()).toHaveLength(0);

  act(() => emit(...asks(pnpmTest, "req-1", "session-1", 2)));
  act(() => emit({ type: "commandRefused", command: "sendPrompt", reason: "The session is paused; resume it first" }));
  expect(toasts()).toHaveLength(2);

  act(() => vi.advanceTimersByTime(4_000));
  expect([...toasts()].map((t) => t.textContent)).toEqual([expect.stringContaining("wants to run")]);
  act(() => vi.advanceTimersByTime(16_000));
  expect(toasts()).toHaveLength(0);
});
