// The Session view: header with status and Stop, the summary strip, the virtualised
// transcript and the composer.
import { useEffect, useState } from "react";
import { useAction } from "./actions";
import { at } from "./chunked";
import { matches } from "./keymap";
import type { Command } from "./schema/Command";
import type { Entry } from "./schema/Entry";
import type { SessionView as Session } from "./state";
import { sessionCost } from "./spend";
import { AGENT_LABEL, AGENT_SHORT, MODE_LABEL, Status } from "./status";
import { VirtualList } from "./VirtualList";

export function SessionView({ session, send }: { session: Session; send: (command: Command) => void }) {
  const active = session.status === "running" || session.status === "waiting";
  const stop = () => send({ type: "stopSession", sessionId: session.id });
  // Esc's last layer: interrupt the agent of the session on screen.
  useAction("session.interrupt", () => (active ? stop() : false));
  const name = session.branch ?? session.worktreePath;

  return (
    <section className="session" role="region" aria-label={`Session ${name}`}>
      <div className="lane-head">
        <span className="av" style={{ background: "var(--ag-claude)" }}>
          {AGENT_SHORT[session.agent]}
        </span>
        <div className="title">
          <h1>{name}</h1>
        </div>
        <Status status={session.status} />
        {active && (
          <button className="btn sm" title="Stop the agent (Esc)" onClick={stop}>
            Stop <kbd>Esc</kbd>
          </button>
        )}
      </div>
      <SummaryStrip session={session} />
      <Transcript session={session} />
      <Composer session={session} send={send} />
    </section>
  );
}

function SummaryStrip({ session }: { session: Session }) {
  const { facts } = session;
  const cliMode = facts.cliPermissionMode ? ` (CLI: ${facts.cliPermissionMode})` : "";
  const [cost, costDetail] = sessionCost(session);
  const cells: [string, string, string?][] = [
    ["Agent", AGENT_LABEL[session.agent]],
    ["Model", facts.model ?? "—"],
    ["CLI", facts.cliVersion ?? "—"],
    ["Worktree", session.worktreePath],
    ["Mode", MODE_LABEL[session.permissionMode] + cliMode],
    ["Turns", String(facts.turns)],
    ["Cost", cost, costDetail],
  ];
  return (
    <div className="summary" aria-label="Summary">
      {cells.map(([label, value, detail]) => (
        <div className="c" key={label} title={detail ?? value}>
          <div className="l">{label}</div>
          <div className="v">{value}</div>
        </div>
      ))}
      <div className="c">
        <div className="l">Elapsed</div>
        <div className="v">
          <Elapsed since={session.startedAtMs} />
        </div>
      </div>
    </div>
  );
}

function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const s = Math.max(0, Math.floor((now - since) / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return <>{h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`}</>;
}

const ESTIMATE: Record<Entry["type"], number> = { user: 64, agent: 64, tool: 26, system: 26 };

function Transcript({ session }: { session: Session }) {
  // Expanded/collapsed tool rows the user toggled, by transcript index.
  const [toggled, setToggled] = useState<Set<number>>(() => new Set());
  const { transcript } = session;
  const estimate = (i: number) => {
    const entry = at(transcript, i);
    if (entry?.type === "tool" && isOpen(entry, toggled.has(i))) return 26 + 120;
    return entry ? ESTIMATE[entry.type] : 26;
  };
  return (
    <VirtualList
      key={session.id}
      className="transcript"
      label="Transcript"
      count={transcript.length}
      version={transcript}
      estimate={estimate}
      renderRow={(i) => (
        <EntryRow
          entry={at(transcript, i)!}
          agent={AGENT_SHORT[session.agent]}
          toggled={toggled.has(i)}
          onToggle={() =>
            setToggled((t) => {
              const next = new Set(t);
              if (!next.delete(i)) next.add(i);
              return next;
            })
          }
        />
      )}
    />
  );
}

/** Shell runs and failures start expanded; other tool rows start collapsed. */
function isOpen(entry: Entry & { type: "tool" }, toggled: boolean): boolean {
  const byDefault = entry.tool === "Bash" || entry.state === "failed";
  return toggled ? !byDefault : byDefault;
}

function EntryRow(props: { entry: Entry; agent: string; toggled: boolean; onToggle: () => void }) {
  const { entry, agent, toggled, onToggle } = props;
  switch (entry.type) {
    case "user":
    case "agent":
      return (
        <div className={`msg ${entry.type}`} data-entry={entry.type}>
          <div className="av" style={entry.type === "agent" ? { background: "var(--ag-claude)" } : undefined}>
            {entry.type === "agent" ? agent : "You"}
          </div>
          <div className="text">{entry.text}</div>
        </div>
      );
    case "system":
      return (
        <div className="sys" data-entry="system">
          {entry.text}
        </div>
      );
    case "tool": {
      const open = isOpen(entry, toggled);
      return (
        <div className={`tlrow ${open ? "" : "collapsed"}`} data-entry="tool" data-open={open}>
          <button className="tl" aria-expanded={open} onClick={onToggle}>
            <span className={`v ${entry.state === "failed" ? "bad" : ""}`}>{entry.tool}</span>
            <code>{entry.summary}</code>
            <span className={`st ${entry.state}`}>{entry.state === "running" ? "…" : entry.state === "ok" ? "ok" : "failed"}</span>
          </button>
          {open && entry.output != null && <pre className="tool-out">{entry.output}</pre>}
        </div>
      );
    }
  }
}

function Composer({ session, send }: { session: Session; send: (command: Command) => void }) {
  const [text, setText] = useState("");
  const ready = session.status === "waiting" || session.status === "done" || session.status === "failed";
  const submit = () => {
    if (!ready || !text.trim()) return;
    send({ type: "sendPrompt", sessionId: session.id, text });
    setText("");
  };
  const placeholder = ready
    ? "Reply to the agent…"
    : session.status === "running"
      ? "The agent is working…"
      : "Paused. Resume arrives later.";
  return (
    <div className="composer">
      <textarea
        aria-label="Message the agent"
        rows={2}
        value={text}
        placeholder={placeholder}
        disabled={!ready}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (matches(e, "composer.send")) {
            e.preventDefault();
            submit();
          }
        }}
      />
      <button className="btn primary" disabled={!ready || !text.trim()} onClick={submit}>
        Send
      </button>
    </div>
  );
}
