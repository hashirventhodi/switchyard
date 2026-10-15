# Committed configuration is a request: it can take privilege away at once, never add it unacknowledged

A repository's committed files (`.switchyard/policy.yml`, `.mcp.json`) are written by anyone who can push to it, and arrive with every clone and pull. Until now Switchyard applied them as they stood: a committed `allow` became a Grant, `sandbox: true` turned Full auto into bypass, and every `.mcp.json` server was started with the agent. Opening a repository was enough to give its authors standing permissions on the user's machine. We decided that committed configuration is a request, not a grant: whatever in it lowers privilege applies immediately, and whatever expands privilege takes effect only after the user acknowledges it, locally, for that exact content.

## The rule

- **Applies at once** (lowers privilege): committed denials, a ceiling, house rules. Deny still wins across layers (ADR-0004).
- **A request** (expands privilege): every committed `allow` rule (wildcard and interpreter ones included), `sandbox: true`, a `default_mode` above Ask first, a `worktrees.root` outside Switchyard's own worktree root, and each server in `.mcp.json`.
- An acknowledgement is user-local (the Switchyard store, never the repository) and pinned to a hash of what it approved: for the policy file, the hash of its privilege-expanding parts; for an MCP server, its name plus a hash of its definition (command, args, env, URL). Any change to what was approved asks again; a change to denials does not.
- Until acknowledged, a request is shown, not applied: repository allow rules are listed as requested but answer nothing, Full auto runs without bypass, New session preselects Ask first, worktrees go under Switchyard's root, and an MCP server is "declared, not enabled" and is not passed to the agent (strict MCP config stays on, so the CLI loads nothing else either).
- Where Switchyard can verify a sandbox claim (it is itself running in a container or VM) the evidence is shown beside the request; where it cannot, the request says so plainly.

## Considered options

- Trust committed files (the previous behaviour): convenient, but a pull request that edits `.mcp.json` or `policy.yml` would silently widen what agents may do on every reviewer's machine.
- Ignore committed allows and servers entirely: safe, but throws away the repository's shared knowledge of what its tools need; teams would re-grant by hand on every machine.
- Trust on first use without a hash: a later change would ride on the earlier yes.

## Consequences

- The Context tab carries the prompts: "Repository policy requests…" with Acknowledge, and each declared MCP server with Enable/Disable.
- Sessions start regardless; what is not acknowledged simply does not apply, and the session's transcript says so.
- Org-layer policy (not yet synced) is outside this rule; it will arrive through a channel the user chose, not a clone.
