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
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub added_lines: usize,
    pub removed_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub version: &'static str,
    pub run_id: String,
    pub verdict: &'static str,
    pub mode: &'static str,
    pub scope: Scope,
    pub files: Vec<FileReport>,
    pub stats: Stats,
    pub gates: Vec<GateResult>,
    pub rejections: Vec<GateResult>,
    /// Unified diff of the scoped formatting (concatenated per-file diffs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
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
        })
        .collect();
    let files_changed = results.iter().filter(|r| r.changed).count();
    let total_added: usize = results.iter().map(|r| r.added_lines).sum();
    let total_removed: usize = results.iter().map(|r| r.removed_lines).sum();

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
        verdict: if all_pass { "ok" } else { "rejected" },
        mode,
        scope: scope.clone(),
        files,
        stats: Stats {
            files_scanned: results.len(),
            files_changed,
            added_lines: total_added,
            removed_lines: total_removed,
        },
        gates: gates.to_vec(),
        rejections,
        patch,
    }
}
