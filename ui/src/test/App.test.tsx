import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { App } from "../App";
import { fakeCore } from "./fakeCore";

afterEach(cleanup);

const acme = { id: "repo-1", name: "acme", path: "/src/acme", baseBranch: "main" };

test("attaches to the core on start and shows repositories with their worktrees", () => {
  const { core, sent, emit } = fakeCore();
  render(<App core={core} pickFolder={async () => null} />);

  expect(sent).toEqual([{ type: "attach" }]);

  act(() =>
    emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: acme },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/src/acme", branch: "main", primary: true } },
      { type: "worktreeAdded", repositoryId: "repo-1", worktree: { path: "/wt/acme/login", branch: "feat/login", primary: false } },
    ),
  );

  const sidebar = screen.getByRole("complementary", { name: "Repositories" });
  const repo = within(sidebar).getByRole("group", { name: "acme" });
  expect(within(repo).getByText("base main")).toBeTruthy();
  const rows = within(repo).getAllByRole("listitem");
  expect(rows.map((r) => r.textContent)).toEqual([
    expect.stringMatching(/^main.*primary.*free/),
    expect.stringMatching(/^feat\/login.*free/),
  ]);
  expect(rows[1].textContent).not.toContain("primary");
});

test("an attach replay replaces what the renderer held", () => {
  const { core, emit } = fakeCore();
  render(<App core={core} pickFolder={async () => null} />);
  const added = { type: "repositoryAdded", repository: acme } as const;

  act(() => emit({ type: "attached" }, added));
  act(() => emit({ type: "attached" }, added));

  expect(screen.getAllByRole("group", { name: "acme" })).toHaveLength(1);
});

test("+ Repo sends addRepository with the picked folder", async () => {
  const { core, sent } = fakeCore();
  render(<App core={core} pickFolder={async () => "/src/new"} />);

  await act(async () => fireEvent.click(screen.getByRole("button", { name: "+ Repo" })));

  expect(sent).toContainEqual({ type: "addRepository", path: "/src/new" });
});

test("a refused command is shown as a toast naming the reason", () => {
  const { core, emit } = fakeCore();
  render(<App core={core} pickFolder={async () => null} />);

  act(() => emit({ type: "commandRefused", command: "addRepository", reason: "/tmp/x is not a git repository" }));

  expect(screen.getByRole("status").textContent).toContain("/tmp/x is not a git repository");
});
