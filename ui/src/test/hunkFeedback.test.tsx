import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import { createActionRegistry } from "../actions";
import type { ChangedFile } from "../schema/ChangedFile";
import type { Event } from "../schema/Event";
import type { Hunk } from "../schema/Hunk";
import type { Session } from "../schema/Session";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const WT = "/wt/acme/fix";
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
  { type: "repositoryAdded", repository: { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: WT, branch: "fix/readme", primary: false } },
  { type: "sessionStarted", session },
  { type: "transcriptEntry", sessionId: "session-1", index: 0, entry: { type: "user", text: "Fix the README typo" } },
];

function hunk(id: string, header: string, extra: Partial<Hunk> = {}): Hunk {
  return {
    id,
    header,
    lines: [
      { kind: "removed", old: 1, new: null, text: `old ${id}` },
      { kind: "added", old: null, new: 1, text: `new ${id}` },
    ],
    decision: null,
    comment: null,
    sent: false,
    ...extra,
  };
}

function changed(path: string, status: ChangedFile["status"], hunks: Hunk[], extra: Partial<ChangedFile> = {}): ChangedFile {
  return { path, oldPath: null, status, added: hunks.length, removed: hunks.length, binary: false, decision: null, stale: false, hunks, staleHunks: [], ...extra };
}

const readme = changed("README.md", "modified", [hunk("h1", "@@ -1,2 +1,2 @@ intro"), hunk("h2", "@@ -9,2 +9,2 @@ outro")]);
const notes = changed("notes.md", "modified", [hunk("n1", "@@ -3 +3 @@")]);

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} actions={createActionRegistry()} />);
  act(() => fake.emit(...base, ...events));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  return fake;
}

const pane = () => screen.getByRole("complementary", { name: "Review" });
const footer = () => pane().querySelector(".review-foot .hint")?.textContent?.replace(/\s+/g, " ").trim();
const hunkBox = (header: string) => within(pane()).getByRole("group", { name: `Hunk ${header}` });
const reviewed = (path: string, hunkId: string, extra: Partial<Extract<Event, { type: "hunkReviewed" }>> = {}): Event => ({
  type: "hunkReviewed",
  worktreePath: WT,
  path,
  hunkId,
  decision: null,
  comment: null,
  sent: false,
  ...extra,
});

test("each hunk has Accept / Reject / Comment, and its decision shows on the hunk", () => {
  const { sent, emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [readme] }));

  const first = hunkBox("@@ -1,2 +1,2 @@ intro");
  fireEvent.click(within(first).getByRole("button", { name: "Reject hunk" }));
  expect(sent).toContainEqual({ type: "decideHunk", worktreePath: WT, path: "README.md", hunkId: "h1", decision: "reject" });
  fireEvent.click(within(hunkBox("@@ -9,2 +9,2 @@ outro")).getByRole("button", { name: "Accept hunk" }));
  expect(sent).toContainEqual({ type: "decideHunk", worktreePath: WT, path: "README.md", hunkId: "h2", decision: "accept" });

  act(() => emit(reviewed("README.md", "h1", { decision: "reject" })));
  expect(hunkBox("@@ -1,2 +1,2 @@ intro").textContent).toContain("rejected");
  expect(within(hunkBox("@@ -1,2 +1,2 @@ intro")).getByRole("button", { name: "Reject hunk" }).getAttribute("aria-pressed")).toBe("true");
  // Pressing it again clears the decision.
  fireEvent.click(within(hunkBox("@@ -1,2 +1,2 @@ intro")).getByRole("button", { name: "Reject hunk" }));
  expect(sent).toContainEqual({ type: "decideHunk", worktreePath: WT, path: "README.md", hunkId: "h1", decision: null });
});

test("a rejected hunk blocks Commit for its file with the reason shown", () => {
  const { emit } = launch();
  act(() =>
    emit({
      type: "changesRead",
      worktreePath: WT,
      files: [changed("README.md", "modified", [hunk("h1", "@@ -1 +1 @@", { decision: "reject" })]), { ...notes, decision: "accept" }],
    }),
  );

  const row = within(pane()).getByRole("button", { name: "File README.md, modified" });
  expect(row.textContent).toContain("rejected");
  fireEvent.click(row);
  expect(within(pane()).getByLabelText("Diff of README.md").textContent).toContain(
    "Not committable: 1 rejected hunk. Send feedback to the agent, or Accept file.",
  );
  expect(footer()).toBe("1 accepted · 1 rejected · 0 undecided");
  fireEvent.click(within(pane()).getByRole("button", { name: "Commit accepted…" }));
  expect((within(pane()).getByLabelText("Commit message") as HTMLTextAreaElement).value).toBe("Fix the README typo\n\n- update notes.md");
  expect(within(pane()).getByText(/Commits 1 accepted file/).textContent).toContain("1 file with rejected hunks stays out");
});

test("a file whose hunks are all accepted counts as accepted", () => {
  const { emit } = launch();
  act(() =>
    emit({
      type: "changesRead",
      worktreePath: WT,
      files: [changed("README.md", "modified", [hunk("h1", "@@ -1 +1 @@", { decision: "accept" })])],
    }),
  );
  expect(within(pane()).getByRole("button", { name: "File README.md, modified" }).textContent).toContain("accepted");
  expect(within(pane()).getByRole("button", { name: "Commit accepted…" })).toHaveProperty("disabled", false);
});

test("Comment opens an editor on the hunk; saving sends the comment, and it shows under the header", () => {
  const { sent, emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [readme] }));

  fireEvent.click(within(hunkBox("@@ -1,2 +1,2 @@ intro")).getByRole("button", { name: "Comment on hunk" }));
  const box = within(hunkBox("@@ -1,2 +1,2 @@ intro")).getByLabelText("Comment for the agent") as HTMLTextAreaElement;
  expect(document.activeElement).toBe(box);
  fireEvent.change(box, { target: { value: "Use the constant." } });
  fireEvent.click(within(hunkBox("@@ -1,2 +1,2 @@ intro")).getByRole("button", { name: "Save comment" }));
  expect(sent).toContainEqual({ type: "commentHunk", worktreePath: WT, path: "README.md", hunkId: "h1", comment: "Use the constant." });
  expect(within(pane()).queryByLabelText("Comment for the agent")).toBeNull();

  act(() => emit(reviewed("README.md", "h1", { comment: "Use the constant." })));
  expect(hunkBox("@@ -1,2 +1,2 @@ intro").textContent).toContain("Use the constant.");
});

test("Send feedback to agent counts the pending hunks across files, sends once, and clears the count when sent", () => {
  const { sent, emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [readme, notes] }));
  expect(within(pane()).getByRole("button", { name: /Send feedback to agent/ })).toHaveProperty("disabled", true);

  act(() =>
    emit(
      reviewed("README.md", "h1", { decision: "reject" }),
      reviewed("README.md", "h1", { decision: "reject", comment: "why?" }),
      reviewed("notes.md", "n1", { comment: "typo" }),
    ),
  );
  const button = within(pane()).getByRole("button", { name: /Send feedback to agent/ });
  expect(button.textContent).toContain("2");
  fireEvent.click(button);
  expect(sent.filter((c) => c.type === "sendFeedback")).toEqual([{ type: "sendFeedback", worktreePath: WT }]);

  act(() =>
    emit(
      reviewed("README.md", "h1", { decision: "reject", comment: "why?", sent: true }),
      reviewed("notes.md", "n1", { comment: "typo", sent: true }),
      { type: "feedbackSent", worktreePath: WT, sessionId: "session-1", hunks: 2 },
    ),
  );
  expect(screen.getByRole("status").textContent).toContain("Sent feedback on 2 hunks to the agent");
  expect(within(pane()).getByRole("button", { name: /Send feedback to agent/ })).toHaveProperty("disabled", true);
  // The rejection still stands, marked as sent.
  expect(hunkBox("@@ -1,2 +1,2 @@ intro").textContent).toContain("sent");
});

test("Accept all accepts every non-conflict file", () => {
  const { sent, emit } = launch();
  act(() =>
    emit({
      type: "changesRead",
      worktreePath: WT,
      files: [readme, { ...notes, decision: "accept" }, changed("merge.ts", "conflicted", []), changed("new.ts", "added", [])],
    }),
  );
  fireEvent.click(within(pane()).getByRole("button", { name: "Accept all" }));
  expect(sent.filter((c) => c.type === "decideFile")).toEqual([
    { type: "decideFile", worktreePath: WT, path: "README.md", decision: "accept" },
    { type: "decideFile", worktreePath: WT, path: "new.ts", decision: "accept" },
  ]);
});

test("a fileDecided accept clears its hunks' rejections only through hunkReviewed events", () => {
  const { emit } = launch();
  act(() => emit({ type: "changesRead", worktreePath: WT, files: [readme] }));
  act(() => emit(reviewed("README.md", "h1", { decision: "reject" })));
  act(() =>
    emit({ type: "fileDecided", worktreePath: WT, path: "README.md", decision: "accept" }, reviewed("README.md", "h1")),
  );
  expect(within(pane()).getByLabelText("Diff of README.md").textContent).not.toContain("Not committable");
  expect(footer()).toBe("1 accepted · 0 rejected · 0 undecided");
});
