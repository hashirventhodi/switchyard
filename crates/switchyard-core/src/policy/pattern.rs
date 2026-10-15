//! Command patterns: a command prefix plus a glob (`*`) on the rest. There is no shell
//! parsing; instead a wildcard is kept narrow:
//!
//! - it never covers an Interpreter prefix (`bash -c`, `sh -c`, `eval`, `xargs`, `env`,
//!   `npx`) or what the interpreter runs: a grant reaches one only when its literal text
//!   names the prefix and at least one word after it (`npx prettier *`, not `npx *` or
//!   `bash -c *`);
//! - it never covers chaining, substitution or redirection (`;`, `|`, `&`, backticks,
//!   `$(`, `<`, `>`, newlines), so `git push *` does not cover `git push && rm -rf /`.
//!
//! Whitespace runs count as one space. A trailing ` *` also matches nothing at all, so
//! `git push *` covers a bare `git push`.

/// Characters a wildcard never matches across.
const CHAIN: &[&str] = &[";", "|", "&", "`", "$(", "<", ">", "\n", "\r"];

/// Commands that run other commands, by name.
const RUNNERS: &[&str] = &["eval", "xargs", "env", "npx"];
/// Shells: an interpreter prefix when followed by a `-c` style flag.
const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash"];

/// Whether `pattern` covers `command`.
pub fn matches(pattern: &str, command: &str) -> bool {
    let pattern = squash(pattern);
    let command = squash(command);
    if !pattern.contains('*') {
        return !pattern.is_empty() && pattern == command;
    }
    let literal = &pattern[..pattern.find('*').expect("has a wildcard")];
    if literal.trim().is_empty() {
        return false;
    }
    let globbed = glob(&pattern, &command)
        || pattern.strip_suffix(" *").is_some_and(|bare| !bare.contains('*') && bare == command);
    if !globbed {
        return false;
    }
    // `command` starts with `literal` (the glob matched); the rest is what wildcards cover.
    let rest = command.get(literal.len()..).unwrap_or("");
    if CHAIN.iter().any(|c| rest.contains(c)) {
        return false;
    }
    let words: Vec<&str> = rest.split(' ').filter(|w| !w.is_empty()).collect();
    if (0..words.len()).any(|i| interpreter_at(&words[i..])) {
        return false;
    }
    // A wildcard right after `bash ` would cover its `-c`; one right after `npx ` would
    // cover whatever it runs.
    match interpreter_words(&command) {
        Some(n) => complete_words(literal) > n && interpreter_words(literal) == Some(n),
        None => true,
    }
}

/// Whole words in a pattern's literal part (a word the wildcard continues is not whole).
fn complete_words(literal: &str) -> usize {
    let words = literal.split(' ').filter(|w| !w.is_empty()).count();
    if literal.ends_with(' ') { words } else { words.saturating_sub(1) }
}

/// The pattern "Always allow" offers for `command`: the command and its subcommand plus
/// a wildcard (`cargo test *`), or the exact command when a wildcard would not cover it
/// (an interpreter, a chain).
pub fn suggest(command: &str) -> String {
    let command = squash(command);
    if starts_with_interpreter(&command) || CHAIN.iter().any(|c| command.contains(c)) || command.contains('*') {
        return command;
    }
    let words: Vec<&str> = command.split(' ').collect();
    let mut prefix = vec![words[0]];
    if let Some(sub) = words.get(1) {
        let plain = sub.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'));
        if plain && !sub.starts_with('-') {
            prefix.push(sub);
        }
    }
    // A dangerous command is answered only by a grant that names its classes, so the
    // suggestion grows word by word until it does (`npm run db:migrate *`, not `npm run *`).
    (prefix.len()..=words.len())
        .map(|n| format!("{} *", words[..n].join(" ")))
        .find(|p| super::dangerous::explicitly_covers(p, &command) && matches(p, &command))
        .unwrap_or(command)
}

/// Whether the command named by `pattern`'s first word is an interpreter of any kind
/// (`bash`, `npx`...), so a CLI-side prefix rule could reach an interpreter prefix.
pub fn names_interpreter(pattern: &str) -> bool {
    let squashed = squash(pattern);
    let first = squashed.split(' ').next().unwrap_or("");
    let name = basename(first);
    RUNNERS.contains(&name) || SHELLS.contains(&name)
}

/// Whitespace runs as single spaces, trimmed.
pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn starts_with_interpreter(text: &str) -> bool {
    interpreter_words(text).is_some()
}

/// How many words the interpreter prefix `text` starts with takes (`npx` 1, `bash -c` 2).
fn interpreter_words(text: &str) -> Option<usize> {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    let first = basename(words.first()?);
    if RUNNERS.contains(&first) {
        return Some(1);
    }
    let flag = words.get(1)?;
    (SHELLS.contains(&first) && flag.starts_with('-') && !flag.starts_with("--") && flag.contains('c')).then_some(2)
}

fn interpreter_at(words: &[&str]) -> bool {
    interpreter_words(&words.join(" ")).is_some()
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `*` matches any run of characters; everything else matches itself.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}
