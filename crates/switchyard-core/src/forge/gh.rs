//! GitHub through its CLI, `gh`: the only Forge implementation through M4 (ADR-0003).

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::Duration;

use serde::Deserialize;
use switchyard_schema::{Check, CheckStatus, ChecksState, ForgeSetup, ForgeSetupState, Link, PullRequest, PullRequestState, ReviewState};

use super::{Forge, ForgeIssue, ForgePullRequest, PullRequestText};
use crate::subprocess;

/// Runs `program` as `gh`. The app finds it with [`GhForge::find`]; tests point it at a fake.
pub struct GhForge {
    pub program: PathBuf,
    /// How long one `gh` call may take before it is given up on (it may be waiting on a
    /// network that will never answer).
    pub timeout: Duration,
}

/// How long the app waits for one `gh` call.
pub const GH_TIMEOUT: Duration = Duration::from_secs(60);

impl GhForge {
    pub fn new(program: PathBuf) -> GhForge {
        GhForge { program, timeout: GH_TIMEOUT }
    }

    pub fn with_timeout(self, timeout: Duration) -> GhForge {
        GhForge { timeout, ..self }
    }

    /// `gh` on PATH or in the usual install places (apps launched from Finder get a short
    /// PATH); plain `gh` when it is nowhere, which detection then reports as missing.
    pub fn find() -> GhForge {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let program = std::env::split_paths(&path)
            .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from))
            .map(|dir| dir.join("gh"))
            .find(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("gh"));
        GhForge::new(program)
    }

    fn run(&self, dir: Option<&Path>, args: &[&str]) -> Result<std::process::Output, String> {
        let mut cmd = Process::new(&self.program);
        cmd.args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1");
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        match subprocess::output_within(&mut cmd, self.timeout) {
            Ok(Some(out)) => Ok(out),
            Ok(None) => Err(format!("gh did not answer within {}", subprocess::describe(self.timeout))),
            Err(e) => Err(format!("could not run gh: {e}")),
        }
    }

    fn run_ok(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        let out = self.run(Some(dir), args)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

const NAME: &str = "GitHub";

impl Forge for GhForge {
    fn detect(&self) -> ForgeSetup {
        let state = match self.run(None, &["--version"]) {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                let version = text
                    .split_whitespace()
                    .skip_while(|w| *w != "version")
                    .nth(1)
                    .unwrap_or("unknown")
                    .to_string();
                match self.run(None, &["auth", "status"]) {
                    Ok(out) if out.status.success() => ForgeSetupState::Ready { version },
                    _ => ForgeSetupState::SignedOut { login_hint: "gh auth login".into() },
                }
            }
            _ => missing(),
        };
        ForgeSetup { name: NAME.into(), state }
    }

    fn create_pull_request(&self, dir: &Path, base: &str, branch: &str, text: Option<&PullRequestText>) -> Result<String, String> {
        let mut args = vec!["pr", "create", "--base", base, "--head", branch];
        match text {
            Some(t) => args.extend(["--title", t.title.as_str(), "--body", t.body.as_str()]),
            None => args.push("--fill"),
        }
        let out = self.run_ok(dir, &args)?;
        out.lines()
            .map(str::trim)
            .rfind(|l| l.starts_with("https://"))
            .map(str::to_owned)
            .ok_or_else(|| format!("gh did not print the pull request's address: {}", out.trim()))
    }

    fn pull_request(&self, dir: &Path, branch: &str) -> Result<Option<ForgePullRequest>, String> {
        let out = self.run_ok(
            dir,
            &[
                "pr", "list", "--head", branch, "--state", "all", "--limit", "10", "--json",
                "number,url,state,isDraft,reviewDecision,statusCheckRollup,headRefOid",
            ],
        )?;
        let list: Vec<GhPullRequest> =
            serde_json::from_str(&out).map_err(|e| format!("gh printed something unexpected: {e}"))?;
        // Newest first; an open one wins over older closed or merged ones.
        let pick = list.iter().position(|p| p.state == "OPEN").unwrap_or(0);
        Ok(list.into_iter().nth(pick).map(GhPullRequest::into_forge))
    }

    fn issues(&self, dir: &Path) -> Result<Vec<ForgeIssue>, String> {
        let out = self.run_ok(
            dir,
            &["issue", "list", "--state", "all", "--limit", "200", "--json", "number,title,body,state,url,labels"],
        )?;
        let list: Vec<GhIssue> =
            serde_json::from_str(&out).map_err(|e| format!("gh printed something unexpected: {e}"))?;
        Ok(list
            .into_iter()
            .map(|i| ForgeIssue {
                number: i.number,
                title: i.title,
                body: i.body,
                url: i.url,
                open: i.state != "CLOSED",
                labels: i.labels.into_iter().map(|l| l.name).collect(),
            })
            .collect())
    }
}

/// One issue as `gh issue list --json` prints it.
#[derive(Deserialize)]
struct GhIssue {
    number: u32,
    title: String,
    #[serde(default)]
    body: String,
    state: String,
    url: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

fn missing() -> ForgeSetupState {
    ForgeSetupState::Missing {
        install_hint: "brew install gh".into(),
        link: Link { label: "Install GitHub CLI".into(), url: "https://cli.github.com".into() },
    }
}

/// One pull request as `gh pr list --json` prints it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u32,
    url: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Option<Vec<GhCheck>>,
    #[serde(default)]
    head_ref_oid: String,
}

/// A check run (`status` + `conclusion`) or a commit status context (`state`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCheck {
    /// A check run's name.
    #[serde(default)]
    name: Option<String>,
    /// A status context's name.
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

impl GhPullRequest {
    fn into_forge(self) -> ForgePullRequest {
        let state = match self.state.as_str() {
            "MERGED" => PullRequestState::Merged,
            "CLOSED" => PullRequestState::Closed,
            _ => PullRequestState::Open,
        };
        let review = match self.review_decision.as_deref() {
            Some("APPROVED") => ReviewState::Approved,
            Some("CHANGES_REQUESTED") => ReviewState::ChangesRequested,
            Some("REVIEW_REQUIRED") => ReviewState::Required,
            _ => ReviewState::None,
        };
        let rollup = self.status_check_rollup.unwrap_or_default();
        ForgePullRequest {
            pull_request: PullRequest {
                number: self.number,
                url: self.url,
                state,
                draft: self.is_draft,
                checks: checks(&rollup),
                check_runs: rollup.iter().map(check).collect(),
                review,
            },
            head_oid: self.head_ref_oid,
        }
    }
}

const FAILED: &[&str] = &["FAILURE", "ERROR", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED", "STARTUP_FAILURE"];

/// One check as History lists it.
fn check(c: &GhCheck) -> Check {
    let conclusion = c.conclusion.as_deref().filter(|v| !v.is_empty());
    let status = match (c.status.as_deref(), conclusion, c.state.as_deref()) {
        (Some("COMPLETED"), Some("SUCCESS"), _) => CheckStatus::Passing,
        (Some("COMPLETED"), Some(v), _) if FAILED.contains(&v) => CheckStatus::Failing,
        (Some("COMPLETED"), _, _) => CheckStatus::Skipped,
        (Some("IN_PROGRESS"), _, _) => CheckStatus::Running,
        (Some(_), _, _) => CheckStatus::Pending,
        (None, _, Some("SUCCESS")) => CheckStatus::Passing,
        (None, _, Some(v)) if FAILED.contains(&v) => CheckStatus::Failing,
        (None, _, _) => CheckStatus::Pending,
    };
    let name = c.name.clone().or_else(|| c.context.clone()).unwrap_or_else(|| "check".into());
    let url = c.details_url.clone().or_else(|| c.target_url.clone()).filter(|u| !u.is_empty());
    Check { name, status, url }
}

/// Failing if any check failed; else Running if any has not finished; else Passing.
fn checks(rollup: &[GhCheck]) -> ChecksState {
    if rollup.is_empty() {
        return ChecksState::None;
    }
    let failed = |c: &GhCheck| {
        [c.conclusion.as_deref(), c.state.as_deref()].into_iter().flatten().any(|v| FAILED.contains(&v))
    };
    let running = |c: &GhCheck| match (&c.status, &c.state) {
        (Some(status), _) => status != "COMPLETED",
        (None, Some(state)) => state == "PENDING" || state == "EXPECTED",
        (None, None) => false,
    };
    if rollup.iter().any(failed) {
        ChecksState::Failing
    } else if rollup.iter().any(running) {
        ChecksState::Running
    } else {
        ChecksState::Passing
    }
}
