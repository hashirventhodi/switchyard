// The action registry: what each ActionId in keymap.ts does right now. Components register a
// handler while they can act (useAction), and the palette asks registered sources for items
// (usePaletteSource). Shortcuts, palette and buttons all go through `run`.
import { createContext, useContext, useEffect, useRef } from "react";
import type { ActionId } from "./keymap";
import type { Matchable } from "./fuzzy";

/** Return false when there was nothing to do, so a layered key (Esc) falls through. */
export type Handler = () => boolean | void;

export type PaletteGroup = "Sessions" | "Worktrees" | "Tasks" | "Views" | "Actions";
export const PALETTE_GROUPS: PaletteGroup[] = ["Sessions", "Worktrees", "Tasks", "Views", "Actions"];

export interface PaletteItem extends Matchable {
  id: string;
  group: PaletteGroup;
  /** Secondary text, e.g. branch or status. */
  detail?: string;
  /** Shortcut shown on the right, e.g. "⌘N". */
  keys?: string;
  run: () => void;
}

export interface ActionRegistry {
  /** The latest registration for an id wins; the returned function unregisters it. */
  register(id: ActionId, handler: Handler): () => void;
  /** Runs the current handler; false when none is registered or it declined. */
  run(id: ActionId): boolean;
  has(id: ActionId): boolean;
  addPaletteSource(source: () => PaletteItem[]): () => void;
  paletteItems(): PaletteItem[];
}

export function createActionRegistry(): ActionRegistry {
  const handlers = new Map<ActionId, Handler[]>();
  const sources = new Set<() => PaletteItem[]>();
  return {
    register(id, handler) {
      const stack = handlers.get(id) ?? [];
      stack.push(handler);
      handlers.set(id, stack);
      return () => {
        const s = handlers.get(id) ?? [];
        const i = s.lastIndexOf(handler);
        if (i !== -1) s.splice(i, 1);
      };
    },
    run(id) {
      const stack = handlers.get(id);
      const handler = stack?.[stack.length - 1];
      if (!handler) return false;
      return handler() !== false;
    },
    has(id) {
      return !!handlers.get(id)?.length;
    },
    addPaletteSource(source) {
      sources.add(source);
      return () => {
        sources.delete(source);
      };
    },
    paletteItems() {
      return [...sources].flatMap((s) => s());
    },
  };
}

export const ActionsContext = createContext<ActionRegistry | null>(null);

export function useActions(): ActionRegistry {
  const registry = useContext(ActionsContext);
  if (!registry) throw new Error("useActions outside <App>");
  return registry;
}

/** Registers `handler` for `id` while mounted (and `enabled`). The latest handler is always used. */
export function useAction(id: ActionId, handler: Handler, enabled = true) {
  useHandler(useActions(), id, handler, enabled);
}

/** useAction against an explicit registry (for the component that provides the context). */
export function useHandler(registry: ActionRegistry, id: ActionId, handler: Handler, enabled = true) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => (enabled ? registry.register(id, () => ref.current()) : undefined), [registry, id, enabled]);
}

/** Offers palette items while mounted; `source` is called each time the palette opens. */
export function usePaletteSource(source: () => PaletteItem[]) {
  usePaletteSourceIn(useActions(), source);
}

export function usePaletteSourceIn(registry: ActionRegistry, source: () => PaletteItem[]) {
  const ref = useRef(source);
  ref.current = source;
  useEffect(() => registry.addPaletteSource(() => ref.current()), [registry]);
}
