//! L1 — Change detection. The harness decides the scope, never the formatter.
//!
//! Supported VCS: git and jj. For git, the caller's own diff hunks (base vs
//! working tree, `--unified=0`) become the line ranges; for jj the same is
//! parsed from `jj diff --git` output.

use std::path::Path;
use std::process::Command;

use crate::types::{LineRange, Scope, ScopedFile, Vcs};

/// Default exclusion patterns (aligned with rustfmt `ignore` semantics).
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "generated/**",
    "**/generated/**",
    "vendor/**",
    "target/**",
    "node_modules/**",
];

#[derive(Debug)]
pub enum ScopeError {
    NotARepository,
    CommandFailed { cmd: String, status: String },
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeError::NotARepository => {
                write!(
                    f,
                    "not a git or jj repository (no .git/.jj found in cwd or parents)"
                )
            }
            ScopeError::CommandFailed { cmd, status } => {
                write!(f, "command `{cmd}` failed: {status}")
            }
        }
    }
}

impl std::error::Error for ScopeError {}

/// Detect the VCS of `cwd` by walking up the directory tree.
pub fn detect_vcs(cwd: &Path) -> Option<Vcs> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(Vcs::Git);
        }
        if d.join(".jj").exists() {
            return Some(Vcs::Jj);
        }
        dir = d.parent();
    }
    None
}

/// Parse a unified-diff hunk header line like `@@ -4,0 +5 @@` or
/// `@@ -10,3 +12,4 @@` into new-side (working-tree) 1-based ranges.
/// Returns (new_start, new_count); new_count == 0 means pure deletion.
fn parse_hunk_new_range(header: &str) -> Option<(usize, usize)> {
    // strip the section between the first "@@" and the second "@@"
    let s = header.strip_prefix("@@")?;
    let end = s.find("@@")?;
    let body = s[..end].trim();
    let mut parts = body.split_whitespace();
    let _old = parts.next()?;
    let new = parts.next()?.strip_prefix('+')?;
    // new is like "+5" or "+5,3"
    let (start, count) = match new.split_once(',') {
        Some((s, c)) => (s.parse::<usize>().ok()?, c.parse::<usize>().ok()?),
        None => (new.parse::<usize>().ok()?, 1),
    };
    Some((start, count))
}

/// Parse unified diff text into new-side ranges (working-tree coordinates)
/// plus the total number of added lines on the new side.
///
/// Ranges come from hunk headers (new_start, new_count); for `git diff
/// --unified=0` these are tight change ranges. Added-line counts come from
/// scanning `+` lines inside hunks (robust regardless of context settings).
fn parse_new_ranges(diff_text: &str) -> (Vec<LineRange>, usize) {
    let mut ranges = Vec::new();
    let mut added = 0usize;
    let mut in_hunk = false;
    for line in diff_text.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            if let Some((start, count)) = parse_hunk_new_range(line) {
                if count > 0 {
                    ranges.push(LineRange::new(start, start + count - 1));
                }
            } else {
                in_hunk = false;
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        // Within a hunk: count added lines (skip the file-header `+++` line,
        // which only appears before the first @@ header).
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        }
    }
    (ranges, added)
}

/// Run a command, returning stdout on success.
fn run(cmd: &mut Command) -> Result<String, ScopeError> {
    let out = cmd.output().map_err(|e| ScopeError::CommandFailed {
        cmd: format!("{cmd:?}"),
        status: e.to_string(),
    })?;
    if !out.status.success() {
        return Err(ScopeError::CommandFailed {
            cmd: format!("{cmd:?}"),
            status: format!(
                "exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_scope(cwd: &Path, base: &str, excludes: &[String]) -> Result<Scope, ScopeError> {
    // 1. changed .rs files vs base
    let name_only = run(Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=ACMR")
        .arg(base)
        .arg("--")
        .arg("*.rs")
        .current_dir(cwd))?;
    let files: Vec<String> = name_only
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let mut scoped = Vec::new();
    for path in files {
        if is_excluded(&path, excludes) {
            continue;
        }
        let diff = run(Command::new("git")
            .arg("diff")
            .arg("--unified=0")
            .arg(base)
            .arg("--")
            .arg(&path)
            .current_dir(cwd))?;
        let (ranges, added) = parse_new_ranges(&diff);
        scoped.push(ScopedFile {
            path,
            ranges,
            agent_added_lines: Some(added),
        });
    }

    Ok(Scope {
        vcs: Some(Vcs::Git),
        base: base.to_string(),
        source: "git-diff".to_string(),
        files: scoped,
    })
}

fn jj_scope(cwd: &Path, excludes: &[String]) -> Result<Scope, ScopeError> {
    // 1. changed files (working copy vs its parent)
    let name_only = run(Command::new("jj")
        .arg("diff")
        .arg("--name-only")
        .current_dir(cwd))?;
    let files: Vec<String> = name_only
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l.ends_with(".rs"))
        .collect();

    let mut scoped = Vec::new();
    for path in files {
        if is_excluded(&path, excludes) {
            continue;
        }
        let diff = run(Command::new("jj")
            .arg("diff")
            .arg("--git")
            .arg("--")
            .arg(&path)
            .current_dir(cwd))?;
        let (ranges, added) = parse_new_ranges(&diff);
        scoped.push(ScopedFile {
            path,
            ranges,
            agent_added_lines: Some(added),
        });
    }

    Ok(Scope {
        vcs: Some(Vcs::Jj),
        base: "@".to_string(),
        source: "jj-diff".to_string(),
        files: scoped,
    })
}

/// Detect scope from the VCS (git or jj). `force` picks a specific VCS.
pub fn detect_scope(
    cwd: &Path,
    vcs: Option<Vcs>,
    base: &str,
    excludes: &[String],
) -> Result<Scope, ScopeError> {
    let vcs = match vcs {
        Some(v) => v,
        None => detect_vcs(cwd).ok_or(ScopeError::NotARepository)?,
    };
    match vcs {
        Vcs::Git => git_scope(cwd, base, excludes),
        Vcs::Jj => jj_scope(cwd, excludes),
    }
}

/// glob-ish matching: '*' matches within a path segment, '**' crosses segments.
pub fn is_excluded(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| glob_match(p, path))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // Simple matcher supporting '*' (any chars except '/') and '**' (any chars).
    fn match_here(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                if p.len() >= 2 && p[1] == '*' {
                    // '**' crosses '/'
                    let rest = &p[2..];
                    // try to match rest at every position
                    for i in 0..=t.len() {
                        if match_here(rest, &t[i..]) {
                            return true;
                        }
                    }
                    false
                } else {
                    // single '*' does not cross '/'
                    let rest = &p[1..];
                    let max = t.iter().position(|&c| c == '/').unwrap_or(t.len());
                    for i in 0..=max {
                        if match_here(rest, &t[i..]) {
                            return true;
                        }
                    }
                    false
                }
            }
            c => !t.is_empty() && t[0] == c && match_here(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    match_here(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hunk_headers() {
        assert_eq!(parse_hunk_new_range("@@ -4,0 +5 @@"), Some((5, 1)));
        assert_eq!(parse_hunk_new_range("@@ -10,3 +12,4 @@"), Some((12, 4)));
        assert_eq!(parse_hunk_new_range("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(parse_hunk_new_range("@@ -5,1 +4,0 @@"), Some((4, 0)));
    }

    #[test]
    fn glob_rules() {
        let pats: Vec<String> = DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect();
        assert!(is_excluded("generated/foo.rs", &pats));
        assert!(is_excluded("src/generated/foo.rs", &pats));
        assert!(is_excluded("vendor/lib.rs", &pats));
        assert!(!is_excluded("src/main.rs", &pats));
        assert!(!is_excluded("generatedx/foo.rs", &pats));
    }

    #[test]
    fn parse_new_ranges_basic() {
        let diff = "\
diff --git a/f.rs b/f.rs
index a93156a..1661c07 100644
--- a/f.rs
+++ b/f.rs
@@ -4,0 +5 @@ fn b() {
+let y=2;
";
        let (ranges, added) = parse_new_ranges(diff);
        assert_eq!(ranges, vec![LineRange::new(5, 5)]);
        assert_eq!(added, 1);
    }
}
