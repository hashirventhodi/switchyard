import { expect, test } from "vitest";
import { matches } from "../keymap";

const ev = (key: string, init: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key, ...init });

// Composer and terminal own these keys; they test events against the shared map.
test("↵ sends and ⇧↵ inserts a newline in the composer", () => {
  expect(matches(ev("Enter"), "composer.send")).toBe(true);
  expect(matches(ev("Enter", { shiftKey: true }), "composer.send")).toBe(false);
  expect(matches(ev("Enter", { shiftKey: true }), "composer.newline")).toBe(true);
});

test("Ctrl+L clears a shell tab, but ⌘L does not", () => {
  expect(matches(ev("l", { ctrlKey: true }), "terminal.clear")).toBe(true);
  expect(matches(ev("l", { metaKey: true }), "terminal.clear")).toBe(false);
  expect(matches(ev("l"), "terminal.clear")).toBe(false);
});

test("? matches with the Shift it needs; ⌘⇧D needs Shift and ⌘D does not match it", () => {
  expect(matches(ev("?", { shiftKey: true }), "help.open")).toBe(true);
  expect(matches(ev("D", { metaKey: true, shiftKey: true }), "review.togglePane")).toBe(true);
  expect(matches(ev("d", { metaKey: true }), "review.togglePane")).toBe(false);
});
