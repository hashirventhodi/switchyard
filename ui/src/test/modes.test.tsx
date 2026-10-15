// Permission modes in the renderer: the mode-mismatch warning and the classifier-denial
// count on the summary strip and the session card, the composer's mode segment, and New
// session's default mode and ceiling from the repository's policy.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Policy } from "../schema/Policy";
import type { Session } from "../schema/Session";
import type { SessionFacts } from "../schema/SessionFacts";
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
  permissionMode: "fullAuto",
  status: "running",
  startedAtMs: 0,
};
const facts = (more: Partial<SessionFacts>): Event => ({
  type: "sessionFactsChanged",
  sessionId: "session-1",
  facts: { model: "m", cliVersion: "2.1.300", cliPermissionMode: "auto", turns: 1, modeWarning: null, classifierDenials: 0, mcpServers: [], ...more },
});
const policy = (more: Partial<Policy>): Event => ({
  type: "policyChanged",
  repositoryId: "repo-1",
  policy: { defaultMode: null, ceiling: null, grants: [], error: null, ...more },
});

function launch(...events: Event[]) {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() => fake.emit(...repoEvents, ...events));
  return fake;
}

const openSession = () =>
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));

const warning = "Asked for Full auto, but Claude Code started in Ask first (default)";

test("a mode mismatch warns on the summary strip and on the session card", () => {
  launch({ type: "sessionStarted", session }, facts({ cliPermissionMode: "default", modeWarning: warning }));

  const card = screen.getByRole("button", { name: "Open fix/readme" }).closest(".card") as HTMLElement;
  expect(within(card).getByRole("status").textContent).toContain(warning);

  openSession();
  const strip = screen.getByLabelText("Summary");
  expect(within(strip).getByRole("status").textContent).toContain(warning);
});

test("classifier denials are counted on the strip and never turn the session amber", () => {
  launch({ type: "sessionStarted", session }, facts({ classifierDenials: 3 }));
  openSession();
  expect(within(screen.getByLabelText("Summary")).getByText("3 denied by classifier")).toBeTruthy();
  expect(screen.getByRole("region", { name: "Session fix/readme" }).querySelector("[data-testid=status]")?.getAttribute("data-status")).toBe(
    "running",
  );
});

test("the composer's mode segment switches the session and hides modes above the ceiling", () => {
  const { sent, emit } = launch({ type: "sessionStarted", session: { ...session, permissionMode: "askFirst" } }, policy({ ceiling: "autoEdit" }));
  openSession();
  expect(sent).toContainEqual({ type: "readPolicy", repositoryId: "repo-1" });

  const segment = screen.getByRole("radiogroup", { name: "Session permission mode" });
  const modes = within(segment)
    .getAllByRole("radio")
    .map((r) => r.getAttribute("aria-label") ?? r.parentElement?.textContent);
  expect(modes).toEqual(["Ask first", "Auto edit"]);
  expect(within(segment).getByRole("radio", { name: "Ask first" })).toHaveProperty("checked", true);

  fireEvent.click(within(segment).getByRole("radio", { name: "Auto edit" }));
  expect(sent).toContainEqual({ type: "setPermissionMode", sessionId: "session-1", permissionMode: "autoEdit" });

  act(() => emit({ type: "sessionModeChanged", sessionId: "session-1", permissionMode: "autoEdit" }));
  expect(within(segment).getByRole("radio", { name: "Auto edit" })).toHaveProperty("checked", true);
  expect(within(screen.getByLabelText("Summary")).getByText(/^Auto edit/)).toBeTruthy();
});

test("an Explore session has no mode segment", () => {
  launch({ type: "sessionStarted", session: { ...session, permissionMode: "explore" } });
  openSession();
  expect(screen.queryByRole("radiogroup", { name: "Session permission mode" })).toBeNull();
});

test("New session preselects the policy's default mode and offers nothing above the ceiling", () => {
  const { sent, emit } = launch();
  fireEvent.click(screen.getByRole("button", { name: "New session" }));
  expect(sent).toContainEqual({ type: "readPolicy", repositoryId: "repo-1" });
  const dialog = screen.getByRole("dialog", { name: "New session" });
  const offered = () => within(dialog).getAllByRole("radio", { name: /Explore|Ask first|Auto edit|Full auto/ }).map((r) => r.getAttribute("value"));
  // No policy yet: every mode, Ask first chosen.
  expect(offered()).toEqual(["askFirst", "autoEdit", "fullAuto", "explore"]);
  expect(within(dialog).getByRole("radio", { name: "Ask first" })).toHaveProperty("checked", true);

  act(() => emit(policy({ defaultMode: "autoEdit", ceiling: "autoEdit" })));
  expect(offered()).toEqual(["askFirst", "autoEdit", "explore"]);
  expect(within(dialog).getByRole("radio", { name: "Auto edit" })).toHaveProperty("checked", true);

  // The user's own choice is kept when the policy arrives again.
  fireEvent.click(within(dialog).getByRole("radio", { name: "Ask first" }));
  act(() => emit(policy({ defaultMode: "autoEdit", ceiling: "autoEdit" })));
  expect(within(dialog).getByRole("radio", { name: "Ask first" })).toHaveProperty("checked", true);
});
