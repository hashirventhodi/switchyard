// A Permission request in the transcript: amber while pending with the exact request in a
// mono block and its answers; afterwards uncoloured with the verdict line.
import type { Entry } from "./schema/Entry";
import type { Verdict } from "./schema/Verdict";
import { suggestGrant } from "./grants";
import { KIND_LABEL, detail } from "./requests";

type RequestEntry = Entry & { type: "request" };

const VERDICT_LINE: Record<Exclude<RequestEntry["state"], "answered" | "alwaysAllowed">, string> = {
  pending: "",
  allowed: "Allowed once.",
  denied: "Skipped. The agent continues without it.",
  cancelled: "Cancelled: the session was stopped.",
};

export function RequestCard(props: {
  entry: RequestEntry;
  answer: (verdict: Verdict) => void;
  answerInChat: () => void;
}) {
  const { entry, answer, answerInChat } = props;
  const { request } = entry;
  const question = request.type === "question";
  const heading = question ? "Question for you" : `Permission request: ${KIND_LABEL[request.type]}`;
  const pending = entry.state === "pending";
  // Always allow: command, network and MCP tool only; file writes never get it.
  const grant = suggestGrant(request);
  return (
    <div className="msg" data-entry="request">
      <div />
      <div role="group" aria-label={heading} className={`perm ${pending ? "" : "resolved"}`} data-pending={pending}>
        <div className="ph">
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
            {question ? (
              <>
                <circle cx="8" cy="8" r="6.5" />
                <path d="M6 6.5a2 2 0 113 1.7c-.7.4-1 .8-1 1.5M8 12h.01" />
              </>
            ) : (
              <>
                <rect x="3" y="7" width="10" height="7" rx="1.5" />
                <path d="M5.5 7V5a2.5 2.5 0 015 0v2" />
              </>
            )}
          </svg>
          {heading}
        </div>
        {question ? <div className="q">{request.question}</div> : <code className="cmd">{detail(request)}</code>}
        {request.type === "fileWrite" && request.diff && <pre className="diff">{request.diff}</pre>}
        {request.type === "mcpTool" && <pre className="diff">{request.input}</pre>}
        {entry.reason && <div className="why">{entry.reason}</div>}
        {pending ? (
          <div className="acts">
            {question ? (
              <>
                {request.options.map((option) => (
                  <button key={option} className="btn sm" onClick={() => answer({ type: "answer", text: option })}>
                    {option}
                  </button>
                ))}
                <button className="btn ghost sm" onClick={answerInChat}>
                  Answer in chat
                </button>
              </>
            ) : (
              <>
                <button className="btn ok sm" onClick={() => answer({ type: "allowOnce" })}>
                  Run once
                </button>
                {grant !== null && (
                  <button
                    className="btn sm"
                    onClick={() => answer({ type: "allowAlways", pattern: request.type === "command" ? grant : null })}
                  >
                    Always allow <code>{grant}</code> in this repo
                  </button>
                )}
                <button className="btn danger sm" onClick={() => answer({ type: "deny" })}>
                  Skip
                </button>
              </>
            )}
          </div>
        ) : (
          <div className="verdict">
            {entry.state === "answered"
              ? `You answered: ${entry.answer ?? ""}`
              : entry.state === "alwaysAllowed"
                ? `Always allowed in this repo: ${entry.answer ?? ""}`
                : VERDICT_LINE[entry.state]}
          </div>
        )}
      </div>
    </div>
  );
}
