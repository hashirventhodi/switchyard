import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Entry } from "../schema/Entry";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { CreateTerminal, TerminalHandle } from "../terminal";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
];
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: 0,
};

/** A terminal emulator stand-in: records what is written and lets the test type and resize. */
class FakeTerminal implements TerminalHandle {
  written = "";
  clears = 0;
  size = { cols: 80, rows: 24 };
  private data: ((d: string) => void)[] = [];
  private keys: ((e: KeyboardEvent) => boolean)[] = [];
  write(data: string) {
    this.written += data;
  }
  clear() {
    this.clears++;
  }
  fit() {
    return this.size;
  }
  onData(listener: (d: string) => void) {
    this.data.push(listener);
  }
  onKey(handler: (e: KeyboardEvent) => boolean) {
    this.keys.push(handler);
  }
  focus() {}
  dispose() {}
  /** The user types: the emulator first offers the key to the app, then sends its bytes. */
  type(key: KeyboardEventInit, bytes: string) {
    const e = new KeyboardEvent("keydown", key);
    if (this.keys.every((h) => h(e))) this.data.forEach((l) => l(bytes));
  }
}

let resize: () => void = () => {};
class FakeResizeObserver {
  constructor(callback: () => void) {
    resize = callback;
  }
  observe() {}
  unobserve() {}
  disconnect() {}
}

function launch(...events: Event[]) {
  const terminals: FakeTerminal[] = [];
  const createTerminal: CreateTerminal = () => {
    const t = new FakeTerminal();
    terminals.push(t);
    return t;
  };
  const fake = fakeCore();
  (globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver;
  render(<App core={fake.core} pickFolder={async () => null} createTerminal={createTerminal} />);
  act(() => fake.emit(...repoEvents, ...events));
  return { ...fake, terminals };
}

const dock = () => screen.getByRole("region", { name: "Terminals" });
const shellOpened: Event = { type: "shellOpened", shell: { id: "shell-1", worktreePath: "/wt/acme/fix" } };

test("+ shell opens a shell in the shown session's worktree at the terminal's size", () => {
  const { sent } = launch({ type: "sessionStarted", session });
  fireEvent.click(screen.getByRole("button", { name: /fix\/readme/ }));

  fireEvent.click(within(dock()).getByRole("button", { name: "+ shell" }));

  expect(sent).toContainEqual({ type: "openShell", worktreePath: "/wt/acme/fix", cols: 80, rows: 24 });
});

test("the palette offers a shell on every worktree", () => {
  const { sent } = launch();
  act(() => {
    fireEvent.keyDown(document.body, { key: "k", metaKey: true });
  });
  fireEvent.change(screen.getByRole("textbox", { name: /search/i }), { target: { value: "shell in fix" } });
  fireEvent.click(screen.getByRole("option", { name: /Shell in acme\/fix\/readme/ }));

  expect(sent.filter((c) => c.type === "openShell").map((c) => c.type === "openShell" && c.worktreePath)).toEqual([
    "/wt/acme/fix",
  ]);
});

test("a Worktrees card's Terminal button opens a shell on that worktree and shows the dock", () => {
  const { sent } = launch();
  fireEvent.click(screen.getByRole("button", { name: "Hide `" }));
  fireEvent.keyDown(document.body, { key: "3" });

  const card = screen.getByRole("article", { name: "Worktree fix/readme" });
  fireEvent.click(within(card).getByRole("button", { name: "Terminal" }));

  expect(sent).toContainEqual({ type: "openShell", worktreePath: "/wt/acme/fix", cols: 80, rows: 24 });
  expect(dock().className).not.toContain("collapsed");
});

test("a shell tab streams output, sends keystrokes, clears on Ctrl+L and follows the dock's size", () => {
  const { sent, emit, terminals } = launch();

  act(() => emit(shellOpened, { type: "shellOutput", shellId: "shell-1", data: "$ " }));
  const tab = within(dock()).getByRole("tab", { name: /fix\/readme/ });
  expect(tab.getAttribute("aria-selected")).toBe("true");
  const term = terminals[0];
  act(() => emit({ type: "shellOutput", shellId: "shell-1", data: "ls\r\nREADME\r\n$ " }));
  expect(term.written).toBe("$ ls\r\nREADME\r\n$ ");

  term.type({ key: "l" }, "l");
  term.type({ key: "s" }, "s");
  term.type({ key: "Enter" }, "\r");
  expect(sent.filter((c) => c.type === "writeShell")).toEqual(
    ["l", "s", "\r"].map((data) => ({ type: "writeShell", shellId: "shell-1", data })),
  );

  term.type({ key: "l", ctrlKey: true }, "\x0c");
  expect(term.clears).toBe(1);

  term.size = { cols: 120, rows: 30 };
  act(() => resize());
  expect(sent).toContainEqual({ type: "resizeShell", shellId: "shell-1", cols: 120, rows: 30 });
});

test("Esc inside a shell tab goes to the shell, not to the running agent", () => {
  const { sent } = launch({ type: "sessionStarted", session }, shellOpened);
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  const shell = within(dock()).getByLabelText("Shell /wt/acme/fix");

  fireEvent.keyDown(shell, { key: "Escape" });

  expect(sent.filter((c) => c.type === "stopSession")).toEqual([]);
});

test("output that arrives before the shell tab is drawn is not lost", () => {
  const { terminals } = launch(shellOpened, { type: "shellOutput", shellId: "shell-1", data: "welcome\r\n" });

  expect(terminals[0].written).toBe("welcome\r\n");
});

test("closing a shell tab closes the shell; a closed shell's tab goes away", () => {
  const { sent, emit } = launch(shellOpened);

  fireEvent.click(within(dock()).getByRole("button", { name: "Close shell fix/readme" }));
  expect(sent).toContainEqual({ type: "closeShell", shellId: "shell-1" });

  act(() => emit({ type: "shellClosed", shellId: "shell-1", exitCode: null }));
  expect(within(dock()).queryByRole("tab", { name: /fix\/readme/ })).toBeNull();
});

const entry = (index: number, e: Entry): Event => ({ type: "transcriptEntry", sessionId: "session-1", index, entry: e });

test("the Agent log shows the session's tool calls and output live, and cannot be typed into", () => {
  const { sent, emit } = launch(
    { type: "sessionStarted", session },
    entry(0, { type: "user", text: "Fix the link" }),
    entry(1, { type: "agent", text: "Looking." }),
    entry(2, { type: "tool", callId: "t1", tool: "Bash", summary: "ls", state: "running", output: null }),
  );

  const tab = within(dock()).getByRole("tab", { name: /fix\/readme/ });
  expect(within(tab).getByText("CC")).toBeTruthy();
  expect(within(tab).getByTestId("status").dataset.status).toBe("running");
  fireEvent.click(tab);
  const log = within(dock()).getByRole("log", { name: "Agent log fix/readme" });
  expect(log.textContent).toContain("$ ls");
  expect(log.textContent).not.toContain("Looking.");

  act(() =>
    emit(
      entry(2, { type: "tool", callId: "t1", tool: "Bash", summary: "ls", state: "ok", output: "README.md\nsrc" }),
      entry(3, { type: "tool", callId: "t2", tool: "Read", summary: "docs/missing.md", state: "failed", output: "File does not exist." }),
    ),
  );
  expect(log.textContent).toContain("README.md\nsrc");
  expect(log.textContent).toContain("Read docs/missing.md");
  expect(log.textContent).toContain("File does not exist.");

  const before = sent.length;
  fireEvent.keyDown(log, { key: "x" });
  fireEvent.keyPress(log, { key: "x" });
  expect(sent.length).toBe(before);
  expect(within(dock()).queryByRole("textbox")).toBeNull();
});

const raw = (index: number, json: string): Event => ({ type: "agentEventReceived", sessionId: "session-1", index, json });

test("the debug toggle shows raw adapter events, is off by default and survives view switches and reloads", () => {
  const events: Event[] = [
    { type: "sessionStarted", session },
    entry(0, { type: "tool", callId: "t1", tool: "Bash", summary: "ls", state: "ok", output: "src" }),
    raw(0, '{"type":"tool_call_started","call_id":"t1"}'),
    raw(1, '{"type":"tool_call_finished","call_id":"t1","ok":true}'),
  ];
  launch(...events);
  fireEvent.click(within(dock()).getByRole("tab", { name: /fix\/readme/ }));

  const toggle = within(dock()).getByRole("checkbox", { name: "Raw events" }) as HTMLInputElement;
  expect(toggle.checked).toBe(false);
  expect(dock().textContent).not.toContain("tool_call_started");

  fireEvent.click(toggle);
  const log = within(dock()).getByRole("log", { name: "Raw events fix/readme" });
  expect(log.textContent).toContain('{"type":"tool_call_started","call_id":"t1"}');
  expect(log.textContent).toContain('{"type":"tool_call_finished","call_id":"t1","ok":true}');

  fireEvent.keyDown(document.body, { key: "3" });
  fireEvent.keyDown(document.body, { key: "1" });
  expect(within(dock()).getByRole("log", { name: "Raw events fix/readme" })).toBeTruthy();

  cleanup();
  launch(...events);
  fireEvent.click(within(dock()).getByRole("tab", { name: /fix\/readme/ }));
  expect((within(dock()).getByRole("checkbox", { name: "Raw events" }) as HTMLInputElement).checked).toBe(true);
});
