// The terminal emulator behind a shell tab. The dock talks to this interface only; the app
// uses xterm.js and renderer tests pass a fake.
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

export interface TerminalHandle {
  write(data: string): void;
  /** Clear the screen and scrollback, keeping the cursor line. */
  clear(): void;
  /** Fit the terminal to its element; returns its size in cells (null when not laid out). */
  fit(): { cols: number; rows: number } | null;
  /** Bytes the user typed or pasted, to send to the shell. */
  onData(listener: (data: string) => void): void;
  /** Sees each key event before the emulator does; returning false swallows it. */
  onKey(handler: (e: KeyboardEvent) => boolean): void;
  focus(): void;
  dispose(): void;
}

export type CreateTerminal = (element: HTMLElement) => TerminalHandle;

function cssVar(name: string, fallback: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

export const xtermTerminal: CreateTerminal = (element) => {
  const term = new Terminal({
    fontFamily: cssVar("--font-mono", "monospace"),
    fontSize: 12,
    lineHeight: 1.25,
    cursorBlink: true,
    scrollback: 5000,
    theme: {
      background: cssVar("--bg-1", "#111"),
      foreground: cssVar("--fg-1", "#ddd"),
      cursor: cssVar("--fg", "#fff"),
    },
  });
  const fitter = new FitAddon();
  term.loadAddon(fitter);
  term.open(element);
  return {
    write: (data) => term.write(data),
    clear: () => term.clear(),
    fit: () => {
      if (!element.offsetWidth || !element.offsetHeight) return null;
      fitter.fit();
      return { cols: term.cols, rows: term.rows };
    },
    onData: (listener) => {
      term.onData(listener);
    },
    onKey: (handler) => term.attachCustomKeyEventHandler(handler),
    focus: () => term.focus(),
    dispose: () => term.dispose(),
  };
};
