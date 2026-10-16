// Stale decisions in the Changes tab (ADR-0008): a Decision whose file or hunk changed since
// it was made stays visible, flagged "changed since you decided", stops counting, and is
// re-confirmed in one click with the ordinary decide command.
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
const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const session: Session = {
  id: "session-1",
  repositoryId: "repo-1",
  worktreePath: WT,
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  model: { type: "default" },
  status: "waiting",
  startedAtMs: 0,
};
const base: Event[] = [
  { type: "attached" },
  { type: "repositoryAdded", repository: acme },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
  { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: WT, branch: "fix/readme", primary: false } },
  { type: "sessionStarted", session },
];

const hunk = (id: string, header: string): Hunk => ({
  id,
  header,
  lines: [{ kind: "added", old: null, new: 1, text: "x" }],
  decision: null,
  comment: null,
  sent: false,
});

function changed(path: string, extra: Partial<ChangedFile> = {}): ChangedFile {
  return {
    path,
    oldPath: null,
    status: "modified",
    added: 1,
    removed: 0,
    binary: false,
    decision: null,
    stale: false,
    hunks: [hunk(`${path}-1`, "@@ -1 +1 @@")],
    staleHunks: [],
    ...extra,
  };
}

function launch(files: ChangedFile[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} actions={createActionRegistry()} />);
  act(() => fake.emit(...base));
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  act(() => fake.emit({ type: "changesRead", worktreePath: WT, files }));
  return fake;
}

const pane = () => screen.getByRole("complementary", { name: "Review" });
const row = (path: string) => within(pane()).getByRole("button", { name: new RegExp(`^File ${path},`) });
const tally = () => pane().querySelector(".review-foot .hint")?.textContent;

test("a stale Accept is flagged on its row and in its diff and does not count as accepted", () => {
  launch([changed("a.ts", { decision: "accept", stale: true }), changed("b.ts", { decision: "accept" })]);

  expect(row("a.ts").textContent).toContain("changed since you decided");
  expect(row("a.ts").textContent).not.toContain("accepted");
  expect(row("b.ts").textContent).toContain("accepted");
  expect(tally()).toBe("1 accepted · 0 rejected · 1 undecided");
  fireEvent.click(row("a.ts"));
  expect(within(within(pane()).getByLabelText("Diff of a.ts")).getByRole("note").textContent).toContain(
    "You accepted this file, but it changed since you decided",
  );
});

test("a stale decision's button is neither pressed nor drawn pressed, but flagged", () => {
  launch([changed("a.ts", { decision: "accept", stale: true }), changed("b.ts", { decision: "accept" })]);

  fireEvent.click(row("a.ts"));
  const stale = within(within(pane()).getByLabelText("Diff of a.ts")).getByRole("button", { name: "Accept file" });
  expect(stale.getAttribute("aria-pressed")).toBe("false");
  expect(stale.classList.contains("ok")).toBe(false);
  expect(stale.classList.contains("stale")).toBe(true);
  fireEvent.click(row("b.ts"));
  const current = within(within(pane()).getByLabelText("Diff of b.ts")).getByRole("button", { name: "Accept file" });
  expect(current.getAttribute("aria-pressed")).toBe("true");
  expect(current.classList.contains("ok")).toBe(true);
});

test("a stale Reject does not count as rejected", () => {
  launch([changed("a.ts", { decision: "reject", stale: true })]);
  expect(tally()).toBe("0 accepted · 0 rejected · 1 undecided");
  expect(row("a.ts").textContent).toContain("changed since you decided");
});

test("only stale Accepts leave Commit accepted disabled", () => {
  launch([changed("a.ts", { decision: "accept", stale: true })]);
  expect((within(pane()).getByRole("button", { name: "Commit accepted…" }) as HTMLButtonElement).disabled).toBe(true);
});

test("re-confirming sends the ordinary decideFile and the fresh decision clears the flag", () => {
  const { sent, emit } = launch([changed("a.ts", { decision: "accept", stale: true })]);
  fireEvent.click(row("a.ts"));

  fireEvent.click(within(pane()).getByRole("button", { name: "Re-confirm accept" }));

  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "a.ts", decision: "accept" });
  act(() => emit({ type: "fileDecided", worktreePath: WT, path: "a.ts", decision: "accept" }));
  expect(row("a.ts").textContent).not.toContain("changed since you decided");
  expect(tally()).toBe("1 accepted · 0 rejected · 0 undecided");
});

test("pressing Accept file on a stale Accept re-confirms it rather than clearing it", () => {
  const { sent } = launch([changed("a.ts", { decision: "accept", stale: true })]);
  fireEvent.click(row("a.ts"));
  fireEvent.click(within(pane()).getByRole("button", { name: "Accept file" }));
  expect(sent).toContainEqual({ type: "decideFile", worktreePath: WT, path: "a.ts", decision: "accept" });
});

test("a stale hunk Reject whose feedback was sent reads feedback sent, revised; an unsent one reads changed since you decided", () => {
  launch([
    changed("a.ts", {
      staleHunks: [
        { id: "old-1", header: "@@ -5 +5 @@ fn one", decision: "reject", comment: "use a constant", sent: true },
        { id: "old-2", header: "@@ -30 +30 @@ fn two", decision: "accept", comment: null, sent: false },
      ],
    }),
  ]);
  fireEvent.click(row("a.ts"));
  const diff = within(pane()).getByLabelText("Diff of a.ts");

  const revised = within(diff).getByRole("group", { name: "Earlier hunk @@ -5 +5 @@ fn one" });
  expect(revised.textContent).toContain("rejected");
  expect(revised.textContent).toContain("feedback sent, revised");
  expect(revised.textContent).toContain("use a constant");
  const changedSince = within(diff).getByRole("group", { name: "Earlier hunk @@ -30 +30 @@ fn two" });
  expect(changedSince.textContent).toContain("changed since you decided");
  // Neither counts: the file has no hunk decision in force.
  expect(tally()).toBe("0 accepted · 0 rejected · 1 undecided");
});

test("a commit that skipped stale Accepts says which and why", () => {
  const { emit } = launch([changed("a.ts", { decision: "accept", stale: true }), changed("b.ts", { decision: "accept" })]);
  act(() =>
    emit({ type: "committed", worktreePath: WT, branch: "fix/readme", commit: "abc1234", files: ["b.ts"], skippedStale: ["a.ts"] }),
  );
  expect(screen.getByRole("status").textContent).toContain(
    "Committed 1 file to fix/readme (abc1234); skipped a.ts: changed since you decided",
  );
});

test("a commit that skipped every accepted file says nothing was committed and why", () => {
  const { emit } = launch([changed("a.ts", { decision: "accept", stale: true })]);
  act(() => emit({ type: "committed", worktreePath: WT, branch: "fix/readme", commit: "", files: [], skippedStale: ["a.ts"] }));
  expect(screen.getByRole("status").textContent).toContain(
    "Nothing committed: a.ts changed since you decided; re-confirm to commit",
  );
});
