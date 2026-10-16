//! Worktree lifecycle: create into the Switchyard worktree root, remove clean ones, and
//! read each worktree's facts from git. Worktrees made by hand are adopted in place: they
//! come from `git worktree list` like any other and are never moved or renamed. Nothing
//! here is stored; git is the source of truth.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use switchyard_schema::command_name;
use switchyard_schema::{Event, SessionStatus, Worktree, WorktreeFacts};

use crate::store::StoredRepository;
use crate::{git, repo_policy, review, sessions, shells, Inner};

/// Create a worktree on a new `branch` from the repository's base; announce it and its
/// facts. Returns it, or refuses `command` with the reason.
pub(crate) fn create(inner: &mut Inner, command: &str, repository_id: &str, branch: &str) -> Option<Worktree> {
    let branch = branch.trim();
    let Some(repo) = repository(inner, repository_id) else {
        inner.refuse(command, crate::no_repository(&repository_id));
        return None;
    };
    let dir = Path::new(&repo.path);
    if branch.is_empty() {
        inner.refuse(command, "Name the branch for the new worktree".into());
        return None;
    }
    if !git::is_valid_branch_name(dir, branch) {
        inner.refuse(command, format!("{branch} is not a valid branch name"));
        return None;
    }
    if git::branch_exists(dir, branch) {
        inner.refuse(command, format!("Branch {branch} already exists; pick another name or use its worktree"));
        return None;
    }
    // The repository's policy may name its own root (for tooling that assumes worktrees
    // sit next to or inside the checkout); otherwise one folder per repository under ours.
    let parent = match repo_policy::load(dir) {
        // A committed root is a request until acknowledged (ADR-0006).
        Ok(policy) => policy
            .worktree_root(dir)
            .filter(|_| crate::policy::acknowledged(inner, &repo.id, &policy))
            .unwrap_or_else(|| inner.runtime.worktree_root.join(repository_folder(dir))),
        Err(reason) => {
            inner.refuse(command, format!("Could not create the worktree: {reason}"));
            return None;
        }
    };
    let target = free_path(&parent, &slug(branch));
    let primary = match git::worktrees(dir) {
        Ok(list) if !list.is_empty() => list.into_iter().next().expect("not empty"),
        _ => {
            inner.refuse(command, format!("{} is no longer a readable git repository", repo.path));
            return None;
        }
    };
    let base = git::base_branch(dir, &primary);
    let created = std::fs::create_dir_all(&parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))
        .and_then(|_| {
            git::add_worktree(dir, branch, &target, &git::base_ref(dir, &base)).map_err(|git::GitError(e)| e)
        });
    if let Err(e) = created {
        inner.refuse(command, format!("Could not create the worktree: {e}"));
        return None;
    }
    // Announce the path exactly as git lists it.
    let listed = git::worktrees(dir).ok().and_then(|list| {
        list.into_iter().find(|w| w.branch.as_deref() == Some(branch))
    });
    let path = listed.map(|w| w.path).unwrap_or(target);
    let worktree = Worktree { path: path.to_string_lossy().into_owned(), branch: Some(branch.into()), primary: false };
    inner.emit(Event::WorktreeAdded { repository_id: repo.id.clone(), worktree: worktree.clone() });
    crate::disk::remember_added(inner, &repo.id, &worktree);
    emit_facts(inner, &path, &base);
    crate::disk::watch(inner, &path);
    Some(worktree)
}

/// "Create new from base": a fresh worktree, then a session on it. A session that cannot
/// start leaves the worktree free rather than deleting what was just made.
pub(crate) fn start_session_on_new(
    inner: &mut Inner,
    core: &Arc<Mutex<Inner>>,
    repository_id: &str,
    branch: &str,
    start: sessions::Start,
) {
    const COMMAND: &str = command_name::START_SESSION_ON_NEW_WORKTREE;
    let (agent, permission_mode) = (start.agent, start.permission_mode);
    if start.prompt.trim().is_empty() {
        return inner.refuse(COMMAND, "Write a prompt for the agent first".into());
    }
    // An agent that cannot start would leave a worktree nobody asked for.
    if let Err(reason) = crate::compat::admit(inner, agent) {
        return inner.refuse(COMMAND, reason);
    }
    if let Some(repo) = repository(inner, repository_id) {
        if let Err(reason) = crate::policy::admit_mode(&repo, permission_mode) {
            return inner.refuse(COMMAND, reason);
        }
    }
    if let Some(worktree) = create(inner, COMMAND, repository_id, branch) {
        sessions::start(inner, core, &worktree.path, start);
    }
}

/// Remove a worktree that is not primary, has no session at work and no uncommitted files.
/// A Paused, Finished or Error session on it is archived (its transcript outlives the
/// worktree). The branch stays. Shells on the worktree close with it, once git has removed it.
pub(crate) fn remove(inner: &mut Inner, worktree_path: &str) {
    if let Err(reason) = try_remove(inner, worktree_path) {
        inner.refuse(command_name::REMOVE_WORKTREE, reason);
    }
}

/// [`remove`], returning the reason instead of refusing a command: the Merged cleanup and
/// Prune merged go through here too.
pub(crate) fn try_remove(inner: &mut Inner, worktree_path: &str) -> Result<(), String> {
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else {
        return Err(format!("{worktree_path} is not a worktree of an added repository"));
    };
    let name = worktree.branch.clone().unwrap_or_else(|| worktree_path.to_string());
    if worktree.primary {
        return Err(format!("{name} is the primary worktree; it is never removed"));
    }
    if let Some(status) = sessions::occupant(inner, worktree_path) {
        if matches!(status, SessionStatus::Running | SessionStatus::Waiting) {
            let label = sessions::status_label(status);
            return Err(format!("{name} has a {label} session; stop it before removing the worktree"));
        }
    }
    match git::dirty_count(&worktree.path) {
        Ok(0) => {}
        Ok(1) => return Err(format!("{name} has 1 uncommitted file; commit or discard it before removing the worktree")),
        Ok(n) => return Err(format!("{name} has {n} uncommitted files; commit or discard them before removing the worktree")),
        Err(git::GitError(e)) => return Err(format!("git could not read {name}: {e}")),
    }
    if let Err(git::GitError(e)) = git::remove_worktree(Path::new(&repo.path), &worktree.path) {
        return Err(format!("Could not remove {name}: {e}"));
    }
    // Checked above: whatever session is left there is not at work.
    let _ = sessions::archive_occupant(inner, worktree_path);
    forget(inner, &repo.id, worktree_path);
    Ok(())
}

/// A worktree git has removed: its reviews, shells, pull request tracking and watch go with
/// it, and it is announced gone. Its session, if any, must be archived first.
pub(crate) fn forget(inner: &mut Inner, repository_id: &str, worktree_path: &str) {
    review::forget(inner, worktree_path);
    shells::close_worktree(inner, worktree_path);
    crate::forge::forget(inner, worktree_path);
    crate::disk::unwatch(inner, Path::new(worktree_path));
    inner.emit(Event::WorktreeRemoved { repository_id: repository_id.into(), worktree_path: worktree_path.into() });
}

/// Re-read every known worktree's facts from git and emit them.
pub(crate) fn refresh(inner: &Inner) {
    let Ok(repos) = inner.store.repositories() else { return };
    for repo in repos {
        refresh_repository(inner, Path::new(&repo.path));
    }
}

/// Re-read the facts of every worktree of the repository at `dir` and emit them.
pub(crate) fn refresh_repository(inner: &Inner, dir: &Path) {
    let Ok(list) = git::worktrees(dir) else { return };
    let Some(primary) = list.first() else { return };
    let base = git::base_branch(dir, primary);
    for wt in &list {
        emit_facts(inner, &wt.path, &base);
    }
}

/// Re-read one worktree's facts from git and emit them (after an agent turn, say).
pub(crate) fn refresh_one(inner: &Inner, worktree_path: &str) {
    let Some((repo, worktree)) = inner.find_worktree(worktree_path) else { return };
    let dir = Path::new(&repo.path);
    let Some(primary) = git::worktrees(dir).ok().and_then(|l| l.into_iter().next()) else { return };
    emit_facts(inner, &worktree.path, &git::base_branch(dir, &primary));
}

/// Emit what git says about the worktree at `path` relative to `base`. A worktree git
/// cannot read (deleted behind our back, say) gets no facts.
pub(crate) fn emit_facts(inner: &Inner, path: &Path, base: &str) {
    let Ok(dirty) = git::dirty_count(path) else { return };
    let base = git::base_ref(path, base);
    let (ahead, behind) = git::ahead_behind(path, &base).unwrap_or((0, 0));
    // Not behind means base is already in the branch: nothing to conflict with.
    let conflicts = if behind > 0 { git::conflicting_files(path, &base).unwrap_or_default() } else { Vec::new() };
    // What Changes lists: the uncommitted files, plus files that conflict with base.
    let changed_files = match git::changed_paths(path) {
        Ok(mut list) => {
            list.extend(conflicts.iter().cloned());
            list.sort();
            list.dedup();
            list.len() as u32
        }
        Err(_) => dirty,
    };
    let commits = ahead.min(crate::rebase::HISTORY_LIMIT as u32);
    inner.emit(Event::WorktreeFactsChanged {
        worktree_path: path.to_string_lossy().into_owned(),
        facts: WorktreeFacts { ahead, behind, dirty, conflicts, changed_files, commits },
    });
}

pub(crate) fn repository(inner: &Inner, id: &str) -> Option<StoredRepository> {
    inner.store.repositories().ok()?.into_iter().find(|r| r.id == id)
}

/// `<name>-<hash>`: the repository folder's name, made short ASCII, plus a hash of its
/// full path so two clones with the same folder name never share a directory.
pub(crate) fn repository_folder(primary: &Path) -> String {
    let name = primary.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = slug(&name);
    let hash = fnv1a(primary.to_string_lossy().as_bytes());
    let hash = format!("{hash:016x}");
    if name.is_empty() {
        hash[..8].to_string()
    } else {
        format!("{name}-{}", &hash[..8])
    }
}

/// A short ASCII directory name: letters, digits, `.`, `_` and `-`; anything else becomes `-`.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    let out: String = out.trim_matches(|c| c == '-' || c == '.').chars().take(48).collect();
    out.trim_end_matches('-').to_string()
}

/// `parent/name`, or `parent/name-2`, `-3`... when that is taken.
fn free_path(parent: &Path, name: &str) -> PathBuf {
    let name = if name.is_empty() { "worktree" } else { name };
    let mut candidate = parent.join(name);
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{name}-{n}"));
        n += 1;
    }
    candidate
}

/// 64-bit FNV-1a: stable across builds and platforms, unlike `std`'s hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
