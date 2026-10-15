# Switchyard

A desktop IDE for running several local AI coding agents in parallel, each in its own git worktree, and getting their work from first prompt to merged branch. One agent per worktree, one queue of things that need a human, git as the source of truth.

## Language

### Core objects

**Repository**:
A local git checkout that Switchyard knows about, with its base branch, remote, context files and standing instructions.
_Avoid_: Project, codebase

**Worktree**:
A git worktree of a Repository on a branch. The durable unit of work: uncommitted files, commits and shells belong to it, never to a Session.
_Avoid_: Checkout, workspace, branch (a branch is what a worktree is on)

**Primary worktree**:
The Repository's main checkout. Exactly one per Repository. Never merged or removed; only Explore sessions may run on it.
_Avoid_: Main worktree, root

**Session**:
One agent running on one Worktree for a while. Owns its transcript; owns no git state. At most one per Worktree at a time, Explore sessions included.
_Avoid_: Run, conversation, chat, thread

**Model**:
The LLM a Session's agent is using. Chosen by the user when the Session starts (or left at Default, meaning the CLI's own choice) and changeable by the user between requests; the agent never changes it. Every Model the Session uses is a named one in its transcript, Default included once resolved.
_Avoid_: Engine, LLM, agent (the agent is the CLI)

**Archived session**:
A Session that was replaced or whose Worktree was removed. Its transcript is kept as the audit trail; it can be read but not resumed.
_Avoid_: Deleted session, history

**Explore session**:
A Session whose agent may read but not write: no edits, no Merge. The only kind of Session allowed on a Primary worktree.
_Avoid_: Plan session, read-only session, companion session

**Task**:
The reason a Session exists: a ticket from GitHub, Jira or a local file. A Session links to at most one Task.
_Avoid_: Ticket, issue, story, card

**Terminal**:
A tab in the dock: either a shell PTY on a Worktree, or the read-only Agent log of a Session.
_Avoid_: Console, tab

**Agent log**:
A Session's event stream rendered as terminal-styled text: tool calls and their output. Derived, never stored separately.
_Avoid_: Agent terminal, agent PTY

**Forge**:
The hosted git service that owns pull requests and reports when a branch is Merged. GitHub is the first.
_Avoid_: Remote (that is the git remote), provider, host

### Permissions

**Permission request**:
Something the agent wants that its Permission mode does not let it do unasked. Exactly five kinds: command, file write, network, MCP tool, question. Every kind puts the Session into Waiting.
_Avoid_: Prompt (that is what the user types), approval, perm

**Classifier denial**:
In Full auto, an action the CLI's own safety classifier refused. Recorded and counted on the Session; never a Permission request.

**Grant**:
A standing decision that a command pattern, network domain or MCP tool may run without asking, scoped to a Repository. Created by answering a Permission request with "Always allow" or written in Policy. File writes are never granted.
_Avoid_: Allow-list entry, whitelist, always-allow

**Policy**:
The layered set of Grants and denials that answers a permission request: organisation policy, then the Repository's committed policy, then the user's local Grants for that Repository. A denial at a higher layer wins.
_Avoid_: Settings, config, rules

**Policy request**:
Anything in a Repository's committed files that would expand what agents may do: a committed allow rule, a sandbox declaration, a default mode above Ask first, a worktree root of its own, an MCP server in `.mcp.json`. Shown but not applied until acknowledged (ADR-0006). Committed denials and the ceiling apply at once.
_Avoid_: Committed grant, trusted config

**Acknowledgement**:
The user's local yes to a Policy request, pinned to a hash of exactly what was approved; any change asks again. Kept in Switchyard's store, never in the Repository.
_Avoid_: Trust, approval, consent

**Declared MCP server**:
An MCP server listed in the Repository's `.mcp.json`. Not passed to any agent until enabled (an Acknowledgement for that server's exact definition).
_Avoid_: Installed server, configured server

**Run once**:
Approving a single permission request without creating a Grant.
_Avoid_: Session allow, temporary grant

**Interpreter prefix**:
A command that runs other commands (`bash -c`, `sh -c`, `eval`, `xargs`, `env`, `npx`). Never matched by a wildcard Grant; needs an explicit one.

**Permission mode**:
How much a Session asks before acting: Ask first, Auto edit, or Full auto. Set per Session; the Repository's Policy sets the default and the ceiling.
_Avoid_: Approval mode, autonomy level, YOLO

### Shipping

**Open pull request**:
Pushing a Worktree's branch and opening (or enqueueing) a pull request on the Forge, then tracking it until Merged. The only way work leaves a Worktree for a Repository with a remote.
_Avoid_: Merge, ship, land, submit

**Local merge**:
Merging a Worktree's branch into base in the Primary worktree. Available only for Repositories with no remote.

**Merged**:
The Forge reports the branch is in base. Triggers worktree cleanup and moves the Task to Done, whether or not the pull request was opened from Switchyard.

### Review

**Decision**:
Accept or Reject on a changed file or one of its hunks, pinned to the content it was made on. File decisions override hunk decisions. Discarded on commit.

**Stale decision**:
A Decision whose file or hunk has changed since it was made. It stays visible, flagged as changed since you decided, and stops counting: a stale Accept is not committable until re-confirmed; a stale hunk Reject reads as feedback sent and revised, awaiting re-review. Re-confirming makes a new Decision on the current content.
_Avoid_: Outdated decision, invalidated decision, dropped decision

**Hunk feedback**:
Rejected hunks and comments sent to the agent as structured review. Switchyard never reverts a hunk itself; a file with any rejected hunk is not committable until the agent revises it or the user accepts the whole file.
_Avoid_: Partial revert, hunk staging

### Session status

Exactly five statuses, shown everywhere with the same colour and label.

**Running**:
The agent is working and needs nothing from the user.

**Waiting** (shown as "Needs you"):
The agent is blocked on the user: a permission request, a question, or a finished turn awaiting the next prompt.
_Avoid_: Blocked, pending, idle (for a finished turn)

**Idle** (shown as "Paused"):
The user stopped the Session. Resumable. The only status a user puts a Session into.
_Avoid_: Stopped, suspended, waiting (for a user stop)

**Done** (shown as "Finished"):
The agent declared its work complete. Says nothing about whether the branch can be merged.
_Avoid_: Ready to merge, complete, success

**Failed** (shown as "Error"):
The agent process exited or its last command failed.
_Avoid_: Crashed, broken

**Worktree facts**:
The cheap summary of a Worktree's git state that every badge and counter shows: ahead, behind, dirty, changed-file count, commit count, conflicts. Derived from git, never stored, and pushed to the UI whenever the Worktree changes, whether or not any view of it is open.
_Avoid_: Stats, summary, status (that is a Session's)

**Ready to merge**:
A property of a Worktree, not a Session status: clean tree, ahead of its base, not behind it. Derived from git, never stored.

### Adapters

**Adapter**:
A translator between one CLI's structured output and the Adapter protocol: raw CLI messages in, events out; commands in, raw CLI messages out. Never starts or owns a process, and is deterministic over the same input, with no I/O beyond the lines it is given.
_Avoid_: Driver, wrapper, integration

**Host**:
The part that owns an agent CLI's process and feeds its output to an Adapter: Switchyard's core, or the sidecar's driver.
_Avoid_: Runner, supervisor

**Adapter protocol**:
The versioned wire format between Switchyard and an Adapter: events out, commands in. Pre-1.0, any wire-format change is a minor bump.
_Avoid_: Adapter API, agent protocol

**Fixture**:
A recorded exchange between one CLI version and the Adapter protocol: the CLI's raw output, the events it must produce, and the replies owed to commands. Marked as captured (from a real CLI run) or synthesised (written from the CLI's published types).
_Avoid_: Recording (for a synthesised one), golden file, snapshot

**Conformant adapter**:
An Adapter that passes the conformance runner against captured Fixtures from a named CLI version, under a named Adapter protocol version. Self-assessed; no authority signs off.
_Avoid_: Certified adapter, compatible adapter
