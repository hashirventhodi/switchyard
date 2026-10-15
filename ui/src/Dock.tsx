// The terminal dock: one read-only Agent log tab per session and any number of shell tabs per
// worktree. Shell tabs are PTYs in the core shown through a terminal emulator; Agent logs are
// derived from the session's transcript (or, with the debug toggle, its raw adapter events).
import { useCallback, useEffect, useRef, useState } from "react";
import { at } from "./chunked";
import { matches } from "./keymap";
import type { Command } from "./schema/Command";
import type { Entry } from "./schema/Entry";
import type { Shell } from "./schema/Shell";
import type { ShellStreams } from "./shellStreams";
import type { SessionView, State } from "./state";
import { AGENT_COLOR, AGENT_SHORT, Status } from "./status";
import type { CreateTerminal } from "./terminal";
import { VirtualList } from "./VirtualList";
import "./dock.css";

export interface DockProps {
  state: State;
  send: (command: Command) => void;
  streams: ShellStreams | null;
  createTerminal: CreateTerminal;
  collapsed: boolean;
  onToggle: () => void;
  /** Where "+ shell" opens a shell when no shell tab is active (the shown session's worktree). */
  currentWorktree: string | null;
  /** Debug view: raw adapter events instead of the Agent log. */
  rawEvents: boolean;
  onRawEvents: (on: boolean) => void;
  onOpenShell: (worktreePath: string) => void;
}

type TabId = `log:${string}` | `shell:${string}`;

export function Dock(props: DockProps) {
  const { state, send, streams, createTerminal, collapsed, onToggle, currentWorktree, rawEvents, onRawEvents, onOpenShell } =
    props;
  const sessions = Object.values(state.sessions);
  const [active, setActive] = useState<TabId | null>(null);
  
  // A newly opened shell becomes the active tab.
  const known = useRef(new Set<string>());
  useEffect(() => {
    const fresh = state.shells.filter((s) => !known.current.has(s.id));
    known.current = new Set(state.shells.map((s) => s.id));
    if (fresh.length) setActive(`shell:${fresh[fresh.length - 1].id}`);
  }, [state.shells]);

  const tabs: TabId[] = [
    ...sessions.map((s) => `log:${s.id}` as const),
    ...state.shells.map((s) => `shell:${s.id}` as const),
  ];
  const shown = active && tabs.includes(active) ? active : (tabs[0] ?? null);
  const activeShell = shown?.startsWith("shell:") ? state.shells.find((s) => `shell:${s.id}` === shown) : undefined;
  const activeSession = shown?.startsWith("log:") ? state.sessions[shown.slice(4)] : undefined;

  const firstWorktree = state.repositories[0]?.worktrees[0]?.path ?? null;
  const shellTarget = activeShell?.worktreePath ?? activeSession?.worktreePath ?? currentWorktree ?? firstWorktree;
  const branchOf = (path: string) =>
    state.repositories.flatMap((r) => r.worktrees).find((w) => w.path === path)?.branch ?? path;

  return (
    <section className={`dock ${collapsed ? "collapsed" : ""}`} aria-label="Terminals">
      <div className="dock-head">
        <div className="tabs" role="tablist" aria-label="Terminals">
          {sessions.map((s) => (
            <button
              key={s.id}
              role="tab"
              aria-selected={shown === `log:${s.id}`}
              className={`dtab ${shown === `log:${s.id}` ? "active" : ""}`}
              title={`Agent log: ${s.branch ?? s.worktreePath}`}
              onClick={() => setActive(`log:${s.id}`)}
            >
              <span className="av" style={{ background: AGENT_COLOR[s.agent] }}>
                {AGENT_SHORT[s.agent]}
              </span>
              <span className="nm">{s.branch ?? s.worktreePath}</span>
              <Status status={s.status} variant="dot" />
            </button>
          ))}
          {state.shells.map((sh) => (
            <span key={sh.id} className={`dtab shell ${shown === `shell:${sh.id}` ? "active" : ""}`}>
              <button role="tab" aria-selected={shown === `shell:${sh.id}`} onClick={() => setActive(`shell:${sh.id}`)}>
                <TermIcon />
                <span className="nm">{branchOf(sh.worktreePath)}</span>
              </button>
              <button
                className="x"
                aria-label={`Close shell ${branchOf(sh.worktreePath)}`}
                title="Close shell"
                onClick={() => send({ type: "closeShell", shellId: sh.id })}
              >
                ×
              </button>
            </span>
          ))}
          <button
            className="dtab add"
            aria-label="+ shell"
            title={shellTarget ? `New shell in ${branchOf(shellTarget)}` : "Add a repository first"}
            disabled={!shellTarget}
            onClick={() => shellTarget && onOpenShell(shellTarget)}
          >
            + shell
          </button>
        </div>
        {activeSession && (
          <label className="raw-toggle" title="Show the raw adapter events (debug)">
            <input type="checkbox" checked={rawEvents} onChange={(e) => onRawEvents(e.target.checked)} />
            Raw events
          </label>
        )}
        <button className="btn ghost sm" onClick={onToggle}>
          {collapsed ? "Show" : "Hide"} <kbd>`</kbd>
        </button>
      </div>
      <div className="term">
        {!shown && <p className="empty">No terminals open. Start a session, or open a shell with + shell.</p>}
        {activeSession && (rawEvents ? <RawEvents session={activeSession} /> : <AgentLog session={activeSession} />)}
        {streams &&
          state.shells.map((sh) => (
            <ShellTab
              key={sh.id}
              shell={sh}
              hidden={shown !== `shell:${sh.id}`}
              send={send}
              streams={streams}
              createTerminal={createTerminal}
            />
          ))}
      </div>
    </section>
  );
}

function ShellTab(props: {
  shell: Shell;
  hidden: boolean;
  send: (command: Command) => void;
  streams: ShellStreams;
  createTerminal: CreateTerminal;
}) {
  const { shell, hidden, send, streams, createTerminal } = props;
  const host = useRef<HTMLDivElement>(null);
  const sendRef = useRef(send);
  sendRef.current = send;
  const fitRef = useRef<() => void>(() => {});

  useEffect(() => {
    const el = host.current!;
    const term = createTerminal(el);
    const stop = streams.listen(shell.id, (data) => term.write(data));
    term.onData((data) => sendRef.current({ type: "writeShell", shellId: shell.id, data }));
    term.onKey((e) => {
      // Ctrl+L clears the scrollback here and still reaches the shell, which redraws its prompt.
      if (e.type === "keydown" && matches(e, "terminal.clear")) term.clear();
      return true;
    });
    let last = "";
    const fit = () => {
      const size = term.fit();
      if (!size || `${size.cols}x${size.rows}` === last) return;
      last = `${size.cols}x${size.rows}`;
      sendRef.current({ type: "resizeShell", shellId: shell.id, ...size });
    };
    fitRef.current = fit;
    fit();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(fit);
    observer?.observe(el);
    term.focus();
    return () => {
      observer?.disconnect();
      stop();
      term.dispose();
    };
  }, [shell.id, streams, createTerminal]);

  useEffect(() => {
    if (!hidden) fitRef.current();
  }, [hidden]);

  return <div ref={host} className="shell-term" data-terminal hidden={hidden} aria-label={`Shell ${shell.worktreePath}`} />;
}

/** What one transcript entry contributes to the Agent log; prose belongs to the transcript. */
function logLines(entry: Entry | undefined): { text: string; tone?: "cmd" | "err" | "dim" }[] {
  if (!entry) return [];
  if (entry.type === "system") return [{ text: `# ${entry.text}`, tone: "dim" }];
  if (entry.type !== "tool") return [];
  const head = entry.tool === "Bash" ? `$ ${entry.summary}` : `● ${entry.tool} ${entry.summary}`;
  const mark = entry.state === "running" ? " …" : entry.state === "failed" ? " ✗" : "";
  const lines: { text: string; tone?: "cmd" | "err" | "dim" }[] = [{ text: head + mark, tone: "cmd" }];
  if (entry.output) lines.push({ text: entry.output, tone: entry.state === "failed" ? "err" : undefined });
  return lines;
}

function AgentLog({ session }: { session: SessionView }) {
  const { transcript } = session;
  const name = session.branch ?? session.worktreePath;
  // Prose rows take no space: the log is the same list as the transcript, filtered by height.
  const estimate = useCallback((i: number) => {
    const lines = logLines(at(transcript, i));
    return lines.reduce((h, l) => h + 18 * Math.min(40, l.text.split("\n").length), 0);
  }, [transcript]);
  return (
    <VirtualList
      key={session.id}
      className="agent-log"
      label={`Agent log ${name}`}
      count={transcript.length}
      version={transcript}
      estimate={estimate}
      renderRow={(i) =>
        logLines(at(transcript, i)).map((l, n) => (
          <div key={n} className={`l ${l.tone ?? ""}`}>
            {l.text}
          </div>
        ))
      }
    />
  );
}

function RawEvents({ session }: { session: SessionView }) {
  const { rawEvents } = session;
  const name = session.branch ?? session.worktreePath;
  const estimate = useCallback(() => 18, []);
  return (
    <VirtualList
      key={session.id}
      className="agent-log raw"
      label={`Raw events ${name}`}
      count={rawEvents.length}
      version={rawEvents}
      estimate={estimate}
      renderRow={(i) => (
        <div className="l">
          <span className="n">{i}</span> {at(rawEvents, i)}
        </div>
      )}
    />
  );
}

function TermIcon() {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="1.5">
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" />
      <path d="M4.5 6.5l2 1.5-2 1.5M8 10h3" />
    </svg>
  );
}
