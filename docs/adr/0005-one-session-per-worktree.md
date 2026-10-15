# Exactly one session per worktree, Explore sessions included

Running a read-only "explore" agent alongside a working agent on the same worktree looks harmless and will be the first rule a contributor tries to relax. We decided a worktree has at most one session of any kind: two agents on one tree produce conflicting advice, double the cost, and make incidents hard to diagnose. The primary worktree only ever hosts Explore sessions; any writing session gets its own worktree, and "quick fix on main" is one click that creates a `fix/<slug>` worktree and ends in a pull request.

## Consequences

- Starting on an occupied worktree is refused unless the occupant is Idle, Done or Failed; then "stop and replace" is offered inline, and the replaced session is archived, not dropped.
- Explore is enforced through the CLI's own read-only modes (Claude Code plan mode, Codex read-only sandbox), not by Switchyard inspecting commands.
