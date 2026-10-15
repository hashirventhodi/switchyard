// Shell output, kept outside React state: it is a byte stream for a terminal emulator, not
// something to render. Buffers each open shell's recent output from the moment the core
// reports it, so a tab drawn later (or redrawn) replays what it missed.
import type { CoreConnection } from "./core";

const KEEP_CHARS = 256 * 1024;

export interface ShellStreams {
  /** Replays the shell's buffered output to `listener`, then streams new output to it. */
  listen(shellId: string, listener: (data: string) => void): () => void;
  dispose(): void;
}

export function shellStreams(core: CoreConnection): ShellStreams {
  const buffers = new Map<string, string>();
  const listeners = new Map<string, Set<(data: string) => void>>();
  const unsubscribe = core.subscribe((event) => {
    switch (event.type) {
      case "attached":
        // The core replays every open shell's recent output next.
        buffers.clear();
        break;
      case "shellOutput": {
        const kept = (buffers.get(event.shellId) ?? "") + event.data;
        buffers.set(event.shellId, kept.length > KEEP_CHARS ? kept.slice(-KEEP_CHARS) : kept);
        listeners.get(event.shellId)?.forEach((l) => l(event.data));
        break;
      }
      case "shellClosed":
        buffers.delete(event.shellId);
        break;
    }
  });
  return {
    listen(shellId, listener) {
      const buffered = buffers.get(shellId);
      if (buffered) listener(buffered);
      const set = listeners.get(shellId) ?? new Set();
      set.add(listener);
      listeners.set(shellId, set);
      return () => set.delete(listener);
    },
    dispose: unsubscribe,
  };
}
