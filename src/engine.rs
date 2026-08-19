//! L2 — E3 engine: rustfmt (stable) whole-file formatting clipped to the
//! caller's ranges via diff-hunk intersection.
//!
//! Core idea: run rustfmt once on the whole file, diff original vs formatted,
//! then keep only hunks that overlap the scoped ranges. The result is a
//! minimal, valid unified diff — the formatter is a transformation, not a
//! file writer.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use similar::{ChangeTag, DiffOp, TextDiff};

use crate::types::{LineRange, ScopedFile};

#[derive(Debug)]
pub enum EngineError {
    ReadFailed { path: String, err: String },
    NotUtf8 { path: String },
    RustfmtFailed { path: String, stderr: String },
    TimedOut { path: String },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::ReadFailed { path, err } => write!(f, "cannot read {path}: {err}"),
            EngineError::NotUtf8 { path } => write!(f, "{path} is not valid UTF-8"),
            EngineError::RustfmtFailed { path, stderr } => {
                write!(f, "rustfmt failed on {path}: {}", stderr.trim())
            }
            EngineError::TimedOut { path } => write!(f, "rustfmt timed out on {path}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Per-file formatting result. `patch` is the clipped unified diff (None when
/// the file is unchanged); `new_content` supports `--apply` without re-reading
/// the disk (no TOCTOU between validation and write). `idempotent` is true
/// when formatting the formatted output yields the same bytes again (the
/// formatter reached a stable point); a false value fails the
/// `engine.idempotent` gate.
#[derive(Debug)]
pub struct FormatResult {
    pub path: String,
    pub engine: String,
    pub changed: bool,
    pub idempotent: bool,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub hunks_total: usize,
    pub hunks_kept: usize,
    pub patch: Option<String>,
    pub new_content: Option<String>,
}

pub struct Engine {
    pub rustfmt_path: String,
    pub timeout_secs: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            rustfmt_path: "rustfmt".to_string(),
            timeout_secs: 30,
        }
    }
}

/// Find the nearest Cargo.toml edition for `rel_path` inside `cwd`.
pub fn detect_edition(cwd: &Path, rel_path: &str) -> Option<String> {
    let mut dir = PathBuf::from(rel_path);
    dir.pop(); // file -> its directory
    loop {
        let candidate = cwd.join(&dir).join("Cargo.toml");
        if candidate.is_file() {
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                for line in text.lines() {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("edition") {
                        if let Some(eq) = rest.find('=') {
                            let val = rest[eq + 1..].trim().trim_matches('"');
                            if val.len() == 4 && val.chars().all(|c| c.is_ascii_digit()) {
                                return Some(val.to_string());
                            }
                        }
                    }
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Find a rustfmt config file in the repo root (rustfmt.toml or .rustfmt.toml).
pub fn find_rustfmt_config(cwd: &Path) -> Option<PathBuf> {
    for name in ["rustfmt.toml", ".rustfmt.toml"] {
        let p = cwd.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Run a command with a hard timeout (std has no built-in); kills on expiry.
/// `input` is written to the child's stdin (None closes stdin immediately).
fn run_with_timeout(
    mut cmd: Command,
    input: Option<&str>,
    secs: u64,
) -> Result<std::process::Output, EngineError> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| EngineError::RustfmtFailed {
            path: String::new(),
            stderr: format!("spawn failed: {e}"),
        })?;
    match input {
        Some(data) => {
            let mut stdin = child.stdin.take().expect("stdin piped");
            let owned = data.to_string();
            std::thread::spawn(move || {
                use std::io::Write;
                let _ = stdin.write_all(owned.as_bytes());
            });
        }
        None => {
            drop(child.stdin.take());
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| EngineError::RustfmtFailed {
            path: String::new(),
            stderr: format!("wait failed: {e}"),
        })? {
            let mut out = child.stdout.take().unwrap();
            let mut err = child.stderr.take().unwrap();
            use std::io::Read;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let _ = out.read_to_end(&mut stdout);
            let _ = err.read_to_end(&mut stderr);
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineError::TimedOut {
                path: "(unknown)".to_string(),
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn clip_op_spans(group: &[DiffOp]) -> (Vec<(usize, usize)>, Vec<usize>) {
    // Changed A-side (original) spans: [old_index, old_index+old_len) for
    // Delete/Replace. Insert-only positions: old_index (0-based) where the
    // insertion happens (A-side has no line there).
    let mut spans = Vec::new();
    let mut insert_positions = Vec::new();
    for op in group {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => spans.push((*old_index, old_index + old_len)),
            DiffOp::Insert { old_index, .. } => insert_positions.push(*old_index),
            DiffOp::Replace {
                old_index, old_len, ..
            } => spans.push((*old_index, old_index + old_len)),
        }
    }
    (spans, insert_positions)
}

/// Does the group's changed region intersect any scoped range?
/// `ranges` are 1-based inclusive; converted to 0-based half-open.
fn group_in_scope(group: &[DiffOp], ranges: &[LineRange]) -> bool {
    if ranges.is_empty() {
        // No ranges -> caller asked for whole-file formatting.
        return true;
    }
    let (spans, inserts) = clip_op_spans(group);
    for (s, e) in spans {
        for r in ranges {
            let (rs, re) = r.as_half_open();
            if s < re && rs < e {
                return true;
            }
        }
    }
    for p in inserts {
        for r in ranges {
            let (rs, re) = r.as_half_open();
            if rs <= p && p < re {
                return true;
            }
        }
    }
    false
}

fn op_old_len(op: &DiffOp) -> usize {
    match op {
        DiffOp::Equal { len, .. } => *len,
        DiffOp::Delete { old_len, .. } => *old_len,
        DiffOp::Replace { old_len, .. } => *old_len,
        DiffOp::Insert { .. } => 0,
    }
}

fn op_new_len(op: &DiffOp) -> usize {
    match op {
        DiffOp::Equal { len, .. } => *len,
        DiffOp::Insert { new_len, .. } => *new_len,
        DiffOp::Replace { new_len, .. } => *new_len,
        DiffOp::Delete { .. } => 0,
    }
}

/// Emit one grouped hunk as unified-diff text (with its context).
fn emit_group(diff: &TextDiff<'_, '_, '_, str>, group: &[DiffOp]) -> String {
    let old_index = match group.first().expect("non-empty group") {
        DiffOp::Equal { old_index, .. }
        | DiffOp::Delete { old_index, .. }
        | DiffOp::Replace { old_index, .. }
        | DiffOp::Insert { old_index, .. } => *old_index,
    };
    let new_index = match group.first().expect("non-empty group") {
        DiffOp::Equal { new_index, .. }
        | DiffOp::Delete { new_index, .. }
        | DiffOp::Insert { new_index, .. }
        | DiffOp::Replace { new_index, .. } => *new_index,
    };
    let old_len: usize = group.iter().map(op_old_len).sum();
    let new_len: usize = group.iter().map(op_new_len).sum();

    let mut out = String::new();
    let old_start = old_index + 1;
    let new_start = new_index + 1;
    if old_len == 0 {
        out.push_str(&format!("@@ -{old_start},0 +{new_start},{new_len} @@\n"));
    } else if new_len == 0 {
        out.push_str(&format!("@@ -{old_start},{old_len} +{new_start},0 @@\n"));
    } else {
        out.push_str(&format!(
            "@@ -{old_start},{old_len} +{new_start},{new_len} @@\n"
        ));
    }
    for op in group {
        for change in diff.iter_changes(op) {
            let tag = match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            };
            let value = change.value();
            out.push(tag);
            out.push_str(value);
            if !value.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Build the clipped patch for one file. Returns (patch_text, added, removed,
/// hunks_total, hunks_kept, new_content).
fn build_clipped(
    original: &str,
    formatted: &str,
    ranges: &[LineRange],
) -> (String, usize, usize, usize, usize, String) {
    let diff = TextDiff::from_lines(original, formatted);
    let groups = diff.grouped_ops(3);
    let mut kept: Vec<Vec<DiffOp>> = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    for group in &groups {
        if group_in_scope(group, ranges) {
            kept.push(group.clone());
            for op in group {
                match op {
                    DiffOp::Insert { new_len, .. } => added += new_len,
                    DiffOp::Delete { old_len, .. } => removed += old_len,
                    DiffOp::Replace {
                        old_len, new_len, ..
                    } => {
                        removed += old_len;
                        added += new_len;
                    }
                    DiffOp::Equal { .. } => {}
                }
            }
        }
    }

    let mut patch = String::new();
    for group in &kept {
        patch.push_str(&emit_group(&diff, group));
    }
    let new_content = apply_kept(original, &diff, &kept);

    (patch, added, removed, groups.len(), kept.len(), new_content)
}

/// Reconstruct the formatted content keeping only the kept groups' changes.
///
/// Line model: `split_inclusive('\n')` keeps each line's terminator, so a
/// trailing newline (or its absence) survives the splice exactly.
fn apply_kept(original: &str, diff: &TextDiff<'_, '_, '_, str>, kept: &[Vec<DiffOp>]) -> String {
    let original_lines: Vec<&str> = original.split_inclusive('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0usize;

    for group in kept {
        // old range of the group
        let old_start = match group.first().unwrap() {
            DiffOp::Equal { old_index, .. }
            | DiffOp::Delete { old_index, .. }
            | DiffOp::Replace { old_index, .. }
            | DiffOp::Insert { old_index, .. } => *old_index,
        };
        let old_end = group.iter().fold(old_start, |acc, op| {
            acc + match op {
                DiffOp::Equal { len, .. } => *len,
                DiffOp::Delete { old_len, .. } => *old_len,
                DiffOp::Replace { old_len, .. } => *old_len,
                DiffOp::Insert { .. } => 0,
            }
        });
        // copy untouched prefix
        let hi = old_start.min(original_lines.len());
        out.extend(original_lines[cursor..hi].iter().map(|s| s.to_string()));
        // copy new-side lines of the group (values keep their terminators)
        for op in group {
            for change in diff.iter_changes(op) {
                if change.tag() == ChangeTag::Delete {
                    continue;
                }
                out.push(change.value().to_string());
            }
        }
        cursor = old_end.min(original_lines.len());
    }
    out.extend(original_lines[cursor..].iter().map(|s| s.to_string()));
    out.concat()
}

/// Run rustfmt on `content` (via stdin) and return the formatted stdout.
fn run_rustfmt(
    engine: &Engine,
    file_path: &str,
    edition: Option<&str>,
    config_path: Option<&Path>,
    content: &str,
) -> Result<String, EngineError> {
    let mut cmd = Command::new(&engine.rustfmt_path);
    cmd.arg("--emit").arg("stdout");
    if let Some(edition) = edition {
        cmd.arg("--edition").arg(edition);
    }
    if let Some(cfg) = config_path {
        cmd.arg("--config-path").arg(cfg);
    }

    // Feed the file through stdin: rustfmt does not print the "<path>:"
    // header for stdin input, and we format exactly the bytes we diffed
    // (no re-read TOCTOU between validation and apply).
    let out = run_with_timeout(cmd, Some(content), engine.timeout_secs).map_err(|e| match e {
        EngineError::TimedOut { .. } => EngineError::TimedOut {
            path: file_path.to_string(),
        },
        other => other,
    })?;
    if !out.status.success() {
        return Err(EngineError::RustfmtFailed {
            path: file_path.to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Format one scoped file with E3 (rustfmt + diff intersection).
pub fn format_file(
    engine: &Engine,
    cwd: &Path,
    file: &ScopedFile,
    config_path: Option<&Path>,
) -> Result<FormatResult, EngineError> {
    let abs_path = cwd.join(&file.path);
    let original = std::fs::read_to_string(&abs_path).map_err(|e| EngineError::ReadFailed {
        path: file.path.clone(),
        err: e.to_string(),
    })?;
    let original = match String::from_utf8(original.into_bytes()) {
        Ok(s) => s,
        Err(_) => {
            return Err(EngineError::NotUtf8 {
                path: file.path.clone(),
            })
        }
    };

    let edition = detect_edition(cwd, &file.path);
    let formatted = run_rustfmt(
        engine,
        &file.path,
        edition.as_deref(),
        config_path,
        &original,
    )?;

    if formatted == original {
        return Ok(FormatResult {
            path: file.path.clone(),
            engine: "rustfmt-diff-intersect".to_string(),
            changed: false,
            idempotent: true,
            added_lines: 0,
            removed_lines: 0,
            hunks_total: 0,
            hunks_kept: 0,
            patch: None,
            new_content: None,
        });
    }

    // Idempotency: formatting the formatted output must be a no-op. A
    // formatter that keeps moving is a formatter that will fight the next
    // run — fail closed.
    let formatted2 = run_rustfmt(
        engine,
        &file.path,
        edition.as_deref(),
        config_path,
        &formatted,
    )?;
    let idempotent = formatted2 == formatted;

    let (patch, added, removed, total, kept, new_content) =
        build_clipped(&original, &formatted, &file.ranges);

    Ok(FormatResult {
        path: file.path.clone(),
        engine: "rustfmt-diff-intersect".to_string(),
        changed: kept > 0,
        idempotent,
        added_lines: added,
        removed_lines: removed,
        hunks_total: total,
        hunks_kept: kept,
        patch: if kept > 0 { Some(patch) } else { None },
        new_content: if kept > 0 { Some(new_content) } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: usize, e: usize) -> LineRange {
        LineRange::new(s, e)
    }

    #[test]
    fn clip_keeps_overlapping_only() {
        // changes at line 2 (b->B) and line 10 (j->J); 7 lines between
        // (> 2*context) -> two separate hunks. Range [2,2] keeps only the first.
        let original = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
        let formatted = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nk\n";
        let (patch, added, removed, total, kept, new) =
            build_clipped(original, formatted, &[r(2, 2)]);
        assert_eq!(total, 2);
        assert_eq!(kept, 1);
        assert!(patch.contains("+B"));
        assert!(!patch.contains("+J"));
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
        assert_eq!(new, "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\n");
    }

    #[test]
    fn clip_whole_file_when_no_ranges() {
        let original = "a\nb\nc\n";
        let formatted = "a\nBB\nc\n";
        let (patch, added, removed, total, kept, new) = build_clipped(original, formatted, &[]);
        assert_eq!(kept, 1);
        assert_eq!(total, 1);
        assert!(patch.contains("+BB"));
        assert_eq!(added, 1);
        assert_eq!(removed, 1);
        assert_eq!(new, "a\nBB\nc\n");
    }

    #[test]
    fn apply_roundtrip() {
        let original = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
        let formatted = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nk\n";
        let (_, _, _, _, _, new) = build_clipped(original, formatted, &[r(10, 10)]);
        // only the second hunk kept: line 10 j->J
        assert_eq!(new, "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\nk\n");
    }

    #[test]
    fn apply_preserves_no_trailing_newline() {
        let original = "a\nb";
        let formatted = "a\nB";
        let (_, _, _, _, _, new) = build_clipped(original, formatted, &[r(2, 2)]);
        assert_eq!(new, "a\nB");
    }

    #[test]
    fn insert_only_hunk_kept_when_position_in_range() {
        let original = "a\nb\nc\nd\ne\n";
        let formatted = "a\nb\nX\nc\nd\ne\n";
        let (patch, _, _, _, kept, _) = build_clipped(original, formatted, &[r(3, 3)]);
        assert_eq!(kept, 1);
        assert!(patch.contains("+X"));
    }
}
