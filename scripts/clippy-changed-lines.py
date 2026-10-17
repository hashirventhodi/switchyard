#!/usr/bin/env python3
# Clippy at -D warnings, but only on the lines a change adds or modifies: a warning whose
# primary span lands on such a line fails; warnings on untouched lines are counted and let
# through. Temporary until #52 clears the existing warnings and CI gates the whole workspace.
#
# Usage: scripts/clippy-changed-lines.py <base>   (compares against the merge-base of <base> and HEAD)
import json
import re
import subprocess
import sys

base = sys.argv[1]

# Lines added or modified since the merge-base, per file. With --unified=0 every hunk header
# names exactly the new-side lines it touches; a pure deletion has a count of 0.
changed, path = {}, None
diff = subprocess.run(
    ["git", "diff", "--unified=0", "--no-color", "--no-ext-diff", f"{base}...HEAD"],
    check=True, capture_output=True, text=True,
).stdout
for line in diff.splitlines():
    if line.startswith("+++ "):
        path = line[6:] if line.startswith("+++ b/") else None
    elif path and (m := re.match(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@", line)):
        start, count = int(m[1]), int(m[2] or 1)
        changed.setdefault(path, set()).update(range(start, start + count))

clippy = subprocess.run(
    ["cargo", "clippy", "--workspace", "--exclude", "switchyard-app", "--all-targets",
     "--message-format=json"],
    stdout=subprocess.PIPE, text=True,
)

# --all-targets reports a lib's warning once per target that builds it; count each once.
failing, ignored = {}, set()
for raw in clippy.stdout.splitlines():
    msg = json.loads(raw)
    if msg.get("reason") != "compiler-message":
        continue
    diag = msg["message"]
    primary = [s for s in diag["spans"] if s["is_primary"]]
    if diag["level"] not in ("warning", "error") or not primary:
        continue  # notes, and the "N warnings emitted" summaries
    lint = (diag.get("code") or {}).get("code") or diag["level"]
    key = (primary[0]["file_name"], primary[0]["line_start"], lint, diag["message"])
    if any(n in changed.get(s["file_name"], ())
           for s in primary for n in range(s["line_start"], s["line_end"] + 1)):
        failing[key] = diag["rendered"]
    else:
        ignored.add(key)

for (file, line, lint, message), rendered in sorted(failing.items()):
    print(f"{file}:{line}: {lint}: {message}")
    print(rendered)
print(f"clippy: {len(failing)} on changed lines since {base}; "
      f"{len(ignored - failing.keys())} pre-existing ignored (#52)")

# A build that clippy couldn't finish fails too, wherever the error is.
sys.exit(1 if failing or clippy.returncode else 0)
