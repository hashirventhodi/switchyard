//! The dangerous command classes: network, delete, migration and push. In Ask first and
//! Auto edit (and Explore) they always prompt, whatever the mode or a wildcard grant would
//! allow, unless a grant names them explicitly: its literal text (before any wildcard)
//! is itself in every class the command is in (`git push *` for `git push origin`, `rm *`
//! for `rm -rf build`; never `git *` or `pnpm *`).
//!
//! Classification is by word, anywhere in the command (no shell parsing), so a dangerous
//! word behind a runner (`pnpm exec curl …`) still counts. The Claude Code sidecar keeps
//! the same lists (`sidecar/src/dangerous.ts`) to route such commands to the host even
//! when the CLI's own mode or rules would approve them; both are checked against
//! `sidecar/test/dangerous-commands.json`.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Network,
    Delete,
    Migration,
    Push,
}

const NETWORK: &[&str] = &["curl", "wget", "ssh", "scp", "sftp", "rsync", "nc", "ncat", "telnet", "ftp"];
const DELETE: &[&str] = &["rm", "rmdir", "unlink", "shred", "-delete"];
/// Words that delete after `git`.
const GIT_DELETE: &[&str] = &["clean", "rm"];

/// The dangerous classes `command` is in (empty: not dangerous).
pub fn classes(command: &str) -> BTreeSet<Class> {
    let words: Vec<String> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '`' | '(' | ')' | '<' | '>' | '$'))
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches(|c| c == '"' || c == '\'').rsplit('/').next().unwrap_or(w).to_ascii_lowercase())
        .collect();
    let mut found = BTreeSet::new();
    for (i, word) in words.iter().enumerate() {
        let w = word.as_str();
        if NETWORK.contains(&w) {
            found.insert(Class::Network);
        }
        if DELETE.contains(&w) || (i > 0 && words[i - 1] == "git" && GIT_DELETE.contains(&w)) {
            found.insert(Class::Delete);
        }
        if w.contains("migrat") {
            found.insert(Class::Migration);
        }
        if w == "push" {
            found.insert(Class::Push);
        }
    }
    found
}

/// Whether a command grant `pattern` names every dangerous class of `command` in its
/// literal text.
pub fn explicitly_covers(pattern: &str, command: &str) -> bool {
    let literal = &pattern[..pattern.find('*').unwrap_or(pattern.len())];
    classes(command).is_subset(&classes(literal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The vectors the sidecar's copy is checked against too.
    #[test]
    fn classifies_the_shared_vectors() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../../../sidecar/test/dangerous-commands.json")).unwrap();
        for case in vectors.as_array().unwrap() {
            let command = case["command"].as_str().unwrap();
            let expected: Vec<&str> = case["classes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
            let got: Vec<&str> = classes(command)
                .into_iter()
                .map(|c| match c {
                    Class::Network => "network",
                    Class::Delete => "delete",
                    Class::Migration => "migration",
                    Class::Push => "push",
                })
                .collect();
            assert_eq!(got, expected, "{command}");
        }
    }

    #[test]
    fn a_grant_is_explicit_only_when_its_literal_names_the_class() {
        assert!(explicitly_covers("git push *", "git push origin main"));
        assert!(explicitly_covers("rm *", "rm -rf build"));
        assert!(explicitly_covers("pnpm db:migrate", "pnpm db:migrate"));
        assert!(!explicitly_covers("git *", "git push origin main"));
        assert!(!explicitly_covers("pnpm *", "pnpm db:migrate"));
        assert!(explicitly_covers("git *", "git status"));
    }
}
