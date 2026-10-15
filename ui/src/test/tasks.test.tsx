import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Repository } from "../schema/Repository";
import type { Session } from "../schema/Session";
import type { Task } from "../schema/Task";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme: Repository = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main", remote: "origin" };
const repoEvents: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/login", branch: "feat/login", primary: false } },
];
const task = (change: Partial<Task> = {}): Task => ({
  id: "repo-1/local/L-1",
  repositoryId: "repo-1",
  source: "local",
  key: "L-1",
  title: "Cursor pagination on orders",
  description: "Offset pagination times out past page 400.",
  kind: "feat",
  column: "todo",
  checklist: [],
  url: null,
  ...change,
});
const board = (tasks: Task[]): Event => ({
  type: "tasksChanged",
  repositoryId: "repo-1",
  tasks,
  houseRules: "Write a failing test first.",
});
const running: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/login",
  branch: "feat/login",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "running",
  startedAtMs: 0,
};

function launch(events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

const openTasks = () => fireEvent.click(screen.getByRole("button", { name: "Tasks" }));
const column = (name: string) => screen.getByRole("region", { name });
const card = (key: string) => screen.getByRole("article", { name: `Task ${key}` });
const toasts = () => Array.from(document.querySelectorAll(".toast .tx")).map((t) => t.textContent ?? "");
const openSession = () =>
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByText("feat/login"));
const openContext = () => fireEvent.click(screen.getByRole("tab", { name: "Context" }));

test("the Tasks view reads every repository's tasks into four columns; Sync is inert", () => {
  const { sent, emit } = launch([]);
  openTasks();
  expect(sent).toContainEqual({ type: "readTasks", repositoryId: "repo-1" });
  act(() =>
    emit(
      board([
        task(),
        task({ id: "repo-1/local/L-2", key: "L-2", title: "Rate limits", column: "inProgress" }),
        task({ id: "repo-1/local/L-3", key: "L-3", title: "Moment", column: "inReview" }),
        task({ id: "repo-1/local/L-4", key: "L-4", title: "Token bucket", column: "done" }),
      ]),
    ),
  );
  expect(within(column("To do")).getByText("Cursor pagination on orders")).toBeTruthy();
  expect(within(column("In progress")).getByText("Rate limits")).toBeTruthy();
  expect(within(column("In review")).getByText("Moment")).toBeTruthy();
  expect(within(column("Done")).getByText("Token bucket")).toBeTruthy();
  expect(column("To do").textContent).toContain("1");
  const sync = screen.getByRole("button", { name: "Sync" }) as HTMLButtonElement;
  expect(sync.disabled).toBe(true);
});

test("New local task sends the title, kind, description and checklist lines", () => {
  const { sent, emit } = launch([board([])]);
  openTasks();
  fireEvent.click(screen.getByRole("button", { name: "New local task" }));
  const dialog = screen.getByRole("dialog", { name: "New local task" });
  fireEvent.change(within(dialog).getByLabelText("Title"), { target: { value: "Fix the flaky date test" } });
  fireEvent.change(within(dialog).getByLabelText("Kind"), { target: { value: "fix" } });
  fireEvent.change(within(dialog).getByLabelText("Description"), { target: { value: "Fails 1 in 8." } });
  fireEvent.change(within(dialog).getByLabelText("Checklist"), { target: { value: "Reproduce\n\n Root cause \n" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Add task" }));
  expect(sent).toContainEqual({
    type: "createTask",
    repositoryId: "repo-1",
    title: "Fix the flaky date test",
    description: "Fails 1 in 8.",
    kind: "fix",
    checklist: ["Reproduce", "Root cause"],
  });
  expect(screen.queryByRole("dialog", { name: "New local task" })).toBeNull();
  act(() => emit({ type: "taskCreated", task: task({ title: "Fix the flaky date test" }) }));
  expect(toasts()).toContain("Added L-1 to To do");
});

test("Assign to agent prefills New session from the task and names the branch after it", () => {
  const { sent } = launch([board([task()])]);
  openTasks();
  fireEvent.click(within(card("L-1")).getByRole("button", { name: "Assign to agent" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  const prompt = (within(dialog).getByLabelText("Prompt") as HTMLTextAreaElement).value;
  expect(prompt).toContain("L-1: Cursor pagination on orders");
  expect(prompt).toContain("Offset pagination times out past page 400.");
  expect(prompt).toContain("Write a failing test first.");
  expect((within(dialog).getByLabelText("Branch") as HTMLInputElement).value).toBe("feat/l1-cursor-pagination-orders");
  expect(dialog.textContent).toContain("L-1 · Cursor pagination on orders");

  fireEvent.click(within(dialog).getByRole("button", { name: "Start session" }));
  expect(sent).toContainEqual({
    type: "startTaskSession",
    taskId: "repo-1/local/L-1",
    target: { type: "newWorktree", branch: "feat/l1-cursor-pagination-orders" },
    agent: "claudeCode",
    permissionMode: "askFirst",
    prompt,
  });
});

test("a linked task's card shows its session; a done task offers nothing to assign", () => {
  launch([
    { type: "sessionStarted", session: running },
    { type: "taskLinked", sessionId: "session-1", taskId: "repo-1/local/L-1" },
    board([task({ column: "inProgress" }), task({ id: "repo-1/local/L-2", key: "L-2", column: "done" })]),
  ]);
  openTasks();
  expect(card("L-1").textContent).toContain("feat/login");
  expect(within(card("L-1")).queryByRole("button", { name: "Assign to agent" })).toBeNull();
  fireEvent.click(within(card("L-1")).getByRole("button", { name: "Open" }));
  expect(screen.getByRole("region", { name: "Session feat/login" })).toBeTruthy();

  openTasks();
  expect(within(card("L-2")).queryByRole("button", { name: "Assign to agent" })).toBeNull();
});

test("Context shows the linked task with a checklist ticked locally", () => {
  const { sent } = launch([
    { type: "sessionStarted", session: running },
    { type: "taskLinked", sessionId: "session-1", taskId: "repo-1/local/L-1" },
    board([
      task({
        column: "inProgress",
        checklist: [
          { text: "Cursor param", done: true },
          { text: "OpenAPI", done: false },
        ],
      }),
    ]),
  ]);
  openSession();
  openContext();
  const section = screen.getByRole("region", { name: "Task" });
  expect(section.textContent).toContain("L-1");
  expect(section.textContent).toContain("In progress");
  expect(section.textContent).toContain("Cursor pagination on orders");
  const done = within(section).getByRole("checkbox", { name: "Cursor param" }) as HTMLInputElement;
  expect(done.checked).toBe(true);
  fireEvent.click(within(section).getByRole("checkbox", { name: "OpenAPI" }));
  expect(sent).toContainEqual({ type: "tickTask", taskId: "repo-1/local/L-1", item: 1, done: true });
});

test("Pick one opens Tasks to link a task to the session; changing it is an explicit relink", () => {
  const { sent, emit } = launch([{ type: "sessionStarted", session: running }, board([task()])]);
  openSession();
  openContext();
  const section = screen.getByRole("region", { name: "Task" });
  expect(section.textContent).toContain("No task linked.");
  fireEvent.click(within(section).getByRole("button", { name: "Pick one" }));

  expect(screen.getByRole("heading", { name: "Tasks" })).toBeTruthy();
  expect(screen.getByRole("status", { name: "Linking" }).textContent).toContain("feat/login");
  fireEvent.click(within(card("L-1")).getByRole("button", { name: "Link to feat/login" }));
  expect(sent).toContainEqual({ type: "linkTask", sessionId: "session-1", taskId: "repo-1/local/L-1", relink: false });
  // Back on the session once linked.
  act(() => emit({ type: "taskLinked", sessionId: "session-1", taskId: "repo-1/local/L-1" }));
  expect(screen.getByRole("region", { name: "Session feat/login" })).toBeTruthy();

  openContext();
  fireEvent.click(within(screen.getByRole("region", { name: "Task" })).getByRole("button", { name: "Change" }));
  fireEvent.click(within(card("L-1")).getByRole("button", { name: "Link to feat/login" }));
  expect(sent.filter((c) => c.type === "linkTask").pop()).toEqual({
    type: "linkTask",
    sessionId: "session-1",
    taskId: "repo-1/local/L-1",
    relink: true,
  });
});
