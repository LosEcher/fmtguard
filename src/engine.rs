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

#[allow(dead_code)]
pub fn format_file_e1(
    cwd: &Path,
    file: &ScopedFile,
    binary: &str,
    timeout_secs: u64,
) -> Result<FormatResult, EngineError> {
    let started = std::time::Instant::now();
    let path = cwd.join(&file.path);
    let original = std::fs::read_to_string(&path).map_err(|e| EngineError::ReadFailed {
        path: file.path.clone(),
        err: e.to_string(),
    })?;
    let uri = format!("file://{}", path.display());
    let mut session = crate::lsp::Session::start(
        binary,
        &format!("file://{}", cwd.display()),
        std::time::Duration::from_secs(timeout_secs),
    )
    .map_err(|e| EngineError::RustfmtFailed {
        path: file.path.clone(),
        stderr: format!("E1 start failed: {e}"),
    })?;
    session
        .notify(&crate::lsp::did_open(&uri, "rust", &original))
        .map_err(|e| EngineError::RustfmtFailed {
            path: file.path.clone(),
            stderr: format!("E1 didOpen failed: {e}"),
        })?;
    let range = file
        .ranges
        .first()
        .copied()
        .unwrap_or(LineRange::new(1, original.lines().count().max(1)));
    let (sl, sc, el, ec) =
        crate::lsp::line_range(&original, range.start, range.end).map_err(|e| {
            EngineError::RustfmtFailed {
                path: file.path.clone(),
                stderr: format!("E1 range failed: {e}"),
            }
        })?;
    let edits = session
        .request(&crate::lsp::range_formatting(2, &uri, sl, sc, el, ec))
        .map_err(|e| EngineError::RustfmtFailed {
            path: file.path.clone(),
            stderr: format!("E1 formatting failed: {e}"),
        })?;
    let formatted = if edits.is_null() {
        original.clone()
    } else {
        crate::lsp::apply_text_edits(
            &original,
            edits.as_array().ok_or_else(|| EngineError::RustfmtFailed {
                path: file.path.clone(),
                stderr: "E1 result is not TextEdit[]".to_string(),
            })?,
        )
        .map_err(|e| EngineError::RustfmtFailed {
            path: file.path.clone(),
            stderr: format!("E1 edits failed: {e}"),
        })?
    };
    session.shutdown().map_err(|e| EngineError::RustfmtFailed {
        path: file.path.clone(),
        stderr: format!("E1 shutdown failed: {e}"),
    })?;
    let clip_started = std::time::Instant::now();
    let clip = build_clipped(&original, &formatted, &file.ranges, 3, 0);
    let clip_ms = clip_started.elapsed().as_millis();
    let kept = clip.hunks_kept;
    Ok(FormatResult {
        path: file.path.clone(),
        engine: "rust-analyzer-range".to_string(),
        changed: kept > 0,
        idempotent: true,
        added_lines: clip.added,
        removed_lines: clip.removed,
        hunks_total: clip.hunks_total,
        hunks_kept: kept,
        rustfmt_duration_ms: started.elapsed().as_millis(),
        rustfmt_first_pass_ms: started.elapsed().as_millis(),
        rustfmt_idempotency_pass_ms: 0,
        clip_ms,
        ranges: file.ranges.clone(),
        min_cross_gap: clip.min_cross_gap,
        kept_lines: clip.kept_lines,
        scope_lines: clip.scope_lines,
        hunk_context: 3,
        fmt_clean: None,
        in_scope_debt_hunks: None,
        patch: (kept > 0).then_some(clip.patch),
        new_content: (kept > 0).then_some(clip.new_content),
    })
}

#[derive(Debug)]
pub enum EngineError {
    ReadFailed {
        path: String,
        err: String,
    },
    NotUtf8 {
        path: String,
    },
    RustfmtFailed {
        path: String,
        stderr: String,
    },
    TimedOut {
        path: String,
        bytes: usize,
        lines: usize,
        timeout_secs: u64,
    },
    /// The clip diff exceeded its own budget: fmtguard cannot say whether the
    /// formatting stays inside the declared scope, so it refuses (exit 2). This
    /// is a tool-side limit, never a formatting verdict.
    DiffTooLarge {
        path: String,
        changed_lines: usize,
        timeout_secs: u64,
    },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::ReadFailed { path, err } => write!(f, "cannot read {path}: {err}"),
            EngineError::NotUtf8 { path } => write!(f, "{path} is not valid UTF-8"),
            EngineError::RustfmtFailed { path, stderr } => {
                write!(f, "rustfmt failed on {path}: {}", stderr.trim())
            }
            EngineError::TimedOut {
                path,
                bytes,
                lines,
                timeout_secs,
            } => write!(
                f,
                "rustfmt timed out on {path} ({bytes} bytes, {lines} lines, timeout {timeout_secs}s); split the changeset ranges or increase --engine-timeout-secs explicitly"
            ),
            EngineError::DiffTooLarge {
                path,
                changed_lines,
                timeout_secs,
            } => write!(
                f,
                "scope clipping hit its {timeout_secs}s budget on {path} ({changed_lines} changed \
                 line(s)): the formatter wants to rewrite too much of this file for the declared \
                 scope to be computed; split the change, or raise --diff-timeout-secs explicitly \
                 (0 disables the limit)"
            ),
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
    pub rustfmt_duration_ms: u128,
    pub rustfmt_first_pass_ms: u128,
    pub rustfmt_idempotency_pass_ms: u128,
    /// Time spent in fmtguard's own diff/clip step (whole-file diff +
    /// hunk grouping + `apply_kept`). Recorded separately because on files
    /// rustfmt reformats wholesale this — not rustfmt — dominates wall time.
    pub clip_ms: u128,
    /// The ranges this file was scoped to (needed by the fmt-check gate to
    /// tell "debt I introduced" from "debt that was already there").
    pub ranges: Vec<LineRange>,
    /// Smallest gap between a kept and a dropped hunk (budget-rejection
    /// diagnosis); `None` = every hunk landed on the same side of the scope.
    pub min_cross_gap: Option<usize>,
    /// Lines the kept hunks actually span vs the lines the caller declared.
    pub kept_lines: usize,
    pub scope_lines: usize,
    /// The diff context this run grouped with (`--hunk-context`).
    pub hunk_context: usize,
    /// Filled in by the fmt-check gate when it runs; `None` = not checked.
    pub fmt_clean: Option<bool>,
    pub in_scope_debt_hunks: Option<usize>,
    pub patch: Option<String>,
    pub new_content: Option<String>,
}

/// How strict the post-format cleanliness check is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FmtCheckMode {
    /// Default: the candidate may keep pre-existing (out-of-scope) debt, but
    /// must not leave *new* deviations inside the scoped ranges.
    Delta,
    /// The candidate must be a whole-file rustfmt fixed point. Only usable in
    /// a repo that already has zero debt — otherwise untouched debt rejects
    /// every run (2026-10-07 sweep, measured on /tmp/fg-debt).
    Strict,
}

#[derive(Debug, Clone)]
pub struct FmtCheck {
    pub mode: FmtCheckMode,
    pub fmt_clean: bool,
    pub in_scope_debt_hunks: usize,
    pub out_of_scope_debt_hunks: usize,
}

impl FmtCheck {
    pub fn pass(&self) -> bool {
        match self.mode {
            FmtCheckMode::Delta => self.in_scope_debt_hunks == 0,
            FmtCheckMode::Strict => self.fmt_clean,
        }
    }

    pub fn detail(&self) -> String {
        match self.mode {
            FmtCheckMode::Delta if self.in_scope_debt_hunks == 0 => format!(
                "no new formatting debt in scope ({} pre-existing out-of-scope hunk(s) left alone)",
                self.out_of_scope_debt_hunks
            ),
            FmtCheckMode::Delta => format!(
                "clip left {} new formatting hunk(s) inside the scoped ranges ({} out-of-scope)",
                self.in_scope_debt_hunks, self.out_of_scope_debt_hunks
            ),
            FmtCheckMode::Strict if self.fmt_clean => "candidate is rustfmt-clean".to_string(),
            FmtCheckMode::Strict => format!(
                "candidate is not rustfmt-clean ({} in-scope / {} out-of-scope deviating hunk(s)); \
                 use --verify-fmt-check=delta to gate only debt this change introduced",
                self.in_scope_debt_hunks, self.out_of_scope_debt_hunks
            ),
        }
    }
}

pub struct Engine {
    pub rustfmt_path: String,
    pub timeout_secs: u64,
    /// Diff context for hunk grouping: changes within `2*context` lines merge
    /// into one hunk and can no longer be separated by the scope.
    pub hunk_context: usize,
    /// Budget for fmtguard's own diff/clip step. `0` = unlimited. On expiry the
    /// run fails closed (exit 2) instead of applying a diff whose scope
    /// isolation could not be computed.
    pub diff_timeout_secs: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            rustfmt_path: "rustfmt".to_string(),
            timeout_secs: 30,
            hunk_context: 3,
            diff_timeout_secs: 10,
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

/// How long a reaped child's pipes may take to reach EOF before we stop
/// waiting. rustfmt does not fork, so this only guards against a stray
/// grandchild holding the write end open (the same hazard unirun bounds).
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Read a child pipe to EOF on its own thread and hand the bytes back over a
/// channel, so the parent may wait for exit with a deadline while the child is
/// still writing.
fn spawn_pipe_reader<R: std::io::Read + Send + 'static>(
    mut pipe: R,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// Run a command with a hard timeout (std has no built-in); kills on expiry.
/// `input` is written to the child's stdin (None closes stdin immediately).
///
/// stdout and stderr are drained **while the child runs**, on their own
/// threads. Waiting for exit first and reading afterwards deadlocks as soon as
/// the child writes more than one pipe buffer (64 KiB on Linux/macOS): the
/// child blocks on write, never exits, and `try_wait` never returns a status —
/// which then surfaces as a bogus "rustfmt timed out" on files rustfmt
/// formats in milliseconds.
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
                // stdin drops here, closing the pipe so the child sees EOF.
            });
        }
        None => {
            drop(child.stdin.take());
        }
    }

    let out_rx = spawn_pipe_reader(child.stdout.take().expect("stdout piped"));
    let err_rx = spawn_pipe_reader(child.stderr.take().expect("stderr piped"));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout =
                    out_rx
                        .recv_timeout(DRAIN_GRACE)
                        .map_err(|_| EngineError::RustfmtFailed {
                            path: String::new(),
                            stderr: "rustfmt exited but its stdout pipe never reached EOF \
                                 (a grandchild is holding it open)"
                                .to_string(),
                        })?;
                let stderr = err_rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::RustfmtFailed {
                    path: String::new(),
                    stderr: format!("wait failed: {e}"),
                });
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // Deliberately no join on the readers: a killed child is reaped, but
            // a grandchild could still hold the pipes; the process exits with
            // this error anyway, so leaked readers cannot outlive us meaningfully.
            return Err(EngineError::TimedOut {
                path: "(unknown)".to_string(),
                bytes: input.map_or(0, str::len),
                lines: input.map_or(0, |data| data.lines().count()),
                timeout_secs: secs,
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

/// Outcome of clipping one file's whole-file formatting to the caller's scope.
pub struct ClipOutcome {
    pub patch: String,
    pub added: usize,
    pub removed: usize,
    pub hunks_total: usize,
    pub hunks_kept: usize,
    pub new_content: String,
    /// Smallest gap (in unchanged lines) between a kept hunk and a dropped one.
    /// A small gap is *why* a run can blow the budget: `grouped_ops(context)`
    /// merges changes that are within `2*context` lines of each other, so an
    /// out-of-scope hunk 1 line away drags the caller's region with it.
    pub min_cross_gap: Option<usize>,
    /// Old-side line count covered by the kept hunks, and the line count the
    /// caller actually declared. A large ratio means the scope was extended by
    /// hunk merging (dense debt) rather than by the formatter running wild.
    pub kept_lines: usize,
    pub scope_lines: usize,
    /// True when the diff deadline was reached: the result is an approximation
    /// and must not be applied.
    pub deadline_hit: bool,
}

/// Build the clipped patch for one file. `context` is the diff context used for
/// hunk grouping (`--hunk-context`, default 3).
fn build_clipped(
    original: &str,
    formatted: &str,
    ranges: &[LineRange],
    context: usize,
    diff_timeout_secs: u64,
) -> ClipOutcome {
    let started = std::time::Instant::now();
    let diff = if diff_timeout_secs == 0 {
        TextDiff::from_lines(original, formatted)
    } else {
        TextDiff::configure()
            .timeout(std::time::Duration::from_secs(diff_timeout_secs))
            .diff_lines(original, formatted)
    };
    let groups = diff.grouped_ops(context);
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

    // Boundary diagnosis: walk consecutive groups and measure the gap between a
    // kept and a dropped neighbour.
    let mut min_cross_gap: Option<usize> = None;
    let mut spans: Vec<(usize, usize, bool)> = Vec::new();
    for group in &groups {
        let old_start = match group.first().expect("non-empty group") {
            DiffOp::Equal { old_index, .. }
            | DiffOp::Delete { old_index, .. }
            | DiffOp::Replace { old_index, .. }
            | DiffOp::Insert { old_index, .. } => *old_index,
        };
        let old_end = old_start
            + group
                .iter()
                .map(|op| match op {
                    DiffOp::Equal { len, .. } => *len,
                    DiffOp::Delete { old_len, .. } => *old_len,
                    DiffOp::Replace { old_len, .. } => *old_len,
                    DiffOp::Insert { .. } => 0,
                })
                .sum::<usize>();
        spans.push((old_start, old_end, group_in_scope(group, ranges)));
    }
    for pair in spans.windows(2) {
        let (_, end, kept_a) = pair[0];
        let (start_b, _, kept_b) = pair[1];
        if kept_a != kept_b {
            let gap = start_b.saturating_sub(end);
            min_cross_gap = Some(min_cross_gap.map_or(gap, |m: usize| m.min(gap)));
        }
    }

    let mut kept_lines = 0usize;
    for group in &kept {
        kept_lines += group
            .iter()
            .map(|op| match op {
                DiffOp::Equal { len, .. } => *len,
                DiffOp::Delete { old_len, .. } => *old_len,
                DiffOp::Replace { old_len, .. } => *old_len,
                DiffOp::Insert { .. } => 0,
            })
            .sum::<usize>();
    }
    let scope_lines = ranges
        .iter()
        .map(|r| r.end.saturating_sub(r.start) + 1)
        .sum();

    let mut patch = String::new();
    for group in &kept {
        patch.push_str(&emit_group(&diff, group));
    }
    let new_content = apply_kept(original, &diff, &kept);

    ClipOutcome {
        patch,
        added,
        removed,
        hunks_total: groups.len(),
        hunks_kept: kept.len(),
        new_content,
        min_cross_gap,
        kept_lines,
        scope_lines,
        deadline_hit: diff_timeout_secs > 0
            && started.elapsed() >= std::time::Duration::from_secs(diff_timeout_secs),
    }
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
        EngineError::TimedOut {
            bytes,
            lines,
            timeout_secs,
            ..
        } => EngineError::TimedOut {
            path: file_path.to_string(),
            bytes,
            lines,
            timeout_secs,
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
    let started = std::time::Instant::now();
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
    let first_pass_started = std::time::Instant::now();
    let formatted = run_rustfmt(
        engine,
        &file.path,
        edition.as_deref(),
        config_path,
        &original,
    )?;
    let rustfmt_first_pass_ms = first_pass_started.elapsed().as_millis();

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
            rustfmt_duration_ms: started.elapsed().as_millis(),
            rustfmt_first_pass_ms,
            rustfmt_idempotency_pass_ms: 0,
            clip_ms: 0,
            ranges: file.ranges.clone(),
            min_cross_gap: None,
            kept_lines: 0,
            scope_lines: 0,
            hunk_context: engine.hunk_context,
            fmt_clean: None,
            in_scope_debt_hunks: None,
            patch: None,
            new_content: None,
        });
    }

    // Idempotency: formatting the formatted output must be a no-op. A
    // formatter that keeps moving is a formatter that will fight the next
    // run — fail closed.
    let idempotency_started = std::time::Instant::now();
    let formatted2 = run_rustfmt(
        engine,
        &file.path,
        edition.as_deref(),
        config_path,
        &formatted,
    )?;
    let rustfmt_idempotency_pass_ms = idempotency_started.elapsed().as_millis();
    let idempotent = formatted2 == formatted;

    let clip_started = std::time::Instant::now();
    let clip = build_clipped(
        &original,
        &formatted,
        &file.ranges,
        engine.hunk_context,
        engine.diff_timeout_secs,
    );
    let clip_ms = clip_started.elapsed().as_millis();
    if clip.deadline_hit {
        return Err(EngineError::DiffTooLarge {
            path: file.path.clone(),
            changed_lines: clip.added + clip.removed,
            timeout_secs: engine.diff_timeout_secs,
        });
    }
    let kept = clip.hunks_kept;

    Ok(FormatResult {
        path: file.path.clone(),
        engine: "rustfmt-diff-intersect".to_string(),
        changed: kept > 0,
        idempotent,
        added_lines: clip.added,
        removed_lines: clip.removed,
        hunks_total: clip.hunks_total,
        hunks_kept: kept,
        rustfmt_duration_ms: started.elapsed().as_millis(),
        rustfmt_first_pass_ms,
        rustfmt_idempotency_pass_ms,
        clip_ms,
        ranges: file.ranges.clone(),
        min_cross_gap: clip.min_cross_gap,
        kept_lines: clip.kept_lines,
        scope_lines: clip.scope_lines,
        hunk_context: engine.hunk_context,
        fmt_clean: None,
        in_scope_debt_hunks: None,
        patch: if kept > 0 { Some(clip.patch) } else { None },
        new_content: if kept > 0 {
            Some(clip.new_content)
        } else {
            None
        },
    })
}

/// Post-format cleanliness check for one candidate.
///
/// `Strict` asks the whole file to be a rustfmt fixed point (the v0.2.x
/// behaviour). `Delta` asks only that the candidate introduce no *new*
/// deviation inside the scoped ranges: pre-existing out-of-scope debt is
/// counted and reported, not charged to this run. Both reuse the same diff
/// machinery as the clip, so "debt" means the same thing everywhere.
pub fn verify_fmt_check(
    engine: &Engine,
    cwd: &Path,
    result: &FormatResult,
    config_path: Option<&Path>,
    mode: FmtCheckMode,
) -> Result<Option<FmtCheck>, EngineError> {
    let Some(candidate) = result.new_content.as_deref() else {
        return Ok(None);
    };
    let edition = detect_edition(cwd, &result.path);
    let formatted = run_rustfmt(
        engine,
        &result.path,
        edition.as_deref(),
        config_path,
        candidate,
    )?;
    let fmt_clean = formatted == candidate;
    let clip = build_clipped(candidate, &formatted, &result.ranges, 3, 0);
    Ok(Some(FmtCheck {
        mode,
        fmt_clean,
        in_scope_debt_hunks: clip.hunks_kept,
        out_of_scope_debt_hunks: clip.hunks_total.saturating_sub(clip.hunks_kept),
    }))
}

/// Environment probe for `fmtguard doctor`: does the configured formatter
/// exist, report a version, and actually format a known snippet?
#[derive(Debug, Clone, serde::Serialize)]
pub struct RustfmtProbe {
    pub path: String,
    pub version: Option<String>,
    pub probe_ok: bool,
    pub probe_output: Option<String>,
    pub error: Option<String>,
}

const PROBE_SNIPPET: &str = "pub fn probe( x :u32)->u32{ x +1 }\n";
const PROBE_EXPECTED: &str = "pub fn probe(x: u32) -> u32 {\n    x + 1\n}\n";

pub fn probe_rustfmt(engine: &Engine) -> RustfmtProbe {
    let mut version_cmd = Command::new(&engine.rustfmt_path);
    version_cmd.arg("--version");
    let version = match run_with_timeout(version_cmd, None, engine.timeout_secs) {
        Ok(out) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        _ => None,
    };

    let mut cmd = Command::new(&engine.rustfmt_path);
    cmd.arg("--emit").arg("stdout");
    match run_with_timeout(cmd, Some(PROBE_SNIPPET), engine.timeout_secs) {
        Ok(out) if out.status.success() => {
            let formatted = String::from_utf8_lossy(&out.stdout).into_owned();
            if formatted == PROBE_EXPECTED {
                RustfmtProbe {
                    path: engine.rustfmt_path.clone(),
                    version,
                    probe_ok: true,
                    probe_output: None,
                    error: None,
                }
            } else {
                RustfmtProbe {
                    path: engine.rustfmt_path.clone(),
                    version,
                    probe_ok: false,
                    probe_output: Some(formatted),
                    error: Some(
                        "formatter ran but did not produce the expected output for the probe snippet"
                            .to_string(),
                    ),
                }
            }
        }
        Ok(out) => RustfmtProbe {
            path: engine.rustfmt_path.clone(),
            version,
            probe_ok: false,
            probe_output: None,
            error: Some(format!(
                "probe exit {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        },
        Err(e) => RustfmtProbe {
            path: engine.rustfmt_path.clone(),
            version,
            probe_ok: false,
            probe_output: None,
            error: Some(e.to_string()),
        },
    }
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
        let c = build_clipped(original, formatted, &[r(2, 2)], 3, 0);
        assert_eq!(c.hunks_total, 2);
        assert_eq!(c.hunks_kept, 1);
        assert!(c.patch.contains("+B"));
        assert!(!c.patch.contains("+J"));
        assert_eq!(c.added, 1);
        assert_eq!(c.removed, 1);
        assert_eq!(c.new_content, "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\n");
    }

    #[test]
    fn clip_whole_file_when_no_ranges() {
        let original = "a\nb\nc\n";
        let formatted = "a\nBB\nc\n";
        let c = build_clipped(original, formatted, &[], 3, 0);
        assert_eq!(c.hunks_kept, 1);
        assert_eq!(c.hunks_total, 1);
        assert!(c.patch.contains("+BB"));
        assert_eq!(c.added, 1);
        assert_eq!(c.removed, 1);
        assert_eq!(c.new_content, "a\nBB\nc\n");
    }

    #[test]
    fn apply_roundtrip() {
        let original = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
        let formatted = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nk\n";
        let c = build_clipped(original, formatted, &[r(10, 10)], 3, 0);
        // only the second hunk kept: line 10 j->J
        assert_eq!(c.new_content, "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\nk\n");
    }

    #[test]
    fn apply_preserves_no_trailing_newline() {
        let original = "a\nb";
        let formatted = "a\nB";
        let c = build_clipped(original, formatted, &[r(2, 2)], 3, 0);
        assert_eq!(c.new_content, "a\nB");
    }

    #[test]
    fn insert_only_hunk_kept_when_position_in_range() {
        let original = "a\nb\nc\nd\ne\n";
        let formatted = "a\nb\nX\nc\nd\ne\n";
        let c = build_clipped(original, formatted, &[r(3, 3)], 3, 0);
        assert_eq!(c.hunks_kept, 1);
        assert!(c.patch.contains("+X"));
    }

    /// Regression for the P0-1 deadlock: a child that writes far more than one
    /// pipe buffer (64 KiB) must not block us. Before the drain threads existed
    /// this test hit the deadline and returned `TimedOut`.
    #[cfg(unix)]
    #[test]
    fn output_larger_than_a_pipe_buffer_does_not_deadlock() {
        let payload = "x".repeat(300_000);
        let out = run_with_timeout(Command::new("cat"), Some(&payload), 10)
            .expect("cat with a 300 KB payload must finish, not time out");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), payload.len());
    }

    /// The timeout must stay fail-closed: a child that never exits is killed
    /// and reported as `TimedOut`, with the input size for diagnosis.
    #[cfg(unix)]
    #[test]
    fn timeout_still_kills_and_reports() {
        let mut cmd = Command::new("sleep");
        cmd.arg("3");
        let err = run_with_timeout(cmd, Some("3\n"), 1)
            .expect_err("sleep 3 with a 1s deadline must time out");
        match err {
            EngineError::TimedOut {
                bytes,
                timeout_secs,
                ..
            } => {
                assert_eq!(bytes, 2);
                assert_eq!(timeout_secs, 1);
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }
}
