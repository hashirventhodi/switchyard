# The core pushes every state change; the renderer never pulls

The renderer already receives one typed event stream from the core (ADR-0001), and live things (session status, transcript, terminals, spend) arrive on it. Everything derived from the filesystem and from git did not: Changes, the commit log, Context, policy, declared MCP servers and tasks were read once when a view mounted, and git facts were re-derived only at the end of an agent turn and on window focus. So the Changes list froze mid-turn, its badge froze while another tab was open, facts were empty at startup, a worktree removed in a shell stayed in the sidebar until restart, and an edit to `policy.yml` on disk was never seen. We decided that the core owns knowing when state changed: it watches each Worktree and the Repository's configuration on disk, and pushes the result. A view may read when it opens, but nothing in the renderer may be the only reason a change becomes visible, and nothing in the renderer polls.

## The rule

- The core watches, per Worktree, the working tree (honouring `.gitignore`) and the Repository's `.git` (refs, index, HEAD, `worktrees/`), plus `.switchyard/policy.yml`, `.mcp.json`, instruction files and the local tasks file. Changes are coalesced per Worktree over a short window (about half a second).
- On every change the core pushes **Worktree facts** for that Worktree: ahead, behind, dirty, changed-file count, commit count, conflicts. Facts are cheap, so they go out whether or not any view of the Worktree is open, and `attach` emits them for every Worktree. Badges and counters show facts and nothing else; they are never derived from a list a tab happened to load.
- Alongside facts the core emits a `WorktreeChanged` signal naming what moved (files, git, config). The expensive lists (Changes with diffs, commit log, Context files) are re-read on that signal by whichever view is showing them. Diffs are computed only for what is on screen.
- Configuration found changed on disk is re-read and pushed as the same events the in-app commands produce (`PolicyChanged`, `McpServersChanged`, `ContextRead`, `TasksChanged`). A changed policy or MCP definition re-hashes and so becomes a Policy request again (ADR-0006).
- Worktrees added or removed outside Switchyard produce `WorktreeAdded` / `WorktreeRemoved`. A Session on a removed Worktree is stopped and becomes an Archived session.
- The Forge is the one source without a push: pull requests and GitHub issues stay on the core's poller (15 s backing off to 2 min), the only timer in the system. Issues join it once `gh` is set up and a first sync has happened; the Sync button remains as "sync now". Only Forge data shows a "checked N s ago" indicator; local data carries none because it is live.
- Window focus triggers an immediate Forge poll and nothing else; the old focus-time git refresh is gone. While the window is hidden the watcher and agent events keep running and the poller sits at its slowest cadence.
- If a watcher cannot run (inotify limit, network filesystem), the core falls back to re-deriving that Worktree's facts every ~10 s and tells the user that live updates are unavailable for it. Nothing goes silently stale.

## Considered options

- Keep read-on-mount and add refreshes where staleness was noticed: each fix is local and the next view regresses; the user keeps clicking to see what git already knows.
- Poll from the renderer on a timer: simple, but every view pays for every worktree whether shown or not, latency is the interval, and the renderer becomes a second owner of "when did state change".
- Push the full Changes and commit log for every worktree on every change: the simplest renderer, but recomputing diffs for every worktree while an agent writes files is the wrong cost for data nobody is looking at. Facts-always, lists-on-demand keeps the badges honest and the diffs lazy.

## Consequences

- A Decision is pinned to the content it was made on. Because Changes now move under the reviewer, a changed file or hunk makes its Decision a **Stale decision**: still shown, flagged, no longer counting; a stale Accept must be re-confirmed before the file is committable, and a stale hunk Reject reads as feedback sent and revised.
- `WorktreeFactsChanged` grows the counts the badges need (changed files, commits ahead) so the Changes and History tab labels are right before either tab has been opened.
- New views inherit the rule: render from the store, read the heavy list on open, re-read on `WorktreeChanged`. A view that needs a timer to be correct is a bug in the core.
