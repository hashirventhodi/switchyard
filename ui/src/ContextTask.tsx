// Context's Task section: the Task the session is linked to, with its checklist ticked
// locally (never written to the tracker), or "Pick one" when none is linked.
import { useEffect } from "react";
import type { Command } from "./schema/Command";
import { taskById, type SessionView, type State } from "./state";
import { COLUMN_LABEL, SOURCE_LABEL } from "./tasks";

export function ContextTask(props: {
  state: State;
  session: SessionView;
  send: (command: Command) => void;
  /** Open Tasks to pick the session's task; `relink` replaces the current one. */
  onPickTask?: (sessionId: string, relink: boolean) => void;
}) {
  const { state, session, send, onPickTask = () => {} } = props;
  const linked = state.taskLinks[session.id];
  const task = taskById(state, linked);
  const known = !!state.tasks[session.repositoryId];
  useEffect(() => {
    if (linked && !known) send({ type: "readTasks", repositoryId: session.repositoryId });
  }, [send, linked, known, session.repositoryId]);

  return (
    <section className="ctx task" aria-label="Task">
      <h4>
        Task <span>{task ? SOURCE_LABEL[task.source] : "none"}</span>
      </h4>
      {task ? (
        <>
          <div className="tkt">
            <div className="id">
              <span>{task.key}</span>
              <span>·</span>
              <span>{COLUMN_LABEL[task.column]}</span>
              <button className="btn ghost sm" onClick={() => onPickTask(session.id, true)}>
                Change
              </button>
            </div>
            <div className="tt">{task.title}</div>
            {task.description && <div className="ds">{task.description}</div>}
          </div>
          {task.checklist.map((item, i) => (
            <label key={`${i}:${item.text}`} className={`chk ${item.done ? "done" : ""}`}>
              <input
                type="checkbox"
                checked={item.done}
                onChange={(e) => send({ type: "tickTask", taskId: task.id, item: i, done: e.target.checked })}
              />
              <span>{item.text}</span>
            </label>
          ))}
        </>
      ) : linked ? (
        <p className="empty">Reading the linked task…</p>
      ) : (
        <p className="empty">
          No task linked.{" "}
          <button className="btn ghost sm" onClick={() => onPickTask(session.id, false)}>
            Pick one
          </button>
        </p>
      )}
    </section>
  );
}
