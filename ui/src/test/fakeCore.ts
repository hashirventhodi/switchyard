import type { Command } from "../schema/Command";
import type { Event } from "../schema/Event";
import type { CoreConnection } from "../core";

/** A core stand-in for renderer tests: records commands, emits whatever events a test pushes. */
export function fakeCore() {
  const sent: Command[] = [];
  const listeners = new Set<(e: Event) => void>();
  const core: CoreConnection = {
    send: (command) => {
      sent.push(command);
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  return {
    core,
    sent,
    emit: (...events: Event[]) => events.forEach((e) => listeners.forEach((l) => l(e))),
  };
}
