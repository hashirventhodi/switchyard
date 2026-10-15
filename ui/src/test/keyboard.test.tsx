import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { App } from "../App";
import { createActionRegistry } from "../actions";
import { fakeCore } from "./fakeCore";

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
});

function launch() {
  const actions = createActionRegistry();
  const fake = fakeCore();
  render(<App core={fake.core} pickFolder={async () => null} actions={actions} />);
  return { actions, ...fake };
}

const key = (k: string, init: KeyboardEventInit = {}, target: Element = document.body) =>
  act(() => {
    fireEvent.keyDown(target, { key: k, ...init });
  });

const currentView = () =>
  screen.getByRole("navigation", { name: "Views" }).querySelector("[aria-current=page]")?.getAttribute("aria-label");

test("? lists every shortcut in the spec's table", () => {
  launch();
  key("?");

  const help = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  const keys = within(help)
    .getAllByRole("row")
    .map((r) => r.querySelector("td:last-child")?.textContent);
  for (const k of ["⌘K", "⌘N", "⌘J", "⌘B", "⌘⇧D", "1 – 4", "`", "a / r", "j / k", "?", "↵ / ⇧↵", "Ctrl L", "Esc"]) {
    expect(keys).toContain(k);
  }
});

test("a single key switches view outside a text field but types into one", () => {
  launch();
  key("3");
  expect(currentView()).toBe("Worktrees");

  const composer = document.body.appendChild(document.createElement("textarea"));
  composer.focus();
  let typed = true;
  act(() => {
    typed = fireEvent.keyDown(composer, { key: "1" });
  });
  expect(typed).toBe(true); // default not prevented: the character goes in
  expect(currentView()).toBe("Worktrees");

  key("1");
  expect(currentView()).toBe("Dashboard");
});

test("shortcuts for features that register later run whatever handler is registered", () => {
  const { actions } = launch();
  const ids = [
    "session.new",
    "needsYou.next",
    "review.togglePane",
    "review.accept",
    "review.reject",
    "review.nextFile",
    "review.prevFile",
  ] as const;
  const ran: string[] = [];
  for (const id of ids) actions.register(id, () => void ran.push(id));

  key("n", { metaKey: true });
  key("j", { ctrlKey: true });
  key("D", { metaKey: true, shiftKey: true });
  key("a");
  key("r");
  key("j");
  key("k");

  expect(ran).toEqual([...ids]);
});

test("review single keys do nothing while typing", () => {
  const { actions } = launch();
  const accept = vi.fn();
  actions.register("review.accept", accept);
  const input = document.body.appendChild(document.createElement("input"));

  key("a", {}, input);

  expect(accept).not.toHaveBeenCalled();
});

test("Esc with an overlay open closes it and does not interrupt the agent", () => {
  const { actions } = launch();
  const interrupt = vi.fn();
  actions.register("session.interrupt", interrupt);

  key("?");
  key("Escape", {}, screen.getByRole("dialog"));

  expect(screen.queryByRole("dialog")).toBeNull();
  expect(interrupt).not.toHaveBeenCalled();
});

test("Esc without an overlay cancels an inline confirm before interrupting the agent", () => {
  const { actions } = launch();
  const interrupt = vi.fn();
  actions.register("session.interrupt", interrupt);
  const cancel = vi.fn();
  const unregister = actions.register("confirm.cancel", cancel);

  key("Escape");
  expect(cancel).toHaveBeenCalledTimes(1);
  expect(interrupt).not.toHaveBeenCalled();

  unregister();
  key("Escape");
  expect(interrupt).toHaveBeenCalledTimes(1);
});

test("⌘K opens the palette; typing fuzzy-filters and ↑ ↓ ↵ pick", () => {
  launch();
  key("k", { metaKey: true });
  const input = screen.getByRole("textbox", { name: /search/i });

  act(() => {
    fireEvent.change(input, { target: { value: "wktr" } });
  });
  expect(screen.getAllByRole("option")[0].textContent).toMatch(/^Worktrees/);

  act(() => {
    fireEvent.change(input, { target: { value: "s" } });
  });
  const titles = screen.getAllByRole("option").map((o) => o.textContent);
  const target = titles.findIndex((t) => t?.startsWith("Session"));
  for (let i = 0; i < target + 1; i++) key("ArrowDown", {}, input);
  key("ArrowUp", {}, input);
  key("Enter", {}, input);

  expect(screen.queryByRole("dialog")).toBeNull();
  expect(currentView()).toBe("Session");
});

test("the palette finds worktrees by branch and shows actions with their keys", () => {
  const { emit } = launch();
  act(() =>
    emit(
      { type: "attached" },
      { type: "repositoryAdded", repository: { id: "r", name: "acme", path: "/src/acme", baseBranch: "main" } },
      { type: "worktreeAdded", repositoryId: "r", worktree: { path: "/wt/login", branch: "feat/login-form", primary: false } },
    ),
  );
  key("k", { metaKey: true });
  const palette = screen.getByRole("dialog", { name: "Command palette" });

  expect(within(palette).getByRole("option", { name: /Toggle sidebar/ }).textContent).toContain("⌘B");

  act(() => {
    fireEvent.change(within(palette).getByRole("textbox"), { target: { value: "flogin" } });
  });
  expect(within(palette).getAllByRole("option").map((o) => o.textContent)).toEqual([
    expect.stringContaining("acme/feat/login-form"),
  ]);
});

test("the palette offers items other features provide, matched by task key", () => {
  const { actions } = launch();
  const open = vi.fn();
  actions.addPaletteSource(() => [
    { id: "t1", group: "Tasks", title: "Per-tenant rate limiting", keywords: ["#412"], run: open },
  ]);

  key("k", { metaKey: true });
  const input = screen.getByRole("textbox");
  act(() => {
    fireEvent.change(input, { target: { value: "412" } });
  });
  key("Enter", {}, input);

  expect(open).toHaveBeenCalled();
});

test("⌘K toggles the palette closed, and single keys do not fire under it", () => {
  launch();
  key("k", { metaKey: true });
  key("3");
  expect(currentView()).toBe("Dashboard");

  key("k", { metaKey: true });
  expect(screen.queryByRole("dialog")).toBeNull();
});
