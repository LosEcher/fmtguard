//! Output layer: the FormatReport (machine JSON) and unified-diff patch.

use serde::Serialize;

use crate::engine::FormatResult;
use crate::gates::GateResult;
use crate::types::Scope;

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub engine: String,
    pub changed: bool,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub hunks_total: usize,
    pub hunks_kept: usize,
    pub out_of_scope_hunks: usize,
    /// Gap (unchanged lines) to the nearest out-of-scope hunk: the number that
    /// explains a budget rejection caused by hunk merging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_cross_gap: Option<usize>,
    pub kept_lines: usize,
    pub scope_lines: usize,
    pub hunk_context: usize,
    /// fmt-check results (P1d): `None` = the check did not run for this file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_scope_debt_hunks: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fmt_clean: Option<bool>,
    pub rustfmt_duration_ms: u128,
    pub rustfmt_first_pass_ms: u128,
    pub rustfmt_idempotency_pass_ms: u128,
    /// fmtguard-side diff/clip cost (excluded from the rustfmt pass timings).
    pub clip_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
    /// Formatting hunks the clip dropped because they sit outside the declared
    /// scope. Non-zero means "this verdict does not assert repo cleanliness".
    pub out_of_scope_hunks: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub version: &'static str,
    pub run_id: String,
    pub verdict: String,
    pub mode: String,
    pub scope: Scope,
    pub files: Vec<FileReport>,
    pub stats: Stats,
    pub gates: Vec<GateResult>,
    pub rejections: Vec<GateResult>,
    /// Unified diff of the scoped formatting (concatenated per-file diffs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// Present when the run (or the replayed run) aborted before emitting a
    /// normal report. `verdict == "error"` / `"interrupted"` accompanies it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn build_report(
    run_id: String,
    scope: &Scope,
    results: &[FormatResult],
    gates: &[GateResult],
    all_pass: bool,
    mode: &'static str,
) -> Report {
    let files: Vec<FileReport> = results
        .iter()
        .map(|r| FileReport {
            path: r.path.clone(),
            engine: r.engine.clone(),
            changed: r.changed,
            added_lines: r.added_lines,
            removed_lines: r.removed_lines,
            hunks_total: r.hunks_total,
            hunks_kept: r.hunks_kept,
            out_of_scope_hunks: r.hunks_total.saturating_sub(r.hunks_kept),
            min_cross_gap: r.min_cross_gap,
            kept_lines: r.kept_lines,
            scope_lines: r.scope_lines,
            hunk_context: r.hunk_context,
            in_scope_debt_hunks: r.in_scope_debt_hunks,
            fmt_clean: r.fmt_clean,
            rustfmt_duration_ms: r.rustfmt_duration_ms,
            rustfmt_first_pass_ms: r.rustfmt_first_pass_ms,
            rustfmt_idempotency_pass_ms: r.rustfmt_idempotency_pass_ms,
            clip_ms: r.clip_ms,
        })
        .collect();
    let files_changed = results.iter().filter(|r| r.changed).count();
    let total_added: usize = results.iter().map(|r| r.added_lines).sum();
    let total_removed: usize = results.iter().map(|r| r.removed_lines).sum();
    let total_out_of_scope: usize = results
        .iter()
        .map(|r| r.hunks_total.saturating_sub(r.hunks_kept))
        .sum();

    let patch = if files_changed > 0 {
        let mut p = String::new();
        for r in results {
            if let Some(one) = &r.patch {
                p.push_str(&format!("diff --git a/{} b/{}\n", r.path, r.path));
                p.push_str(&format!("--- a/{}\n", r.path));
                p.push_str(&format!("+++ b/{}\n", r.path));
                p.push_str(one);
            }
        }
        Some(p)
    } else {
        None
    };

    let rejections: Vec<GateResult> = gates.iter().filter(|g| !g.pass).cloned().collect();

    Report {
        tool: "fmtguard",
        version: env!("CARGO_PKG_VERSION"),
        run_id,
        verdict: if all_pass {
            "ok".to_string()
        } else {
            "rejected".to_string()
        },
        mode: mode.to_string(),
        scope: scope.clone(),
        files,
        stats: Stats {
            files_scanned: results.len(),
            files_changed,
            added_lines: total_added,
            removed_lines: total_removed,
            out_of_scope_hunks: total_out_of_scope,
        },
        gates: gates.to_vec(),
        rejections,
        patch,
        error: None,
    }
}
