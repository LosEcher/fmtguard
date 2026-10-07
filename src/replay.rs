//! `fmtguard replay <runId>` — rebuild a run's report/patch from the
//! event-sourced JSONL log. The log is the single source of truth; reports
//! and patches are derived projections.
//!
//! Deliberately no `replay --apply`: a stored patch describes the file state
//! at run time; blindly re-applying it later could corrupt a file that has
//! since changed. Replay is for audit and reconstruction only (fail-closed).

use std::io::BufRead;
use std::path::Path;

use serde_json::Value;

use crate::gates::GateResult;
use crate::report::{FileReport, Report, Stats};
use crate::types::{Scope, ScopedFile, Vcs};

#[derive(Debug)]
pub enum ReplayError {
    LogMissing(String),
    RunNotFound(String),
    CorruptLog(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::LogMissing(p) => write!(f, "event log not found: {p}"),
            ReplayError::RunNotFound(id) => write!(f, "no run with id {id} in the log"),
            ReplayError::CorruptLog(msg) => write!(f, "corrupt event log: {msg}"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Rebuild the report of one run from the event log.
pub fn replay(run_id: &str, log_path: &Path) -> Result<Report, ReplayError> {
    let file = std::fs::File::open(log_path)
        .map_err(|_| ReplayError::LogMissing(log_path.display().to_string()))?;
    let reader = std::io::BufReader::new(file);

    // The log is append-only with a single writer and runs never interleave:
    // a RunStart event marks a run boundary.
    let mut events: Vec<Value> = Vec::new();
    let mut found = false;
    for line in reader.lines() {
        let line = line.map_err(|e| ReplayError::CorruptLog(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value =
            serde_json::from_str(&line).map_err(|e| ReplayError::CorruptLog(e.to_string()))?;
        if v["t"] == "run_start" {
            if found {
                break; // next run begins
            }
            if v["run_id"] == run_id {
                found = true;
            }
        }
        if found {
            events.push(v);
        }
    }
    if !found {
        return Err(ReplayError::RunNotFound(run_id.to_string()));
    }

    let mut base = String::new();
    let mut vcs: Option<Vcs> = None;
    let mut scope_files: Vec<ScopedFile> = Vec::new();
    let mut files: Vec<FileReport> = Vec::new();
    let mut gates: Vec<GateResult> = Vec::new();
    let mut patch_parts: Vec<(String, String)> = Vec::new(); // (path, diff)
    let mut verdict = "ok";
    let mut mode = "dry-run";
    let mut error: Option<String> = None;
    let mut saw_report_emit = false;
    let mut stats = Stats {
        files_scanned: 0,
        files_changed: 0,
        added_lines: 0,
        removed_lines: 0,
        out_of_scope_hunks: 0,
    };

    for v in &events {
        match v["t"].as_str().unwrap_or("") {
            "run_start" => {
                base = v["base"].as_str().unwrap_or("").to_string();
                vcs = v["vcs"].as_str().and_then(|s| match s {
                    "git" => Some(Vcs::Git),
                    "jj" => Some(Vcs::Jj),
                    _ => None,
                });
                mode = if v["dry_run"].as_bool().unwrap_or(true) {
                    "dry-run"
                } else {
                    "apply"
                };
            }
            "scope_detect" => {
                if let Some(arr) = v["files"].as_array() {
                    for f in arr {
                        if let Some(p) = f.as_str() {
                            scope_files.push(ScopedFile {
                                path: p.to_string(),
                                ranges: Vec::new(),
                                agent_added_lines: None,
                                untracked: false,
                            });
                        }
                    }
                }
            }
            "fmt_result" => {
                let path = v["file"].as_str().unwrap_or("").to_string();
                let changed = v["changed"].as_bool().unwrap_or(false);
                let added = v["added_lines"].as_u64().unwrap_or(0) as usize;
                let removed = v["removed_lines"].as_u64().unwrap_or(0) as usize;
                let total = v["hunks_total"].as_u64().unwrap_or(0) as usize;
                let kept = v["hunks_kept"].as_u64().unwrap_or(0) as usize;
                let out_of_scope_hunks = v["out_of_scope_hunks"]
                    .as_u64()
                    .unwrap_or_else(|| total.saturating_sub(kept) as u64)
                    as usize;
                let rustfmt_duration_ms = v["rustfmt_duration_ms"].as_u64().unwrap_or(0) as u128;
                let rustfmt_first_pass_ms =
                    v["rustfmt_first_pass_ms"].as_u64().unwrap_or(0) as u128;
                let rustfmt_idempotency_pass_ms =
                    v["rustfmt_idempotency_pass_ms"].as_u64().unwrap_or(0) as u128;
                // Absent in logs written before the field existed.
                let clip_ms = v["clip_ms"].as_u64().unwrap_or(0) as u128;
                files.push(FileReport {
                    path: path.clone(),
                    engine: "rustfmt-diff-intersect".to_string(),
                    changed,
                    added_lines: added,
                    removed_lines: removed,
                    hunks_total: total,
                    hunks_kept: kept,
                    out_of_scope_hunks,
                    min_cross_gap: v["min_cross_gap"].as_u64().map(|n| n as usize),
                    kept_lines: v["kept_lines"].as_u64().unwrap_or(0) as usize,
                    scope_lines: v["scope_lines"].as_u64().unwrap_or(0) as usize,
                    hunk_context: v["hunk_context"].as_u64().unwrap_or(3) as usize,
                    in_scope_debt_hunks: None,
                    fmt_clean: None,
                    rustfmt_duration_ms,
                    rustfmt_first_pass_ms,
                    rustfmt_idempotency_pass_ms,
                    clip_ms,
                });
                if let Some(p) = v["patch"].as_str() {
                    if !p.is_empty() {
                        patch_parts.push((path.clone(), p.to_string()));
                    }
                }
                stats.files_scanned += 1;
            }
            "fmt_check" => {
                let path = v["file"].as_str().unwrap_or("");
                if let Some(f) = files.iter_mut().find(|f| f.path == path) {
                    f.fmt_clean = v["fmt_clean"].as_bool();
                    f.in_scope_debt_hunks = v["in_scope_debt_hunks"].as_u64().map(|n| n as usize);
                }
            }
            "gate_check" => {
                gates.push(GateResult {
                    gate: v["gate"].as_str().unwrap_or("").to_string(),
                    pass: v["pass"].as_bool().unwrap_or(false),
                    file: v["file"].as_str().map(|s| s.to_string()),
                    metric: v["metric"].as_f64(),
                    limit: v["limit"].as_f64(),
                    detail: v["detail"].as_str().unwrap_or("").to_string(),
                });
            }
            "run_error" => {
                let kind = v["kind"].as_str().unwrap_or("unknown");
                let message = v["message"].as_str().unwrap_or("");
                error = Some(format!("[{kind}] {message}"));
            }
            "report_emit" => {
                saw_report_emit = true;
                verdict = v["verdict"].as_str().unwrap_or("ok");
                stats.files_changed = v["files_changed"].as_u64().unwrap_or(0) as usize;
                stats.added_lines = v["total_added"].as_u64().unwrap_or(0) as usize;
                stats.removed_lines = v["total_removed"].as_u64().unwrap_or(0) as usize;
                stats.out_of_scope_hunks =
                    files.iter().map(|f| f.out_of_scope_hunks).sum::<usize>();
            }
            _ => {}
        }
    }

    // A run with no `report_emit` never finished: it crashed, was killed, or
    // lost its log tail. Reporting that as `ok` (the old default) is exactly
    // the kind of silent success this tool exists to prevent.
    if !saw_report_emit {
        verdict = "interrupted";
        if error.is_none() {
            error = Some("no report_emit event: the run aborted or was killed".to_string());
        }
    }

    // Rebuild the patch exactly as the original run emitted it.
    let patch = if patch_parts.is_empty() {
        None
    } else {
        let mut p = String::new();
        for (path, one) in &patch_parts {
            p.push_str(&format!("diff --git a/{path} b/{path}\n"));
            p.push_str(&format!("--- a/{path}\n"));
            p.push_str(&format!("+++ b/{path}\n"));
            p.push_str(one);
        }
        Some(p)
    };

    let mut untracked: Vec<String> = Vec::new();
    for v in &events {
        if v["t"] == "scope_detect" {
            if let Some(arr) = v["untracked"].as_array() {
                untracked = arr
                    .iter()
                    .filter_map(|s| s.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }
    }

    let scope = Scope {
        vcs,
        base,
        source: "replay".to_string(),
        files: scope_files,
        untracked,
    };

    let rejections: Vec<GateResult> = gates.iter().filter(|g| !g.pass).cloned().collect();

    Ok(Report {
        tool: "fmtguard",
        version: env!("CARGO_PKG_VERSION"),
        run_id: run_id.to_string(),
        verdict: verdict.to_string(),
        mode: mode.to_string(),
        scope,
        files,
        stats,
        gates,
        rejections,
        patch,
        error,
    })
}
