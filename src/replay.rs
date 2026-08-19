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
    let mut stats = Stats {
        files_scanned: 0,
        files_changed: 0,
        added_lines: 0,
        removed_lines: 0,
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
                files.push(FileReport {
                    path: path.clone(),
                    engine: "rustfmt-diff-intersect".to_string(),
                    changed,
                    added_lines: added,
                    removed_lines: removed,
                    hunks_total: total,
                    hunks_kept: kept,
                });
                if let Some(p) = v["patch"].as_str() {
                    if !p.is_empty() {
                        patch_parts.push((path.clone(), p.to_string()));
                    }
                }
                stats.files_scanned += 1;
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
            "report_emit" => {
                verdict = v["verdict"].as_str().unwrap_or("ok");
                stats.files_changed = v["files_changed"].as_u64().unwrap_or(0) as usize;
                stats.added_lines = v["total_added"].as_u64().unwrap_or(0) as usize;
                stats.removed_lines = v["total_removed"].as_u64().unwrap_or(0) as usize;
            }
            _ => {}
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

    let scope = Scope {
        vcs,
        base,
        source: "replay".to_string(),
        files: scope_files,
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
    })
}
