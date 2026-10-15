// The Changes tab of the review pane (BranchPane): the worktree's uncommitted files grouped
// Conflicts / Modified / Added / Deleted, the selected file's hunks, file decisions (a / r,
// j / k to move) and Commit accepted…, which commits accepted files whole. Each hunk has
// Accept / Reject / Comment; rejected hunks and comments go to the agent as Hunk feedback
// (Send feedback to agent), and a file with a rejected hunk stays out of commits until the
// agent revises it or the user accepts the whole file. Switchyard never reverts anything.
// A conflicted file shows its conflict hunks (base's lines highlighted) with Keep ours /
// Keep base (files git holds unmerged; the core writes the chosen side) and Ask agent to
// rebase; Commit stays disabled while any conflict is left. A Stale decision (its file or
// hunk changed since it was made) stays visible, flagged "changed since you decided", does
// not count, and is re-confirmed in one click with the ordinary decideFile.
import { useEffect, useRef, useState } from "react";
import { useAction, usePaletteSource } from "./actions";
import { at } from "./chunked";
import { keysLabel, matches } from "./keymap";
import type { ChangedFile } from "./schema/ChangedFile";
import type { ChangeKind } from "./schema/ChangeKind";
import type { Command } from "./schema/Command";
import type { ConflictFile } from "./schema/ConflictFile";
import type { ConflictHunk } from "./schema/ConflictHunk";
import type { ConflictSide } from "./schema/ConflictSide";
import type { Decision } from "./schema/Decision";
import type { FileStatus } from "./schema/FileStatus";
import type { Hunk } from "./schema/Hunk";
import type { StaleHunk } from "./schema/StaleHunk";
import type { WorktreeFacts } from "./schema/WorktreeFacts";
import type { SessionView, State } from "./state";
import { useWorktreeChanged } from "./useWorktreeChanged";

const LETTER: Record<FileStatus, string> = { modified: "M", added: "A", deleted: "D", renamed: "R", conflicted: "!" };
/** A listed file. `baseConflict`: a dry-run merge of base conflicts on it (#17's fact). */
type Row = ChangedFile & { baseConflict?: boolean };
const conflicted = (f: Row) => f.status === "conflicted" || !!f.baseConflict;

/** The file's own decision when it counts: a Stale decision does not. */
const inForce = (f: ChangedFile) => (f.stale ? null : f.decision);
/** A file decision made on content that has since changed. */
const staleDecision = (f: ChangedFile) => f.stale && !!f.decision;
/** A rejected hunk and no file decision: kept out of commits (the core decides the same). */
const rejectedHunks = (f: ChangedFile) => (inForce(f) ? 0 : f.hunks.filter((h) => h.decision === "reject").length);
/** Committed by Commit accepted…: accepted whole, or every hunk accepted. */
const committable = (f: Row) => {
  const decision = inForce(f);
  return !conflicted(f) && (decision === "accept" || (!decision && f.hunks.length > 0 && f.hunks.every((h) => h.decision === "accept")));
};
type FileState = "accepted" | "rejected" | "undecided" | "conflict";
function fileState(f: Row): FileState {
  if (conflicted(f)) return "conflict";
  if (committable(f)) return "accepted";
  if (inForce(f) === "reject" || rejectedHunks(f) > 0) return "rejected";
  return "undecided";
}
/** Hunks whose rejection or comment the agent has not been sent yet. */
const pendingFeedback = (files: ChangedFile[]) =>
  files.flatMap((f) => f.hunks).filter((h) => !h.sent && (h.decision === "reject" || h.comment)).length;

const GROUPS: [string, (f: Row) => boolean][] = [
  ["Conflicts", conflicted],
  ["Modified", (f) => !conflicted(f) && (f.status === "modified" || f.status === "renamed")],
  ["Added", (f) => !conflicted(f) && f.status === "added"],
  ["Deleted", (f) => !conflicted(f) && f.status === "deleted"],
];

/** Files in the order the list shows them (group by group); files that conflict with base
 *  join the Conflicts group even when they have no uncommitted change. */
function reviewOrder(files: ChangedFile[], baseConflicts: string[]): Row[] {
  const rows: Row[] = files.map((f) => (baseConflicts.includes(f.path) ? { ...f, baseConflict: true } : f));
  for (const path of baseConflicts)
    if (!files.some((f) => f.path === path))
      rows.push({
        path,
        oldPath: null,
        status: "modified",
        added: 0,
        removed: 0,
        binary: false,
        decision: null,
        stale: false,
        hunks: [],
        staleHunks: [],
        baseConflict: true,
      });
  return GROUPS.flatMap(([, test]) => rows.filter(test));
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

export interface ChangesTabProps {
  session: SessionView;
  files: ChangedFile[] | undefined;
  facts: WorktreeFacts | undefined;
  /** The Repository's base branch, for conflict notes. */
  base: string | undefined;
  send: (command: Command) => void;
  /** Put text in the composer for the user to finish and send. */
  onAsk: (text: string) => void;
  /** The worktree's conflicts with their hunks (conflictsRead), when read. */
  conflicts?: ConflictFile[];
  /** The core's change signals (State.worktreeChanges): files or git changes re-read. */
  worktreeChanges?: State["worktreeChanges"];
}

const CHANGES_KINDS: ChangeKind[] = ["files", "git"];

export function ChangesTab(props: ChangesTabProps) {
  const { session, files, facts, base, send, onAsk, conflicts: conflictFiles = [], worktreeChanges = {} } = props;
  const worktreePath = session.worktreePath;
  // Read on open, and again whenever the core says the files or git's records moved.
  useWorktreeChanged(worktreeChanges, worktreePath, CHANGES_KINDS, () => send({ type: "readChanges", worktreePath }));

  const ordered = reviewOrder(files ?? [], facts?.conflicts ?? []);
  const [activePath, setActivePath] = useState<string | null>(null);
  const active = ordered.find((f) => f.path === activePath) ?? ordered[0];
  const [committing, setCommitting] = useState(false);

  const count = (state: FileState) => ordered.filter((f) => fileState(f) === state);
  const accepted = count("accepted");
  const rejected = count("rejected");
  const conflicts = count("conflict");
  const blocked = ordered.filter((f) => rejectedHunks(f) > 0).length;
  const canCommit = accepted.length > 0 && conflicts.length === 0;
  const pending = pendingFeedback(files ?? []);
  const acceptAll = () => {
    for (const f of ordered)
      if (!conflicted(f) && inForce(f) !== "accept") send({ type: "decideFile", worktreePath, path: f.path, decision: "accept" });
  };

  const decide = (file: Row, decision: Decision, advance = false) => {
    if (conflicted(file)) return false;
    // Pressing a Stale decision's button again re-confirms it rather than clearing it.
    const next = inForce(file) === decision ? null : decision;
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
  usePaletteSource(() =>
    canCommit && !committing
      ? [{ id: "review.commit", group: "Actions" as const, title: "Commit accepted…", run: () => setCommitting(true) }]
      : [],
  );

  return (
    <div className="changes-body" role="tabpanel" aria-label="Changes">
      {files === undefined ? (
        <p className="diff-empty">Reading changes…</p>
      ) : ordered.length === 0 ? (
        <p className="diff-empty">
          {session.status === "done"
            ? `Working tree clean. ${commitsReady(facts?.ahead ?? 0)}`
            : "No uncommitted changes in this worktree."}
        </p>
      ) : (
        <>
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
              base={base}
              onDecide={(d) => decide(active, d)}
              onReconfirm={() => active.decision && send({ type: "decideFile", worktreePath, path: active.path, decision: active.decision })}
              onDecideHunk={(hunk, d) =>
                send({ type: "decideHunk", worktreePath, path: active.path, hunkId: hunk.id, decision: hunk.decision === d ? null : d })
              }
              onComment={(hunk, comment) => send({ type: "commentHunk", worktreePath, path: active.path, hunkId: hunk.id, comment })}
              onAsk={() => onAsk(`Explain the change in ${active.path} and whether it needs a test.`)}
              conflict={conflictFiles.find((c) => c.path === active.path)}
              branch={session.branch}
              canAskRebase={session.status === "waiting" || session.status === "done" || session.status === "failed"}
              onResolve={(hunk, side) => send({ type: "resolveConflict", worktreePath, path: active.path, hunkId: hunk.id, side })}
              onAskRebase={() => send({ type: "askAgentToRebase", sessionId: session.id })}
            />
          )}
          <div className="commit-box">
            {committing ? (
              <CommitBox
                draft={draftCommitMessage(sessionTitle(session), accepted)}
                count={accepted.length}
                blocked={blocked}
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
                  className="btn ghost sm"
                  disabled={pending === 0}
                  title={pending ? "Send rejected hunks and comments to the agent as one message" : "Reject or comment on a hunk first"}
                  onClick={() => send({ type: "sendFeedback", worktreePath })}
                >
                  Send feedback to agent{pending > 0 && <span className="n"> {pending}</span>}
                </button>
                <button className="btn ghost sm" onClick={acceptAll}>
                  Accept all
                </button>
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
        </>
      )}
    </div>
  );
}

function commitsReady(ahead: number): string {
  return ahead === 1 ? "1 commit ready." : `${ahead} commits ready.`;
}

function FileRow({ file, active, onSelect }: { file: Row; active: boolean; onSelect: () => void }) {
  const status = conflicted(file) ? "conflicted" : file.status;
  const state = fileState(file);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (active) ref.current?.scrollIntoView?.({ block: "nearest" });
  }, [active]);
  return (
    <button
      ref={ref}
      className={`file ${active ? "active" : ""}`}
      aria-label={`File ${file.path}, ${status}`}
      aria-current={active ? "true" : undefined}
      onClick={onSelect}
    >
      <span className={`st ${LETTER[status]}`}>{LETTER[status]}</span>
      <span className="path" title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}>
        <span>{file.path}</span>
      </span>
      <span className="stat">
        {staleDecision(file) && (
          <span className="dec stale" data-testid="stale" title={`You ${decided(file.decision)} it; re-confirm to count it`}>
            changed since you decided
          </span>
        )}
        {(state === "accepted" || state === "rejected") && (
          <span
            className={`dec ${state === "accepted" ? "accept" : "reject"}`}
            data-testid="decision"
            title={rejectedHunks(file) ? "A rejected hunk keeps this file out of commits" : undefined}
          >
            {state}
          </span>
        )}{" "}
        <span className="a">+{file.added}</span> <span className="d">−{file.removed}</span>
      </span>
    </button>
  );
}

const decided = (d: Decision | null) => (d === "accept" ? "accepted" : "rejected");

function FileDiff(props: {
  file: Row;
  base: string | undefined;
  onDecide: (d: Decision) => void;
  /** Re-confirm the file's Stale decision on its current content. */
  onReconfirm: () => void;
  onDecideHunk: (hunk: Hunk, d: Decision) => void;
  onComment: (hunk: Hunk, comment: string | null) => void;
  onAsk: () => void;
  conflict?: ConflictFile;
  branch: string | null;
  canAskRebase: boolean;
  onResolve: (hunk: ConflictHunk, side: ConflictSide) => void;
  onAskRebase: () => void;
}) {
  const { file, base, onDecide, onDecideHunk, onComment, onAsk } = props;
  const rejectedCount = rejectedHunks(file);
  const conflict = conflicted(file);
  const letter = LETTER[conflict ? "conflicted" : file.status];
  return (
    <div className="diff" aria-label={`Diff of ${file.path}`}>
      <div className="diff-file">
        <span className={`st ${letter}`}>{letter}</span>
        <span className="p">{file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}</span>
        <span className="m">
          +{file.added} −{file.removed}
        </span>
        <button className="btn ghost sm" onClick={onAsk}>
          Ask agent
        </button>
        {!conflict && (
          <>
            <button
              className={`btn sm ${file.decision === "accept" ? "ok" : ""}`}
              aria-pressed={inForce(file) === "accept"}
              title={`Accept file (${keysLabel("review.accept")})`}
              onClick={() => onDecide("accept")}
            >
              Accept file
            </button>
            <button
              className={`btn sm ${file.decision === "reject" ? "danger" : ""}`}
              aria-pressed={inForce(file) === "reject"}
              title={`Reject file (${keysLabel("review.reject")})`}
              onClick={() => onDecide("reject")}
            >
              Reject file
            </button>
          </>
        )}
      </div>
      {!conflict && staleDecision(file) && (
        <p className="stale-note" role="note">
          You {decided(file.decision)} this file, but it changed since you decided; the decision does not count until re-confirmed.{" "}
          <button className="btn sm" onClick={props.onReconfirm}>
            Re-confirm {file.decision}
          </button>
        </p>
      )}
      {rejectedCount > 0 && (
        <p className="blocked-note" role="note">
          Not committable: {rejectedCount} rejected hunk{rejectedCount === 1 ? "" : "s"}. Send feedback to the agent, or Accept file.
        </p>
      )}
      {conflict && props.conflict?.hunks.length ? (
        props.conflict.hunks.map((h, i, all) => (
          <ConflictHunkView
            key={h.id}
            hunk={h}
            label={`Conflict with ${base ?? "base"} in ${file.path} (${i + 1} of ${all.length})`}
            base={base ?? "base"}
            branch={props.branch}
            resolvable={props.conflict!.kind === "unmerged"}
            canAskRebase={props.canAskRebase}
            onResolve={(side) => props.onResolve(h, side)}
            onAskRebase={props.onAskRebase}
          />
        ))
      ) : file.status === "conflicted" ? (
        <p className="diff-empty">This file has a merge conflict. Resolve it before deciding.</p>
      ) : file.baseConflict && !file.hunks.length ? (
        <p className="diff-empty">
          {base ?? "Base"} changed this file too: merging it would conflict. Rebase, or ask the agent to rebase.
        </p>
      ) : file.binary ? (
        <p className="diff-empty">Binary file; no text diff.</p>
      ) : file.hunks.length === 0 ? (
        <p className="diff-empty">No line changes (mode or name only).</p>
      ) : (
        file.hunks.map((h) => (
          <HunkView
            key={h.id}
            hunk={h}
            fileDecision={inForce(file)}
            decidable={!conflict}
            onDecide={(d) => onDecideHunk(h, d)}
            onComment={(c) => onComment(h, c)}
          />
        ))
      )}
      {!conflict && file.staleHunks.map((h) => <StaleHunkView key={h.id} hunk={h} />)}
    </div>
  );
}

/** A hunk Decision whose hunk has since changed: shown, not counted, not decidable (the
 *  revised lines are a new hunk above). A sent Reject reads as revised: re-review it. */
function StaleHunkView({ hunk }: { hunk: StaleHunk }) {
  const revised = hunk.decision === "reject" && hunk.sent;
  return (
    <div className="hunkbox stale" role="group" aria-label={`Earlier hunk ${hunk.header}`}>
      <div className="hh">
        <span className="h">{hunk.header}</span>
        <span className="dec">{decided(hunk.decision)}</span>
        <span className="stale-flag">{revised ? "feedback sent, revised" : "changed since you decided"}</span>
      </div>
      {hunk.comment && (
        <div className="hcomment">
          <span className="who">Comment</span> {hunk.comment}
        </div>
      )}
    </div>
  );
}

/** One conflict: the branch's lines, then base's (highlighted), with context either side. */
function ConflictHunkView(props: {
  hunk: ConflictHunk;
  label: string;
  base: string;
  branch: string | null;
  /** The file is unmerged in the working tree: Keep ours / Keep base can write it. */
  resolvable: boolean;
  canAskRebase: boolean;
  onResolve: (side: ConflictSide) => void;
  onAskRebase: () => void;
}) {
  const { hunk, label, base, branch, resolvable, canAskRebase, onResolve, onAskRebase } = props;
  const ours = branch?.split("/").pop() || "ours";
  const line = (kind: string, text: string, i: number, sign = " ") => (
    <div key={`${kind}-${i}`} className={`ln ${kind}`}>
      <span className="g" />
      <span className="g" />
      <span className="s">{sign}</span>
      <span className="c">{text}</span>
    </div>
  );
  return (
    <div className="hunkbox conflict" role="group" aria-label={label}>
      <div className="conf-note">
        <b>Conflict with {base}</b>
        <span>
          {resolvable
            ? `${base} changed these lines too. Keep one side, or ask the agent.`
            : `${base} changed these lines while the agent worked; merging it would conflict.`}
        </span>
        <span className="r">
          {resolvable && (
            <>
              <button className="btn sm" title={`Keep ${branch ?? "this branch"}'s lines`} onClick={() => onResolve("ours")}>
                Keep {ours}
              </button>
              <button className="btn sm" title={`Keep ${base}'s lines`} onClick={() => onResolve("base")}>
                Keep {base}
              </button>
            </>
          )}
          <button
            className="btn sm primary"
            disabled={!canAskRebase}
            title={canAskRebase ? "Send the agent one message asking it to rebase and resolve conflicts" : "The agent is busy or paused"}
            onClick={onAskRebase}
          >
            Ask agent to rebase
          </button>
        </span>
      </div>
      {hunk.before.map((t, i) => line("context", t, i))}
      {hunk.ours.map((t, i) => line("ours added", t, i, "+"))}
      {hunk.base.map((t, i) => line("base", t, i, "−"))}
      {hunk.after.map((t, i) => line("context after", t, i))}
    </div>
  );
}

function HunkView(props: {
  hunk: Hunk;
  fileDecision: Decision | null;
  decidable: boolean;
  onDecide: (d: Decision) => void;
  onComment: (comment: string | null) => void;
}) {
  const { hunk, fileDecision, decidable, onDecide, onComment } = props;
  // A file decision overrides its hunks'.
  const decision = fileDecision ?? hunk.decision;
  const [editing, setEditing] = useState(false);
  return (
    <div className={`hunkbox ${decision ?? ""}`} data-hunk={hunk.id} role="group" aria-label={`Hunk ${hunk.header}`}>
      <div className="hh">
        <span className="h">{hunk.header}</span>
        <span className="dec">{decision === "accept" ? "accepted" : decision === "reject" ? "rejected" : ""}</span>
        {hunk.sent && (
          <span className="sent" title="The agent has this feedback">
            sent
          </span>
        )}
        {decidable && (
          <span className="r">
            <button className="a" aria-label="Accept hunk" aria-pressed={hunk.decision === "accept"} onClick={() => onDecide("accept")}>
              Accept
            </button>
            <button className="rj" aria-label="Reject hunk" aria-pressed={hunk.decision === "reject"} onClick={() => onDecide("reject")}>
              Reject
            </button>
            <button aria-label="Comment on hunk" aria-pressed={editing} onClick={() => setEditing(true)}>
              Comment
            </button>
          </span>
        )}
      </div>
      {editing ? (
        <CommentBox
          initial={hunk.comment ?? ""}
          onSave={(text) => {
            onComment(text.trim() ? text.trim() : null);
            setEditing(false);
          }}
          onCancel={() => setEditing(false)}
        />
      ) : (
        hunk.comment && (
          <div className="hcomment">
            <span className="who">Comment</span> {hunk.comment}
          </div>
        )
      )}
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

function CommentBox(props: { initial: string; onSave: (text: string) => void; onCancel: () => void }) {
  const { initial, onSave, onCancel } = props;
  const [text, setText] = useState(initial);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);
  useAction("confirm.cancel", onCancel);
  return (
    <div className="hcomment-edit">
      <textarea
        ref={ref}
        aria-label="Comment for the agent"
        rows={2}
        value={text}
        placeholder="What should the agent change here?"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (matches(e, "review.commit")) {
            e.preventDefault();
            onSave(text);
          }
        }}
      />
      <div className="row">
        <button className="btn ghost sm" onClick={onCancel}>
          Cancel
        </button>
        <button className="btn sm" aria-label="Save comment" onClick={() => onSave(text)}>
          {initial ? "Save" : "Add comment"}
        </button>
      </div>
    </div>
  );
}

function CommitBox(props: {
  draft: string;
  count: number;
  /** Files kept out by a rejected hunk. */
  blocked: number;
  branch: string | null;
  disabled: boolean;
  onCommit: (message: string) => void;
  onCancel: () => void;
}) {
  const { draft, count, blocked, branch, disabled, onCommit, onCancel } = props;
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
          {blocked > 0 && `; ${blocked} file${blocked === 1 ? "" : "s"} with rejected hunks stay${blocked === 1 ? "s" : ""} out`}
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
