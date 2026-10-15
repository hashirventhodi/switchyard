//! Worktree lifecycle: create into the Switchyard worktree root, remove clean ones, and
//! read each worktree's facts from git. Worktrees made by hand are adopted in place: they
//! come from `git worktree list` like any other and are never moved or renamed. Nothing
//! here is stored; git is the source of truth.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use switchyard_schema::{AgentKind, Event, PermissionMode, Worktree};

use crate::store::StoredRepository;
use crate::{git, sessions, Inner};

/// Create a worktree on a new `branch` from the repository's base; announce it and its
/// facts. Returns it, or refuses `command` with the reason.
pub(crate) fn create(inner: &mut Inner, command: &str, repository_id: &str, branch: &str) -> Option<Worktree> {
    let branch = branch.trim();
    let Some(repo) = repository(inner, repository_id) else {
        inner.refuse(command, format!("There is no repository {repository_id}"));
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
    let parent = inner.runtime.worktree_root.join(repository_folder(dir));
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
    emit_facts(inner, &path, &base);
    Some(worktree)
}

pub(crate) fn start_session_on_new(
    _inner: &mut Inner,
    _core: &Arc<Mutex<Inner>>,
    _repository_id: &str,
    _branch: &str,
    _agent: AgentKind,
    _permission_mode: PermissionMode,
    _prompt: &str,
) {
    let _ = sessions::start;
}

pub(crate) fn remove(_inner: &mut Inner, _worktree_path: &str) {}

pub(crate) fn refresh(_inner: &Inner) {}

fn emit_facts(inner: &Inner, path: &Path, base: &str) {
    let _ = (inner, path, base);
}

fn repository(inner: &Inner, id: &str) -> Option<StoredRepository> {
    inner.store.repositories().ok()?.into_iter().find(|r| r.id == id)
}

/// `<name>-<hash>`: the repository folder's name, made short ASCII, plus a hash of its
/// full path so two clones with the same folder name never share a directory.
fn repository_folder(primary: &Path) -> String {
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
