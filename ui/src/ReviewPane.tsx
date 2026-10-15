// The review pane beside the transcript. Its Changes tab lists the worktree's uncommitted
// files grouped Conflicts / Modified / Added / Deleted, shows the selected file's hunks,
// takes file decisions (a / r, j / k to move) and commits accepted files whole.
import { useEffect, useRef, useState } from "react";
import { useAction, usePaletteSource } from "./actions";
import { at } from "./chunked";
import { keysLabel, matches } from "./keymap";
import type { ChangedFile } from "./schema/ChangedFile";
import type { Command } from "./schema/Command";
import type { Decision } from "./schema/Decision";
import type { FileStatus } from "./schema/FileStatus";
import type { Hunk } from "./schema/Hunk";
import type { WorktreeFacts } from "./schema/WorktreeFacts";
import type { SessionView } from "./state";

const LETTER: Record<FileStatus, string> = { modified: "M", added: "A", deleted: "D", renamed: "R", conflicted: "!" };
const GROUPS: [string, (f: ChangedFile) => boolean][] = [
  ["Conflicts", (f) => f.status === "conflicted"],
  ["Modified", (f) => f.status === "modified" || f.status === "renamed"],
  ["Added", (f) => f.status === "added"],
  ["Deleted", (f) => f.status === "deleted"],
];

/** Files in the order the list shows them (group by group). */
export function reviewOrder(files: ChangedFile[]): ChangedFile[] {
  return GROUPS.flatMap(([, test]) => files.filter(test));
}

/** The session's title: the first line of its first prompt. */
export function sessionTitle(session: SessionView): string {
  for (let i = 0; i < session.transcript.length; i++) {
    const entry = at(session.transcript, i);
    if (entry?.type === "user") {
      const line = entry.text.split("\n")[0].trim();
      if (line) return line.length > 72 ? `${line.slice(0, 71)}…` : line;
    }
  }
  return session.branch ?? "Update files";
}

/** A commit message from the session title and the accepted paths. */
export function draftCommitMessage(title: string, accepted: ChangedFile[]): string {
  const verb = (f: ChangedFile) =>
    f.status === "added" ? `add ${f.path}` : f.status === "deleted" ? `remove ${f.path}` : f.status === "renamed" ? `rename ${f.oldPath} to ${f.path}` : `update ${f.path}`;
  return `${title}\n\n${accepted.map((f) => `- ${verb(f)}`).join("\n")}`;
}

export interface ReviewPaneProps {
  session: SessionView;
  files: ChangedFile[] | undefined;
  facts: WorktreeFacts | undefined;
  send: (command: Command) => void;
  expanded: boolean;
  onToggleExpand: () => void;
  /** Put text in the composer for the user to finish and send. */
  onAsk: (text: string) => void;
}

export function ReviewPane({ session, files, facts, send, expanded, onToggleExpand, onAsk }: ReviewPaneProps) {
  const worktreePath = session.worktreePath;
  // Re-read whenever the agent may have changed something: on open and on every status change.
  useEffect(() => {
    send({ type: "readChanges", worktreePath });
  }, [send, worktreePath, session.status]);

  const ordered = reviewOrder(files ?? []);
  const [activePath, setActivePath] = useState<string | null>(null);
  const active = ordered.find((f) => f.path === activePath) ?? ordered[0];
  const [committing, setCommitting] = useState(false);

  const accepted = ordered.filter((f) => f.decision === "accept");
  const rejected = ordered.filter((f) => f.decision === "reject");
  const conflicts = ordered.filter((f) => f.status === "conflicted");
  const canCommit = accepted.length > 0 && conflicts.length === 0;

  const decide = (file: ChangedFile, decision: Decision, advance = false) => {
    if (file.status === "conflicted") return false;
    const next = file.decision === decision ? null : decision;
    send({ type: "decideFile", worktreePath, path: file.path, decision: next });
    // Deciding by key moves on to the next file, so a / r / a / r walks the list.
    const i = ordered.indexOf(file);
    if (advance && next && i < ordered.length - 1) setActivePath(ordered[i + 1].path);
  };
  const move = (step: number) => {
    if (!active) return false;
    const i = ordered.indexOf(active);
    setActivePath(ordered[(i + step + ordered.length) % ordered.length].path);
  };
  const reviewing = !!active && !committing;
  useAction("review.nextFile", () => move(1), reviewing);
  useAction("review.prevFile", () => move(-1), reviewing);
  useAction("review.accept", () => active && decide(active, "accept", true), reviewing);
  useAction("review.reject", () => active && decide(active, "reject", true), reviewing);
  usePaletteSource(() => [
    ...(canCommit && !committing
      ? [{ id: "review.commit", group: "Actions" as const, title: "Commit accepted…", run: () => setCommitting(true) }]
      : []),
    {
      id: "review.expand",
      group: "Actions" as const,
      title: expanded ? "Show agent beside review" : "Expand review pane",
      run: onToggleExpand,
    },
  ]);

  return (
    <section className="pane changes-pane" role="region" aria-label="Review">
      <div className="pane-head">
        <div className="tabs" role="tablist">
          <button className="tab active" role="tab" aria-selected="true">
            Changes <span className="n">{ordered.length}</span>
          </button>
        </div>
        <div className="spacer" />
        <button className="btn ghost sm" title="Expand review pane" aria-pressed={expanded} onClick={onToggleExpand}>
          {expanded ? "Show agent" : "Expand"}
        </button>
      </div>
      {files === undefined ? (
        <div className="changes-body">
          <p className="diff-empty">Reading changes…</p>
        </div>
      ) : ordered.length === 0 ? (
        <div className="changes-body">
          <p className="diff-empty">
            {session.status === "done"
              ? `Working tree clean. ${commitsReady(facts?.ahead ?? 0)}`
              : "No uncommitted changes in this worktree."}
          </p>
        </div>
      ) : (
        <div className="changes-body">
          <div className="filelist">
            {GROUPS.map(([name, test]) => {
              const list = ordered.filter(test);
              if (!list.length) return null;
              return (
                <div key={name} role="group" aria-label={name}>
                  <div className={`group-h ${name === "Conflicts" ? "bad" : ""}`}>
                    <span>{name}</span>
                    <span className="n">{list.length}</span>
                  </div>
                  {list.map((f) => (
                    <FileRow key={f.path} file={f} active={f === active} onSelect={() => setActivePath(f.path)} />
                  ))}
                </div>
              );
            })}
          </div>
          {active && (
            <FileDiff
              file={active}
              onDecide={(d) => decide(active, d)}
              onAsk={() => onAsk(`Explain the change in ${active.path} and whether it needs a test.`)}
            />
          )}
          <div className="commit-box">
            {committing ? (
              <CommitBox
                draft={draftCommitMessage(sessionTitle(session), accepted)}
                count={accepted.length}
                branch={session.branch}
                disabled={!canCommit}
                onCommit={(message) => {
                  send({ type: "commitAccepted", worktreePath, message });
                  setCommitting(false);
                }}
                onCancel={() => setCommitting(false)}
              />
            ) : (
              <div className="review-foot">
                <span className="hint">
                  <span className="ok">{accepted.length} accepted</span> · <span className="bad">{rejected.length} rejected</span> ·{" "}
                  {ordered.length - accepted.length - rejected.length - conflicts.length} undecided
                  {conflicts.length > 0 && <span className="bad"> · {conflicts.length} conflict</span>}
                </span>
                <button
                  className="btn primary sm"
                  disabled={!canCommit}
                  title={conflicts.length ? "Resolve conflicts first" : accepted.length ? undefined : "Accept a file first"}
                  onClick={() => setCommitting(true)}
                >
                  Commit accepted…
                </button>
              </div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

function commitsReady(ahead: number): string {
  return ahead === 1 ? "1 commit ready." : `${ahead} commits ready.`;
}

function FileRow({ file, active, onSelect }: { file: ChangedFile; active: boolean; onSelect: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (active) ref.current?.scrollIntoView?.({ block: "nearest" });
  }, [active]);
  return (
    <button
      ref={ref}
      className={`file ${active ? "active" : ""}`}
      aria-label={`File ${file.path}, ${file.status}`}
      aria-current={active ? "true" : undefined}
      onClick={onSelect}
    >
      <span className={`st ${LETTER[file.status]}`}>{LETTER[file.status]}</span>
      <span className="path" title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}>
        <span>{file.path}</span>
      </span>
      <span className="stat">
        {file.decision && (
          <span className={`dec ${file.decision}`} data-testid="decision">
            {file.decision === "accept" ? "accepted" : "rejected"}
          </span>
        )}{" "}
        <span className="a">+{file.added}</span> <span className="d">−{file.removed}</span>
      </span>
    </button>
  );
}

function FileDiff(props: { file: ChangedFile; onDecide: (d: Decision) => void; onAsk: () => void }) {
  const { file, onDecide, onAsk } = props;
  const conflicted = file.status === "conflicted";
  return (
    <div className="diff" aria-label={`Diff of ${file.path}`}>
      <div className="diff-file">
        <span className={`st ${LETTER[file.status]}`}>{LETTER[file.status]}</span>
        <span className="p">{file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}</span>
        <span className="m">
          +{file.added} −{file.removed}
        </span>
        <button className="btn ghost sm" onClick={onAsk}>
          Ask agent
        </button>
        {!conflicted && (
          <>
            <button
              className={`btn sm ${file.decision === "accept" ? "ok" : ""}`}
              aria-pressed={file.decision === "accept"}
              title={`Accept file (${keysLabel("review.accept")})`}
              onClick={() => onDecide("accept")}
            >
              Accept file
            </button>
            <button
              className={`btn sm ${file.decision === "reject" ? "danger" : ""}`}
              aria-pressed={file.decision === "reject"}
              title={`Reject file (${keysLabel("review.reject")})`}
              onClick={() => onDecide("reject")}
            >
              Reject file
            </button>
          </>
        )}
      </div>
      {conflicted ? (
        <p className="diff-empty">This file has a merge conflict. Resolve it before deciding.</p>
      ) : file.binary ? (
        <p className="diff-empty">Binary file; no text diff.</p>
      ) : file.hunks.length === 0 ? (
        <p className="diff-empty">No line changes (mode or name only).</p>
      ) : (
        file.hunks.map((h) => <HunkView key={h.id} hunk={h} decision={file.decision} />)
      )}
    </div>
  );
}

function HunkView({ hunk, decision }: { hunk: Hunk; decision: Decision | null }) {
  return (
    <div className={`hunkbox ${decision ?? ""}`} data-hunk={hunk.id}>
      <div className="hh">{hunk.header}</div>
      {hunk.lines.map((l, i) => (
        <div key={i} className={`ln ${l.kind}`}>
          <span className="g">{l.old ?? ""}</span>
          <span className="g">{l.new ?? ""}</span>
          <span className="s">{l.kind === "added" ? "+" : l.kind === "removed" ? "−" : " "}</span>
          <span className="c">{l.text}</span>
        </div>
      ))}
    </div>
  );
}

function CommitBox(props: {
  draft: string;
  count: number;
  branch: string | null;
  disabled: boolean;
  onCommit: (message: string) => void;
  onCancel: () => void;
}) {
  const { draft, count, branch, disabled, onCommit, onCancel } = props;
  const [message, setMessage] = useState(draft);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  // An open commit box is an inline confirm: Esc cancels it.
  useAction("confirm.cancel", onCancel);
  const commit = () => {
    if (!disabled && message.trim()) onCommit(message);
  };
  return (
    <>
      <textarea
        ref={ref}
        aria-label="Commit message"
        rows={4}
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={(e) => {
          if (matches(e, "review.commit")) {
            e.preventDefault();
            commit();
          }
        }}
      />
      <div className="row">
        <span className="hint">
          Commits {count} accepted file{count === 1 ? "" : "s"} on <code>{branch ?? "this worktree"}</code>
        </span>
        <button className="btn ghost sm" onClick={onCancel}>
          Cancel
        </button>
        <button
          className="btn primary sm"
          disabled={disabled || !message.trim()}
          title={`Commit (${keysLabel("review.commit")})`}
          onClick={commit}
        >
          Commit
        </button>
      </div>
    </>
  );
}
