//! A fake GitHub CLI: a shell script named `gh` in its own bin directory, so the core's
//! real Forge implementation runs it exactly as it would run `gh` on the user's PATH. It
//! never touches the network. It logs every invocation, answers `--version` and
//! `auth status`, records `pr create` as an open pull request (with the worktree's HEAD as
//! the head commit) and answers `pr list --head <branch>` from what it holds. Tests change
//! what GitHub reports with `set_state`, `set_checks`, `set_review` and `set_prs`.
//!
//! `issue list` prints what `set_issues` holds; `issue close` only logs (tests read the
//! calls and change the issues themselves).
//!
//! Until `install` is called there is no `gh` at all (the CLI is missing).

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Clone)]
pub struct FakeGh {
    dir: PathBuf,
}

const SCRIPT: &str = r#"#!/bin/sh
state="@STATE@"
for a in "$@"; do printf '%s\037' "$a"; done >> "$state/calls"
printf '\n' >> "$state/calls"
hold="$state/hold-$1-$2"
if [ -f "$hold" ]; then
  touch "$state/held-$1-$2"
  while [ -f "$hold" ]; do sleep 0.02; done
fi
head=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--head" ]; then head="$a"; fi
  prev="$a"
done
file="$state/prs/$(printf '%s' "$head" | tr '/' '_').json"
case "$1 $2" in
  "--version "*)
    echo "gh version 2.81.0 (2026-09-01)"
    ;;
  "auth status")
    if [ -f "$state/signed-out" ]; then
      echo "You are not logged into any GitHub hosts. To log in, run: gh auth login" >&2
      exit 1
    fi
    echo "github.com: Logged in to github.com account octo (keyring)"
    ;;
  "pr create")
    if [ -f "$state/create-fails" ]; then cat "$state/create-fails" >&2; exit 1; fi
    n=$(cat "$state/next")
    echo $((n + 1)) > "$state/next"
    oid=$(git rev-parse HEAD)
    url="https://github.com/acme/acme/pull/$n"
    printf '[{"number":%s,"url":"%s","state":"OPEN","isDraft":false,"reviewDecision":"","statusCheckRollup":[],"headRefOid":"%s"}]' "$n" "$url" "$oid" > "$file"
    echo "$url"
    ;;
  "pr list")
    if [ -f "$file" ]; then cat "$file"; else echo "[]"; fi
    ;;
  "issue list")
    if [ -f "$state/signed-out" ]; then echo "To get started with GitHub CLI, please run:  gh auth login" >&2; exit 4; fi
    if [ -f "$state/issues.json" ]; then cat "$state/issues.json"; else echo "[]"; fi
    ;;
  "issue close")
    if [ -f "$state/close-fails" ]; then cat "$state/close-fails" >&2; exit 1; fi
    echo "Closed issue #$3"
    ;;
  *)
    echo "fake gh: unknown command $*" >&2
    exit 1
    ;;
esac
"#;

impl FakeGh {
    pub fn in_dir(dir: PathBuf) -> Self {
        FakeGh { dir }
    }

    /// The path the core runs as `gh`.
    pub fn program(&self) -> PathBuf {
        self.dir.join("bin").join("gh")
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    /// Put `gh` in place, signed in. Pull requests are numbered from 421.
    pub fn install(&self) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(self.dir.join("bin")).unwrap();
        std::fs::create_dir_all(self.state().join("prs")).unwrap();
        if !self.state().join("next").exists() {
            std::fs::write(self.state().join("next"), "421\n").unwrap();
        }
        let script = SCRIPT.replace("@STATE@", self.state().to_str().unwrap());
        std::fs::write(self.program(), script).unwrap();
        std::fs::set_permissions(self.program(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    pub fn sign_out(&self) {
        std::fs::write(self.state().join("signed-out"), "").unwrap();
    }

    pub fn sign_in(&self) {
        let _ = std::fs::remove_file(self.state().join("signed-out"));
    }

    /// `pr create` fails with `message` on stderr.
    pub fn fail_create(&self, message: &str) {
        std::fs::write(self.state().join("create-fails"), message).unwrap();
    }

    /// From now on `gh <verb>` waits (after logging the call) until `release(verb)`: a slow
    /// network, under the test's control.
    pub fn hold(&self, verb: [&str; 2]) {
        std::fs::write(self.state().join(format!("hold-{}-{}", verb[0], verb[1])), "").unwrap();
    }

    /// Wait until a `gh <verb>` is waiting on `hold` (panics after 20 s).
    pub fn wait_until_held(&self, verb: [&str; 2]) {
        let held = self.state().join(format!("held-{}-{}", verb[0], verb[1]));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !held.exists() {
            assert!(std::time::Instant::now() < deadline, "gh {} {} never ran", verb[0], verb[1]);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub fn release(&self, verb: [&str; 2]) {
        let _ = std::fs::remove_file(self.state().join(format!("hold-{}-{}", verb[0], verb[1])));
    }

    /// Every invocation so far, as argument lists.
    pub fn calls(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.state().join("calls")).unwrap_or_default();
        text.lines()
            .map(|line| line.split('\u{1f}').filter(|a| !a.is_empty()).map(str::to_owned).collect())
            .collect()
    }

    /// Invocations whose first two arguments are `verb` (e.g. `["pr", "list"]`).
    pub fn calls_of(&self, verb: [&str; 2]) -> usize {
        self.calls_with(verb).len()
    }

    /// The invocations whose first two arguments are `verb`, whole.
    pub fn calls_with(&self, verb: [&str; 2]) -> Vec<Vec<String>> {
        self.calls().into_iter().filter(|c| c.len() >= 2 && c[0] == verb[0] && c[1] == verb[1]).collect()
    }

    /// What `issue list` prints from now on (GitHub's JSON, newest first).
    pub fn set_issues(&self, issues: Value) {
        std::fs::write(self.state().join("issues.json"), issues.to_string()).unwrap();
    }

    /// `issue close` fails with `message` on stderr.
    pub fn fail_close(&self, message: &str) {
        std::fs::write(self.state().join("close-fails"), message).unwrap();
    }

    fn file(&self, branch: &str) -> PathBuf {
        self.state().join("prs").join(format!("{}.json", branch.replace('/', "_")))
    }

    /// What `pr list --head <branch>` prints from now on: the pull requests for the branch,
    /// newest first, as GitHub's JSON.
    pub fn set_prs(&self, branch: &str, prs: Value) {
        std::fs::write(self.file(branch), prs.to_string()).unwrap();
    }

    pub fn prs(&self, branch: &str) -> Value {
        std::fs::read_to_string(self.file(branch)).map(|t| serde_json::from_str(&t).unwrap()).unwrap_or(json!([]))
    }

    fn edit_newest(&self, branch: &str, field: &str, value: Value) {
        let mut prs = self.prs(branch);
        prs[0][field] = value;
        self.set_prs(branch, prs);
    }

    /// The newest pull request for `branch` is now `state` (OPEN, CLOSED, MERGED), as when
    /// someone merges or closes it in the browser.
    pub fn set_state(&self, branch: &str, state: &str) {
        self.edit_newest(branch, "state", json!(state));
    }

    /// Its status check rollup, as GitHub reports it.
    pub fn set_checks(&self, branch: &str, rollup: Value) {
        self.edit_newest(branch, "statusCheckRollup", rollup);
    }

    pub fn set_review(&self, branch: &str, decision: &str) {
        self.edit_newest(branch, "reviewDecision", json!(decision));
    }
}

/// An issue as `gh issue list --json` prints it.
pub fn gh_issue(number: u32, title: &str, body: &str, state: &str, labels: &[&str]) -> Value {
    let labels: Vec<Value> = labels.iter().map(|l| json!({ "name": l, "description": "", "color": "ededed" })).collect();
    json!({
        "number": number,
        "title": title,
        "body": body,
        "state": state,
        "url": format!("https://github.com/acme/acme/issues/{number}"),
        "labels": labels,
    })
}

/// A pull request as `gh pr list --json` prints it.
pub fn gh_pr(number: u32, state: &str, head_oid: &str) -> Value {
    json!({
        "number": number,
        "url": format!("https://github.com/acme/acme/pull/{number}"),
        "state": state,
        "isDraft": false,
        "reviewDecision": "",
        "statusCheckRollup": [],
        "headRefOid": head_oid,
    })
}
