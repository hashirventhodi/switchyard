// Secrets in Context, driven through a fake core: the repository's secrets by name with
// grant / revoke, where each value would come from, adding a local name and keeping a value
// in the keychain. Values are never shown.
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test } from "vitest";
import { App } from "../App";
import type { Event } from "../schema/Event";
import type { Secret } from "../schema/Secret";
import type { Session } from "../schema/Session";
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
  startedAtMs: 1_760_000_000_000,
};

function launch() {
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} />);
  act(() =>
    fake.emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/fix", branch: "fix/readme", primary: false } },
      { type: "sessionStarted", session: fix },
      { type: "transcriptEntry", sessionId: "session-1", index: 0, entry: { type: "user", text: "Fix it" } },
    ),
  );
  fireEvent.click(within(screen.getByRole("complementary", { name: "Repositories" })).getByRole("button", { name: /fix\/readme/ }));
  fireEvent.click(screen.getByRole("tab", { name: "Context" }));
  return fake;
}

const section = () => screen.getByRole("region", { name: "Secrets" });
const changed = (secrets: Secret[]): Event => ({ type: "secretsChanged", repositoryId: "repo-1", secrets });

const secrets: Secret[] = [
  { name: "GITHUB_TOKEN", granted: false, origin: "mcp", source: "environment" },
  { name: "NPM_TOKEN", granted: true, origin: "policy", source: "keychain" },
  { name: "SENTRY_DSN", granted: false, origin: "local", source: "missing" },
];

test("Context reads the repository's secrets and lists them by name with grant state and value source", () => {
  const { sent, emit } = launch();
  expect(sent).toContainEqual({ type: "readSecrets", repositoryId: "repo-1" });

  act(() => emit(changed(secrets)));

  const rows = within(section()).getAllByRole("listitem");
  expect(rows.map((r) => within(r).getByText(/_/, { selector: "code" }).textContent)).toEqual([
    "GITHUB_TOKEN",
    "NPM_TOKEN",
    "SENTRY_DSN",
  ]);
  expect(rows[0].textContent).toContain("environment");
  expect(rows[1].textContent).toContain("keychain");
  expect(rows[1].textContent).toContain("granted");
  expect(rows[2].textContent).toContain("no value");
});

test("Grant and Revoke send the command for that secret", () => {
  const { sent, emit } = launch();
  act(() => emit(changed(secrets)));

  fireEvent.click(within(section()).getByRole("button", { name: "Grant GITHUB_TOKEN" }));
  fireEvent.click(within(section()).getByRole("button", { name: "Revoke NPM_TOKEN" }));

  expect(sent).toContainEqual({ type: "grantSecret", repositoryId: "repo-1", name: "GITHUB_TOKEN" });
  expect(sent).toContainEqual({ type: "revokeSecret", repositoryId: "repo-1", name: "NPM_TOKEN" });
});

test("only a locally added secret can be removed", () => {
  const { sent, emit } = launch();
  act(() => emit(changed(secrets)));

  expect(within(section()).queryByRole("button", { name: "Remove NPM_TOKEN" })).toBeNull();
  fireEvent.click(within(section()).getByRole("button", { name: "Remove SENTRY_DSN" }));

  expect(sent).toContainEqual({ type: "removeSecret", repositoryId: "repo-1", name: "SENTRY_DSN" });
});

test("a secret name can be added, and a value kept in the keychain without being shown", () => {
  const { sent, emit } = launch();
  act(() => emit(changed(secrets)));

  fireEvent.change(within(section()).getByRole("textbox", { name: "Secret name" }), { target: { value: "STRIPE_KEY" } });
  fireEvent.click(within(section()).getByRole("button", { name: "Add secret" }));
  expect(sent).toContainEqual({ type: "addSecret", repositoryId: "repo-1", name: "STRIPE_KEY" });

  fireEvent.click(within(section()).getByRole("button", { name: "Set value of SENTRY_DSN" }));
  const field = within(section()).getByLabelText("Value of SENTRY_DSN");
  expect(field.getAttribute("type")).toBe("password");
  fireEvent.change(field, { target: { value: "https://key@sentry.io/1" } });
  fireEvent.click(within(section()).getByRole("button", { name: "Save to keychain" }));

  expect(sent).toContainEqual({ type: "setSecretValue", repositoryId: "repo-1", name: "SENTRY_DSN", value: "https://key@sentry.io/1" });
  expect(within(section()).queryByLabelText("Value of SENTRY_DSN")).toBeNull();
  expect(section().textContent).not.toContain("https://key@sentry.io/1");
});
