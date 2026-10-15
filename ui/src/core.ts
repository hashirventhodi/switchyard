// The renderer's only door to the core: send schema commands, receive schema events.
// The renderer never touches a process, a repository or the file system itself.
import type { Command } from "./schema/Command";
import type { Event } from "./schema/Event";

export interface CoreConnection {
  send(command: Command): void;
  /** Returns an unsubscribe function. */
  subscribe(listener: (event: Event) => void): () => void;
}

/** Name of the Tauri event every core event is emitted on, and of the one IPC command. */
export const CORE_EVENT = "core://event";
export const DISPATCH = "dispatch";

export function insideTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Stand-in when the renderer runs in a plain browser (`pnpm dev`): refuses every command. */
export function detachedCore(): CoreConnection {
  const listeners = new Set<(e: Event) => void>();
  return {
    send: (command) => {
      if (command.type === "attach") return;
      const event: Event = {
        type: "commandRefused",
        command: command.type,
        reason: "No core: the renderer is running outside the Switchyard app",
      };
      listeners.forEach((l) => l(event));
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

/** The core inside the Tauri app, over IPC. */
export async function tauriCore(): Promise<CoreConnection> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const listeners = new Set<(e: Event) => void>();
  await listen<Event>(CORE_EVENT, (msg) => listeners.forEach((l) => l(msg.payload)));
  return {
    send: (command) => {
      invoke(DISPATCH, { command }).catch((err) => console.error("dispatch failed", err));
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
