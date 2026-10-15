// The one keyboard map. Every shortcut is a Binding from a key chord to an ActionId; what an
// action does is registered separately (see actions.tsx), so features that arrive later only
// register a handler for an existing id. The ? overlay and the palette are built from this table.

import type { KeyboardEvent as ReactKeyboardEvent } from "react";

export type ActionId =
  | "palette.toggle"
  | "session.new"
  | "needsYou.next"
  | "sidebar.toggle"
  | "review.togglePane"
  | "view.dashboard"
  | "view.session"
  | "view.worktrees"
  | "view.tasks"
  | "dock.toggle"
  | "review.accept"
  | "review.reject"
  | "review.nextFile"
  | "review.prevFile"
  | "help.open"
  | "composer.send"
  | "composer.newline"
  | "terminal.clear"
  // Esc is layered: close overlay → confirm.cancel → session.interrupt. Handlers return false
  // when they have nothing to do so the next layer runs.
  | "escape"
  | "confirm.cancel"
  | "session.interrupt";

export interface Chord {
  key: string;
  /** ⌘ on macOS; Ctrl is accepted too so the app works elsewhere. */
  mod?: boolean;
  /** A literal Ctrl chord (terminal Ctrl+L), shown as "Ctrl". */
  ctrl?: boolean;
  shift?: boolean;
}

/**
 * Where a binding fires:
 * - global: anywhere, even in a text field (modifier chords, Esc);
 * - outsideText: single keys, only when focus is not in a text field and no overlay is open;
 * - composer / terminal: only inside that component, which matches it itself with `matches`.
 */
export type Scope = "global" | "outsideText" | "composer" | "terminal";

export interface Binding {
  action: ActionId;
  chord: Chord;
  scope: Scope;
  /** Palette title; actions without one are not offered in the palette. */
  title?: string;
}

const b = (action: ActionId, chord: Chord, scope: Scope, title?: string): Binding => ({ action, chord, scope, title });

export const BINDINGS: Binding[] = [
  b("palette.toggle", { key: "k", mod: true }, "global"),
  b("session.new", { key: "n", mod: true }, "global", "New session"),
  b("needsYou.next", { key: "j", mod: true }, "global", "Next session needing you"),
  b("sidebar.toggle", { key: "b", mod: true }, "global", "Toggle sidebar"),
  b("review.togglePane", { key: "d", mod: true, shift: true }, "global", "Toggle review pane"),
  b("view.dashboard", { key: "1" }, "outsideText"),
  b("view.session", { key: "2" }, "outsideText"),
  b("view.worktrees", { key: "3" }, "outsideText"),
  b("view.tasks", { key: "4" }, "outsideText"),
  b("dock.toggle", { key: "`" }, "outsideText", "Toggle terminal dock"),
  b("review.accept", { key: "a" }, "outsideText"),
  b("review.reject", { key: "r" }, "outsideText"),
  b("review.nextFile", { key: "j" }, "outsideText"),
  b("review.prevFile", { key: "k" }, "outsideText"),
  b("help.open", { key: "?" }, "outsideText", "Keyboard shortcuts"),
  b("composer.send", { key: "Enter" }, "composer"),
  b("composer.newline", { key: "Enter", shift: true }, "composer"),
  b("terminal.clear", { key: "l", ctrl: true }, "terminal"),
  b("escape", { key: "Escape" }, "global"),
];

/** Rows of the ? overlay, in order. `range` shows first – last instead of a / b. */
export const HELP_ROWS: { label: string; actions: ActionId[]; range?: boolean }[] = [
  { label: "Command palette", actions: ["palette.toggle"] },
  { label: "New session", actions: ["session.new"] },
  { label: "Next session needing you", actions: ["needsYou.next"] },
  { label: "Toggle sidebar", actions: ["sidebar.toggle"] },
  { label: "Toggle review pane", actions: ["review.togglePane"] },
  { label: "Dashboard · Session · Worktrees · Tasks", actions: ["view.dashboard", "view.session", "view.worktrees", "view.tasks"], range: true },
  { label: "Toggle terminal dock", actions: ["dock.toggle"] },
  { label: "Accept / reject current file (review)", actions: ["review.accept", "review.reject"] },
  { label: "Next / previous file (review)", actions: ["review.nextFile", "review.prevFile"] },
  { label: "Send / newline (composer)", actions: ["composer.send", "composer.newline"] },
  { label: "Clear terminal (shell tab)", actions: ["terminal.clear"] },
  { label: "Close overlay · cancel confirm · interrupt agent", actions: ["escape"] },
  { label: "This help", actions: ["help.open"] },
];

export function bindingFor(action: ActionId): Binding {
  const found = BINDINGS.find((x) => x.action === action);
  if (!found) throw new Error(`no binding for ${action}`);
  return found;
}

const KEY_NAMES: Record<string, string> = { Enter: "↵", Escape: "Esc" };

/** How a chord is written in the UI, e.g. "⌘⇧D", "Ctrl L", "`". */
export function chordLabel(chord: Chord): string {
  const named = KEY_NAMES[chord.key];
  const key = named ?? (chord.mod || chord.ctrl ? chord.key.toUpperCase() : chord.key);
  if (chord.ctrl) return `Ctrl ${key}`;
  return `${chord.mod ? "⌘" : ""}${chord.shift ? "⇧" : ""}${key}`;
}

export function keysLabel(action: ActionId): string {
  return chordLabel(bindingFor(action).chord);
}

const isLetter = (k: string) => /^[a-z]$/i.test(k);

/** Does this keyboard event press the chord? */
export function matchesChord(e: KeyboardEvent, chord: Chord): boolean {
  if (e.altKey) return false;
  if (e.key.toLowerCase() !== chord.key.toLowerCase()) return false;
  if (chord.ctrl) return e.ctrlKey && !e.metaKey;
  if ((e.metaKey || e.ctrlKey) !== !!chord.mod) return false;
  // Shift only counts for letters and named keys; "?" needs Shift on most layouts.
  if (isLetter(chord.key) || chord.key.length > 1) return e.shiftKey === !!chord.shift;
  return true;
}

/** For components that own a scoped binding: `if (matches(e, "composer.send")) ...`. */
export function matches(e: KeyboardEvent | ReactKeyboardEvent, action: ActionId): boolean {
  const native = "nativeEvent" in e ? e.nativeEvent : e;
  return matchesChord(native, bindingFor(action).chord);
}

export function inTextField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName));
}
