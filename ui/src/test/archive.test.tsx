// Resume, stop and replace, archived sessions in Context, retention settings and Export
// transcript, driven through a fake core.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { ArchivedSession } from "../schema/ArchivedSession";
import type { Entry } from "../schema/Entry";
import type { Event } from "../schema/Event";
import type { Session } from "../schema/Session";
import type { SessionStatus } from "../schema/SessionStatus";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };
const session = (id: string, status: SessionStatus): Session => ({
  id,
  repositoryId: "repo-1",
  worktreePath: "/wt/acme/fix",
  branch: "fix/readme",
  agent: "claudeCode",
  permissionMode: "askFirst",
  status,
  startedAtMs: 1_760_000_000_000,
});
const entry = (sessionId: string, index: number, e: Entry): Event => ({ type: "transcriptEntry", sessionId, index, entry: e });
const archived = (id: string, archivedAtMs = 1_760_000_100_000): ArchivedSession => ({
  session: session(id, "idle"),
  archivedAtMs,
  entries: 2,
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
      ...events,
    ),
  );
  return fake;
}

const sidebar = () => screen.getByRole("complementary", { name: "Repositories" });
const openFix = () => fireEvent.click(within(sidebar()).getByRole("button", { name: /fix\/readme/ }));
const sessionRegion = () => screen.getByRole("region", { name: "Session fix/readme" });

test("a Paused session offers Resume, which asks the core to resume it", () => {
  const { sent } = launch({ type: "sessionStarted", session: session("session-1", "idle") }, entry("session-1", 0, { type: "user", text: "Fix it" }));
  openFix();

  fireEvent.click(within(sessionRegion()).getByRole("button", { name: "Resume" }));

  expect(sent).toContainEqual({ type: "resumeSession", sessionId: "session-1" });
  expect((screen.getByLabelText("Message the agent") as HTMLTextAreaElement).placeholder).toMatch(/Resume/);
});

test("a Finished session offers Resume; a Running one offers Stop instead", () => {
  const { emit } = launch({ type: "sessionStarted", session: session("session-1", "done") });
  openFix();
  expect(within(sessionRegion()).getByRole("button", { name: "Resume" })).toBeTruthy();

  act(() => emit({ type: "sessionStatusChanged", sessionId: "session-1", status: "running" }));

  expect(within(sessionRegion()).queryByRole("button", { name: "Resume" })).toBeNull();
  expect(within(sessionRegion()).getByRole("button", { name: /Stop/ })).toBeTruthy();
});

test("Export transcript asks the core and says where the file went", () => {
  const { sent, emit } = launch({ type: "sessionStarted", session: session("session-1", "done") });
  openFix();

  fireEvent.click(within(sessionRegion()).getByRole("button", { name: "Export transcript" }));
  act(() => emit({ type: "transcriptExported", sessionId: "session-1", path: "/Users/me/Downloads/fix-readme-session-1.jsonl" }));

  expect(sent).toContainEqual({ type: "exportTranscript", sessionId: "session-1" });
  expect(screen.getByText(/Exported the transcript to \/Users\/me\/Downloads\/fix-readme-session-1.jsonl/)).toBeTruthy();
});

test("New session on a worktree with a Paused session offers stop and replace", () => {
  const { sent } = launch({ type: "sessionStarted", session: session("session-1", "idle") });
  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  fireEvent.click(within(dialog).getByRole("radio", { name: /Use existing/ }));
  const worktree = within(dialog).getByLabelText("Worktree") as HTMLSelectElement;
  const option = within(worktree).getByRole("option", { name: /fix\/readme/ }) as HTMLOptionElement;
  expect(option.disabled).toBe(false);
  expect(option.textContent).toMatch(/Paused/);
  fireEvent.change(worktree, { target: { value: "/wt/acme/fix" } });
  fireEvent.change(within(dialog).getByLabelText("Prompt"), { target: { value: "Try again" } });

  expect(within(dialog).getByText(/archived/i)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "Stop and replace" }));

  expect(sent).toContainEqual({
    type: "replaceSession",
    sessionId: "session-1",
    agent: "claudeCode",
    permissionMode: "askFirst",
    prompt: "Try again",
  });
});

test("a worktree with a Running session is still not offered", () => {
  launch({ type: "sessionStarted", session: session("session-1", "running") });
  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  const dialog = screen.getByRole("dialog", { name: "New session" });
  fireEvent.click(within(dialog).getByRole("radio", { name: /Use existing/ }));
  const worktree = within(dialog).getByLabelText("Worktree") as HTMLSelectElement;

  expect((within(worktree).getByRole("option", { name: /fix\/readme/ }) as HTMLOptionElement).disabled).toBe(true);
});

function contextOfNewSession(fake: ReturnType<typeof fakeCore>) {
  openFix();
  fireEvent.click(screen.getByRole("tab", { name: /Context/ }));
  return screen.getByRole("region", { name: "Archived sessions" });
}

test("an archived session leaves the live sessions and is listed in Context by branch", () => {
  const fake = launch(
    { type: "sessionStarted", session: session("session-1", "idle") },
    { type: "sessionArchived", archived: archived("session-1") },
    { type: "sessionStarted", session: session("session-2", "running") },
    { type: "settingsChanged", settings: { archiveKeepPerRepository: 50, archiveKeepDays: 30 } },
  );

  const section = contextOfNewSession(fake);

  expect(within(sidebar()).getAllByRole("button", { name: /fix\/readme/ })).toHaveLength(1);
  const row = within(section).getByRole("listitem");
  expect(row.textContent).toMatch(/fix\/readme/);
  expect(row.textContent).toMatch(/session-1/);
});

test("Read opens the archived transcript; Export writes it", () => {
  const fake = launch(
    { type: "sessionArchived", archived: archived("session-1") },
    { type: "sessionStarted", session: session("session-2", "running") },
  );
  const section = contextOfNewSession(fake);

  fireEvent.click(within(section).getByRole("button", { name: "Read session-1" }));
  expect(fake.sent).toContainEqual({ type: "readArchivedSession", sessionId: "session-1" });
  act(() =>
    fake.emit({
      type: "archivedTranscript",
      sessionId: "session-1",
      entries: [
        { type: "user", text: "The first attempt" },
        { type: "system", text: "Stopped by you." },
      ],
    }),
  );

  const reader = screen.getByRole("dialog", { name: /Archived session/ });
  expect(within(reader).getByText("The first attempt")).toBeTruthy();
  expect(within(reader).getByText("Stopped by you.")).toBeTruthy();
  fireEvent.click(within(reader).getByRole("button", { name: "Export transcript" }));
  expect(fake.sent).toContainEqual({ type: "exportTranscript", sessionId: "session-1" });
});

test("a pruned archived session disappears from Context", () => {
  const fake = launch(
    { type: "sessionArchived", archived: archived("session-1") },
    { type: "sessionStarted", session: session("session-2", "running") },
  );
  const section = contextOfNewSession(fake);

  act(() => fake.emit({ type: "archivedSessionPruned", repositoryId: "repo-1", sessionId: "session-1" }));

  expect(within(section).queryByRole("listitem")).toBeNull();
});

test("removing a worktree with a Paused session says the session is archived", () => {
  launch({ type: "sessionStarted", session: session("session-1", "idle") });
  fireEvent.click(screen.getByRole("button", { name: "Worktrees" }));
  const card = screen.getByRole("article", { name: "Worktree fix/readme" });

  fireEvent.click(within(card).getByRole("button", { name: "Remove" }));

  expect(within(card).getByRole("group", { name: "Remove fix/readme?" }).textContent).toMatch(/Paused session is archived/);
});

test("retention settings are shown and saved through the core", () => {
  const fake = launch(
    { type: "sessionStarted", session: session("session-2", "running") },
    { type: "settingsChanged", settings: { archiveKeepPerRepository: 50, archiveKeepDays: 30 } },
  );
  const section = contextOfNewSession(fake);
  const keep = within(section).getByLabelText("Keep per repository") as HTMLInputElement;
  const days = within(section).getByLabelText("Keep days") as HTMLInputElement;
  expect([keep.value, days.value]).toEqual(["50", "30"]);

  fireEvent.change(keep, { target: { value: "10" } });
  fireEvent.change(days, { target: { value: "7" } });
  fireEvent.click(within(section).getByRole("button", { name: "Save retention" }));

  expect(fake.sent).toContainEqual({ type: "updateSettings", settings: { archiveKeepPerRepository: 10, archiveKeepDays: 7 } });
});
