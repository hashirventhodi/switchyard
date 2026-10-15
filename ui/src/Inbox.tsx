// The Needs-you inbox: every session blocked on the user and what it wants. A pending
// permission request can be answered right here; anything else opens the session.
import { wants } from "./requests";
import type { Verdict } from "./schema/Verdict";
import { waitingSessions, type State } from "./state";
import { AGENT_LABEL } from "./status";

export function Inbox(props: {
  state: State;
  onOpenSession: (id: string) => void;
  answer: (sessionId: string, requestId: string, verdict: Verdict) => void;
  onClose: () => void;
}) {
  const { state, onOpenSession, answer, onClose } = props;
  const waiting = waitingSessions(state);
  return (
    <>
      <div className="inbox-scrim" onClick={onClose} />
      <div className="inbox" role="dialog" aria-label="Needs you">
        <div className="inbox-h">
          Needs you{" "}
          <small>
            {waiting.length} session{waiting.length === 1 ? "" : "s"}
          </small>
        </div>
        {waiting.length === 0 ? (
          <div className="inbox-empty">Nothing is waiting on you.</div>
        ) : (
          <ul className="inbox-list">
            {waiting.map((s) => {
              const request = state.requests[s.id]?.[0];
              const name = s.branch ?? s.worktreePath;
              return (
                <li key={s.id} className="inbox-item">
                  <button className="open" aria-label={`Open ${name}`} onClick={() => onOpenSession(s.id)}>
                    <span className="top">
                      <span>{AGENT_LABEL[s.agent]}</span>
                      <span>·</span>
                      <code>{name}</code>
                    </span>
                    <span className="what">{request ? wants(request.request) : "Waiting for your next prompt"}</span>
                  </button>
                  {request && request.request.type !== "question" && (
                    <span className="acts">
                      <button className="btn ok sm" onClick={() => answer(s.id, request.requestId, { type: "allowOnce" })}>
                        Run once
                      </button>
                      <button className="btn danger sm" onClick={() => answer(s.id, request.requestId, { type: "deny" })}>
                        Skip
                      </button>
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </>
  );
}
