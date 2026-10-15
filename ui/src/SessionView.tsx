// The Session view: header with status and Stop, the summary strip, the virtualised
// transcript and the composer.
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import type { PermissionMode } from "./schema/PermissionMode";
import type { Policy } from "./schema/Policy";
import { useAction } from "./actions";
import { at } from "./chunked";
import { keysLabel, matches } from "./keymap";
import { RequestCard } from "./RequestCard";
import type { Command } from "./schema/Command";
import type { Entry } from "./schema/Entry";
import type { Verdict } from "./schema/Verdict";
import type { PendingRequest, SessionView as Session } from "./state";
import { sessionCost } from "./spend";
import { AGENT_LABEL, AGENT_SHORT, MODE_LABEL, ModeWarning, Status, withinCeiling } from "./status";
import { VirtualList } from "./VirtualList";

export interface SessionViewProps {
  session: Session;
  send: (command: Command) => void;
  /** Its pending Permission requests, in transcript order. */
  pending: PendingRequest[];
  /** Changes when the view should scroll to the first pending request (⌘J, inbox). */
  revealRequest?: number;
  /** Where the worktree's branch stands against base (↓N behind), beside the title. */
  branchState?: ReactNode;
  /** Text drafted elsewhere (Ask agent in review) for the composer; a new seq replaces it. */
  draft?: Draft;
  /** The review pane beside this one is hidden (⌘⇧D). */
  reviewHidden?: boolean;
  onToggleReview?: () => void;
  /** Below 860 px the review pane replaces this one while shown. */
  showChanges?: boolean;
  onToggleChanges?: () => void;
  /** The repository's policy: its ceiling caps the composer's mode segment. */
  policy?: Policy;
}

export interface Draft {
  text: string;
  seq: number;
}
const NO_DRAFT: Draft = { text: "", seq: 0 };

export function SessionView(props: SessionViewProps) {
  const { session, send, pending, revealRequest, draft = NO_DRAFT, reviewHidden = false, onToggleReview, policy } = props;
  useEffect(() => send({ type: "readPolicy", repositoryId: session.repositoryId }), [session.repositoryId, send]);
  const composer = useRef<HTMLTextAreaElement>(null);
  const active = session.status === "running" || session.status === "waiting";
  const stop = () => send({ type: "stopSession", sessionId: session.id });
  // Paused and Finished sessions continue where they left off (never on their own).
  const resumable = session.status === "idle" || session.status === "done";
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
        {props.branchState}
        <Status status={session.status} />
        {!reviewHidden && props.onToggleChanges && (
          <button className="btn sm mobile-only" onClick={props.onToggleChanges}>
            {props.showChanges ? "Agent" : "Changes"}
          </button>
        )}
        {active && (
          <button className="btn sm" title="Stop the agent (Esc)" onClick={stop}>
            Stop <kbd>Esc</kbd>
          </button>
        )}
        {resumable && (
          <button className="btn sm primary" onClick={() => send({ type: "resumeSession", sessionId: session.id })}>
            Resume
          </button>
        )}
        <button
          className="btn sm ghost"
          title="Write the transcript as JSONL, one entry per line"
          onClick={() => send({ type: "exportTranscript", sessionId: session.id })}
        >
          Export transcript
        </button>
        {onToggleReview && (
          <button
            className="btn ghost sm hide-sm"
            title={`Toggle review pane (${keysLabel("review.togglePane")})`}
            onClick={onToggleReview}
          >
            {reviewHidden ? "Show changes" : "Hide changes"}
          </button>
        )}
      </div>
      <SummaryStrip session={session} />
      <Transcript
        session={session}
        answer={(requestId, verdict) => send({ type: "answerRequest", sessionId: session.id, requestId, verdict })}
        answerInChat={() => composer.current?.focus()}
        reveal={revealRequest === undefined || !pending.length ? undefined : { index: pending[0].index, seq: revealRequest }}
      />
      <Composer
        session={session}
        send={send}
        pending={pending}
        input={composer}
        draft={draft}
        ceiling={policy?.ceiling ?? null}
      />
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
      {facts.modeWarning && <ModeWarning text={facts.modeWarning} />}
      {facts.classifierDenials > 0 && (
        <div className="c denials" title="Actions the CLI's classifier refused; noted in the transcript">
          <div className="l">Classifier</div>
          <div className="v">{facts.classifierDenials} denied by classifier</div>
        </div>
      )}
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

const SEGMENT_MODES: PermissionMode[] = ["askFirst", "autoEdit", "fullAuto"];

/** The composer's Permission-mode segment. Explore is chosen only at start, so an Explore
 * session has none; modes above the repository's ceiling are not offered. */
function ModeSegment(props: { session: Session; send: (command: Command) => void; ceiling: PermissionMode | null }) {
  const { session, send, ceiling } = props;
  if (session.permissionMode === "explore") return null;
  return (
    <div className="seg mode-seg" role="radiogroup" aria-label="Session permission mode">
      {SEGMENT_MODES.filter((m) => withinCeiling(m, ceiling)).map((m) => (
        <label key={m} className={session.permissionMode === m ? "on" : ""}>
          <input
            type="radio"
            name={`mode-${session.id}`}
            value={m}
            checked={session.permissionMode === m}
            onChange={() => send({ type: "setPermissionMode", sessionId: session.id, permissionMode: m })}
          />
          {MODE_LABEL[m]}
        </label>
      ))}
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

const ESTIMATE: Record<Entry["type"], number> = { user: 64, agent: 64, tool: 26, system: 26, request: 130 };

function Transcript(props: {
  session: Session;
  answer: (requestId: string, verdict: Verdict) => void;
  answerInChat: () => void;
  reveal?: { index: number; seq: number };
}) {
  const { session, answer, answerInChat, reveal } = props;
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
      reveal={reveal}
      renderRow={(i) => (
        <EntryRow
          entry={at(transcript, i)!}
          agent={AGENT_SHORT[session.agent]}
          answer={answer}
          answerInChat={answerInChat}
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

function EntryRow(props: {
  entry: Entry;
  agent: string;
  answer: (requestId: string, verdict: Verdict) => void;
  answerInChat: () => void;
  toggled: boolean;
  onToggle: () => void;
}) {
  const { entry, agent, answer, answerInChat, toggled, onToggle } = props;
  switch (entry.type) {
    case "request":
      return (
        <RequestCard entry={entry} answer={(verdict) => answer(entry.requestId, verdict)} answerInChat={answerInChat} />
      );
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

function Composer(props: {
  session: Session;
  send: (command: Command) => void;
  pending: PendingRequest[];
  input: RefObject<HTMLTextAreaElement>;
  /** Text something else (Ask agent in review) drafted for the user to finish and send. */
  draft: Draft;
  ceiling: PermissionMode | null;
}) {
  const { session, send, pending, input, draft, ceiling } = props;
  const [text, setText] = useState("");
  useEffect(() => {
    if (draft.seq === 0) return;
    setText(draft.text);
    input.current?.focus();
  }, [draft, input]);
  // A pending question is answered from here; any other pending request is answered on its card.
  const question = pending.some((r) => r.request.type === "question");
  const blocked = pending.length > 0 && !question;
  const ready = !blocked && (session.status === "waiting" || session.status === "done" || session.status === "failed");
  const submit = () => {
    if (!ready || !text.trim()) return;
    send({ type: "sendPrompt", sessionId: session.id, text });
    setText("");
  };
  const placeholder = blocked
    ? "Answer the request above first."
    : question
      ? "Answer the agent's question…"
      : ready
    ? "Reply to the agent…"
    : session.status === "running"
      ? "The agent is working…"
      : "Paused. Resume to continue.";
  return (
    <div className="composer">
      <ModeSegment session={session} send={send} ceiling={ceiling} />
      <textarea
        ref={input}
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
