// Context's Archived sessions: the Repository's replaced (or worktree-removed) Sessions,
// keyed by branch and session id, each readable and exportable; and the retention
// settings that prune them (the newest N per repository, none older than D days).
import { useEffect, useState } from "react";
import { useAction } from "./actions";
import type { Command } from "./schema/Command";
import type { Entry } from "./schema/Entry";
import type { SessionView, State } from "./state";
import { STATUS_LABEL } from "./status";
import { VirtualList } from "./VirtualList";

export function ContextArchive({ state, session, send }: { state: State; session: SessionView; send: (command: Command) => void }) {
  const list = [...(state.archived[session.repositoryId] ?? [])].reverse(); // newest first
  const [reading, setReading] = useState<string | null>(null);
  const open = reading ? list.find((a) => a.session.id === reading) : undefined;
  const transcript = state.archivedTranscript?.sessionId === reading ? state.archivedTranscript.entries : undefined;
  return (
    <section className="ctx archive" role="region" aria-label="Archived sessions">
      <h4>
        Archived sessions <span className="n">{list.length}</span>
      </h4>
      {list.length === 0 ? (
        <p className="empty">Sessions you stop and replace, or whose worktree is removed, are kept here.</p>
      ) : (
        <ul className="archive-list">
          {list.map(({ session: s, archivedAtMs, entries }) => (
            <li key={s.id}>
              <code className="br">{s.branch ?? s.worktreePath}</code>
              <span className="meta">
                {s.id} · {STATUS_LABEL[s.status]} · {entries} entries · archived {new Date(archivedAtMs).toLocaleDateString()}
              </span>
              <button
                className="btn sm ghost"
                aria-label={`Read ${s.id}`}
                onClick={() => {
                  setReading(s.id);
                  send({ type: "readArchivedSession", sessionId: s.id });
                }}
              >
                Read
              </button>
            </li>
          ))}
        </ul>
      )}
      <Retention state={state} send={send} />
      {open && (
        <ArchivedReader
          title={open.session.branch ?? open.session.worktreePath}
          entries={transcript}
          onExport={() => send({ type: "exportTranscript", sessionId: open.session.id })}
          onClose={() => setReading(null)}
        />
      )}
    </section>
  );
}

function Retention({ state, send }: { state: State; send: (command: Command) => void }) {
  const settings = state.settings;
  const [keep, setKeep] = useState("");
  const [days, setDays] = useState("");
  useEffect(() => {
    if (!settings) return;
    setKeep(String(settings.archiveKeepPerRepository));
    setDays(String(settings.archiveKeepDays));
  }, [settings]);
  if (!settings) return null;
  const n = Number(keep);
  const d = Number(days);
  const valid = Number.isInteger(n) && n >= 1 && Number.isInteger(d) && d >= 1;
  const changed = n !== settings.archiveKeepPerRepository || d !== settings.archiveKeepDays;
  return (
    <form
      className="retention"
      onSubmit={(e) => {
        e.preventDefault();
        if (valid) send({ type: "updateSettings", settings: { archiveKeepPerRepository: n, archiveKeepDays: d } });
      }}
    >
      <span>Keep the newest</span>
      <input aria-label="Keep per repository" type="number" min={1} value={keep} onChange={(e) => setKeep(e.target.value)} />
      <span>per repository, none older than</span>
      <input aria-label="Keep days" type="number" min={1} value={days} onChange={(e) => setDays(e.target.value)} />
      <span>days</span>
      <button className="btn sm" type="submit" aria-label="Save retention" disabled={!valid || !changed}>
        Save
      </button>
    </form>
  );
}

const ESTIMATE: Record<Entry["type"], number> = { user: 48, agent: 48, tool: 26, system: 26, request: 40 };

function ArchivedReader(props: { title: string; entries?: Entry[]; onExport: () => void; onClose: () => void }) {
  const { title, entries, onExport, onClose } = props;
  useAction("confirm.cancel", onClose);
  return (
    <div className="overlay-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet archive-reader" role="dialog" aria-modal="true" aria-label={`Archived session ${title}`}>
        <div className="modal-h">
          <h2>
            Archived session <code>{title}</code>
          </h2>
          <button type="button" className="iconbtn" aria-label="Close" onClick={onClose}>
            ✕
          </button>
        </div>
        {entries ? (
          <VirtualList
            className="transcript archived"
            label="Archived transcript"
            count={entries.length}
            version={entries}
            estimate={(i) => ESTIMATE[entries[i].type]}
            renderRow={(i) => <ArchivedEntry entry={entries[i]} />}
          />
        ) : (
          <p className="empty">Reading the transcript…</p>
        )}
        <div className="modal-f">
          <span className="hint">Read only: an archived session cannot be resumed.</span>
          <button className="btn" onClick={onExport}>
            Export transcript
          </button>
        </div>
      </div>
    </div>
  );
}

function ArchivedEntry({ entry }: { entry: Entry }) {
  switch (entry.type) {
    case "user":
    case "agent":
      return (
        <div className={`msg ${entry.type}`}>
          <div className="av">{entry.type === "agent" ? "AI" : "You"}</div>
          <div className="text">{entry.text}</div>
        </div>
      );
    case "system":
      return <div className="sys">{entry.text}</div>;
    case "tool":
      return (
        <div className="tlrow collapsed">
          <span className="v">{entry.tool}</span> <code>{entry.summary}</code>
        </div>
      );
    case "request":
      return (
        <div className="sys">
          Request ({entry.request.type}): {entry.state}
          {entry.answer ? ` — ${entry.answer}` : ""}
        </div>
      );
  }
}
