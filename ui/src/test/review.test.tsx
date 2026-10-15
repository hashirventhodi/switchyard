import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import { createActionRegistry } from "../actions";
import type { ChangedFile } from "../schema/ChangedFile";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const WT = "/wt/acme/fix";
const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: WT,
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status: "waiting",
  startedAtMs: 0,
};
const base: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: WT, branch: "fix/readme", primary: false } },
  { type: "sessionStarted", session },
  { type: "transcriptEntry", sessionId: "session-1", index: 0, entry: { type: "user", text: "Fix the README typo\nand more" } },
];

function changed(path: string, status: ChangedFile["status"], extra: Partial<ChangedFile> = {}): ChangedFile {
  return { path, oldPath: null, status, added: 1, removed: 0, binary: false, decision: null, hunks: [], ...extra };
}

const readme = changed("README.md", "modified", {
  added: 1,
  removed: 1,
  hunks: [
    {
      id: "h1",
      header: "@@ -1,2 +1,2 @@ intro",
      lines: [
        { kind: "removed", old: 1, new: null, text: "teh readme" },
        { kind: "added", old: null, new: 1, text: "the readme" },
        { kind: "context", old: 2, new: 2, text: "end" },
      ],
    },
  ],
});
const files: ChangedFile[] = [
  changed("src/new.ts", "added", { added: 3 }),
  readme,
  changed("old.ts", "deleted", { added: 0, removed: 4 }),
  changed("merge.ts", "conflicted"),
];

function launch(...events: Event[]) {
  const fake = fakeCore();
  const actions = createActionRegistry();
  render(<App core={fake.core} pickFolder={async () => null} actions={actions} />);
  act(() => fake.emit(...base, ...events));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return { ...fake, actions };
}

const pane = () => screen.getByRole("complementary", { name: "Review" });
const fileRows = () =>
  within(pane())
    .getAllByRole("button", { name: /^File / })
    .map((b) => b.getAttribute("aria-label"));

test("opening a session reads its worktree's changes and groups them Conflicts / Modified / Added / Deleted", () => {
  const { sent, emit } = launch();
  expect(sent).toContainEqual({ type: "readChanges", worktreePath: WT });

  act(() => emit({ type: "changesRead", worktreePath: WT, files }));

  expect(within(pane()).getByRole("tab", { name: /Changes/ }).textContent).toContain("4");
  const groups = within(pane())
    .getAllByRole("group")
    .map((g) => g.getAttribute("aria-label"));
  expect(groups).toEqual(["Conflicts", "Modified", "Added", "Deleted"]);
  expect(fileRows()).toEqual([
    "File merge.ts, conflicted",
    "File README.md, modified",
    "File src/new.ts, added",
    "File old.ts, deleted",
  ]);
  const row = within(pane()).getByRole("button", { name: "File README.md, modified" });
  expect(row.textContent).toContain("M");
  expect(row.textContent).toContain("+1");
  expect(row.textContent).toContain("−1");
});

const key = (k: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(document.body, { key: k, ...init });
const current = () => within(pane()).getAllByRole("button", { name: /^File / }).find((b) => b.getAttribute("aria-current"));

test("selecting a file shows its hunks as old / new / sign / text lines under a header with the file's actions", () => {
  const { sent, emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files }));

  fireEvent.click(within(pane()).getByRole("button", { name: "File README.md, modified" }));

  const diff = within(pane()).getByLabelText("Diff of README.md");
  expect(diff.querySelector(".hh")?.textContent).toBe("@@ -1,2 +1,2 @@ intro");
  const lines = [...diff.querySelectorAll(".ln")].map((l) => [...l.children].map((c) => c.textContent));
  expect(lines).toEqual([
    ["1", "", "−", "teh readme"],
    ["", "1", "+", "the readme"],
    ["2", "2", " ", "end"],
  ]);
  fireEvent.click(within(diff).getByRole("button", { name: "Accept file" }));
  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "README.md", decision: "accept" });
  act(() => emit({ type: "fileDecided", worktreePath: WT, path: "README.md", decision: "accept" }));
  expect(within(pane()).getByRole("button", { name: "File README.md, modified" }).textContent).toContain("accepted");
  expect(within(diff).getByRole("button", { name: "Accept file" }).getAttribute("aria-pressed")).toBe("true");

  // Ask agent drafts a question in the composer.
  fireEvent.click(within(diff).getByRole("button", { name: "Ask agent" }));
  expect((screen.getByLabelText("Message the agent") as HTMLTextAreaElement).value).toContain("README.md");
});

test("a conflicted file offers no Accept or Reject and blocks Commit accepted", () => {
  const { emit } = launch();
  act(() =>
    emit({
      type: "changesRead",
      worktreePath: WT,
      files: [changed("merge.ts", "conflicted"), { ...readme, decision: "accept" }],
    }),
  );
  const diff = within(pane()).getByLabelText("Diff of merge.ts");
  expect(within(diff).queryByRole("button", { name: "Accept file" })).toBeNull();
  expect(within(pane()).getByRole("button", { name: "Commit accepted…" })).toHaveProperty("disabled", true);
  expect(within(pane()).getByText(/1 conflict/)).toBeTruthy();
});

test("files a merge of base would conflict on join the Conflicts group and block Commit accepted", () => {
  const { sent, emit } = launch();
  act(() =>
    emit(
      { type: "changesRead", worktreePath: WT, files: [{ ...readme, decision: "accept" }, changed("b.ts", "modified")] },
      { type: "worktreeFactsChanged", worktreePath: WT, facts: { ahead: 1, behind: 2, dirty: 2, conflicts: ["b.ts", "c.ts"] } },
    ),
  );
  expect(fileRows()).toEqual(["File b.ts, conflicted", "File c.ts, conflicted", "File README.md, modified"]);
  expect(within(pane()).getByRole("tab", { name: /Changes/ }).textContent).toContain("3");
  expect(within(pane()).getByRole("button", { name: "Commit accepted…" })).toHaveProperty("disabled", true);

  // c.ts has no uncommitted change: only the conflict note, no decision buttons.
  const diff = within(pane()).getByLabelText("Diff of b.ts");
  expect(within(diff).queryByRole("button", { name: "Accept file" })).toBeNull();
  key("a");
  expect(sent.some((c) => c.type === "decideFile")).toBe(false);
  key("j");
  expect(within(pane()).getByLabelText("Diff of c.ts").textContent).toContain("main changed this file too");
});

test("Changes is the review pane's first tab, beside History and Context", () => {
  launch();
  expect(within(pane()).getAllByRole("tab").map((t) => t.textContent?.trim().split(" ")[0])).toEqual([
    "Changes",
    "History",
    "Context",
  ]);
  expect(within(pane()).getByRole("tab", { name: /Changes/ }).getAttribute("aria-selected")).toBe("true");
});

test("keyboard review end to end: j / k move, a / r decide and advance, then commit with a drafted message", () => {
  const { sent, emit, actions } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: files.filter((f) => f.status !== "conflicted") }));
  expect(current()?.getAttribute("aria-label")).toBe("File README.md, modified");

  key("j");
  expect(current()?.getAttribute("aria-label")).toBe("File src/new.ts, added");
  key("k");
  expect(current()?.getAttribute("aria-label")).toBe("File README.md, modified");

  key("a");
  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "README.md", decision: "accept" });
  expect(current()?.getAttribute("aria-label")).toBe("File src/new.ts, added");
  key("a");
  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "src/new.ts", decision: "accept" });
  key("r");
  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "old.ts", decision: "reject" });
  act(() =>
    emit(
      { type: "fileDecided", worktreePath: WT, path: "README.md", decision: "accept" },
      { type: "fileDecided", worktreePath: WT, path: "src/new.ts", decision: "accept" },
      { type: "fileDecided", worktreePath: WT, path: "old.ts", decision: "reject" },
    ),
  );
  // Pressing a decision again on a decided file clears it.
  key("r");
  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "old.ts", decision: null });
  expect(within(pane()).getByText(/2 accepted/)).toBeTruthy();

  // Commit accepted… is in the palette; its box opens focused on a drafted message.
  act(() => actions.paletteItems().find((i) => i.title === "Commit accepted…")!.run());
  const box = within(pane()).getByLabelText("Commit message") as HTMLTextAreaElement;
  expect(document.activeElement).toBe(box);
  expect(box.value).toBe("Fix the README typo\n\n- update README.md\n- add src/new.ts");
  // Single keys type into the message rather than reviewing.
  fireEvent.keyDown(box, { key: "a" });
  expect(sent.filter((c) => c.type === "decideFile")).toHaveLength(4);
  fireEvent.keyDown(box, { key: "Enter", metaKey: true });
  expect(sent).toContainEqual({
    type: "commitAccepted",
    worktreePath: WT,
    message: "Fix the README typo\n\n- update README.md\n- add src/new.ts",
  });
  expect(within(pane()).queryByLabelText("Commit message")).toBeNull();
});

test("Esc cancels an open commit box without committing", () => {
  const { sent, emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [{ ...readme, decision: "accept" }] }));
  fireEvent.click(within(pane()).getByRole("button", { name: "Commit accepted…" }));
  key("Escape");
  expect(within(pane()).queryByLabelText("Commit message")).toBeNull();
  expect(sent.some((c) => c.type === "commitAccepted")).toBe(false);
  expect(sent.some((c) => c.type === "stopSession")).toBe(false);
});

test("a commit toasts its result and the re-read changes carry no decisions", () => {
  const { emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [{ ...readme, decision: "accept" }, changed("b.ts", "modified")] }));
  act(() =>
    emit(
      { type: "committed", worktreePath: WT, branch: "fix/readme", commit: "abc1234", files: ["README.md"] },
      { type: "changesRead", worktreePath: WT, files: [changed("b.ts", "modified")] },
    ),
  );
  expect(screen.getByRole("status").textContent).toContain("Committed 1 file to fix/readme (abc1234)");
  expect(fileRows()).toEqual(["File b.ts, modified"]);
  expect(within(pane()).queryByTestId("decision")).toBeNull();
});

test("empty states: no changes, and when Finished, how many commits are ready", () => {
  const { emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [] }));
  expect(within(pane()).getByText("No uncommitted changes in this worktree.")).toBeTruthy();

  act(() =>
    emit(
      { type: "sessionStatusChanged", sessionId: "session-1", status: "done" },
      { type: "worktreeFactsChanged", worktreePath: WT, facts: { ahead: 3, behind: 0, dirty: 0, conflicts: [] } },
    ),
  );
  expect(within(pane()).getByText("Working tree clean. 3 commits ready.")).toBeTruthy();
});

test("a status change re-reads the changes", () => {
  const { sent, emit } = launch();
  const reads = () => sent.filter((c) => c.type === "readChanges").length;
  const before = reads();
  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "running" }));
  expect(reads()).toBe(before + 1);
});

test("Expand gives the review pane the full width; ⌘⇧D hides and shows it", () => {
  const { emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files }));
  const bench = () => document.querySelector(".workbench")!;

  fireEvent.click(within(pane()).getByRole("button", { name: "Expand" }));
  expect(bench().classList.contains("focus-changes")).toBe(true);
  fireEvent.click(within(pane()).getByRole("button", { name: "Show agent" }));
  expect(bench().classList.contains("focus-changes")).toBe(false);

  key("D", { metaKey: true, shiftKey: true });
  expect(screen.queryByRole("complementary", { name: "Review" })).toBeNull();
  key("D", { metaKey: true, shiftKey: true });
  expect(pane()).toBeTruthy();
});

test("review keys do nothing while the pane is hidden", () => {
  const { sent, emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files }));
  key("D", { metaKey: true, shiftKey: true });
  key("a");
  expect(sent.some((c) => c.type === "decideFile")).toBe(false);
});
