//! G3 — Event-sourced mutation log. Append-only JSONL is the single source of
//! truth; reports, stats and audits are derived projections. Replay/audit
//! recovery is structurally free.

use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn new_run_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("run-{now:013}-{seq}")
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event<'a> {
    RunStart {
        run_id: String,
        version: &'a str,
        cwd: &'a str,
        base: &'a str,
        vcs: Option<&'a str>,
        source: &'a str,
        rustfmt: &'a str,
        toolchain: &'a str,
        dry_run: bool,
    },
    ScopeDetect {
        source: &'a str,
        files: Vec<String>,
        excluded: Vec<String>,
    },
    EngineSelect {
        file: &'a str,
        engine: &'a str,
        edition: Option<String>,
    },
    FmtResult {
        file: &'a str,
        changed: bool,
        idempotent: bool,
        added_lines: usize,
        removed_lines: usize,
        hunks_total: usize,
        hunks_kept: usize,
        /// Clipped unified diff for this file; stored so `fmtguard replay`
        /// can rebuild the original patch byte-for-byte.
        #[serde(skip_serializing_if = "Option::is_none")]
        patch: Option<&'a str>,
    },
    GateCheck {
        gate: &'a str,
        pass: bool,
        file: Option<&'a str>,
        metric: Option<f64>,
        limit: Option<f64>,
        /// Human-readable detail; stored so `fmtguard replay` rebuilds the
        /// report faithfully.
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<&'a str>,
    },
    ReportEmit {
        verdict: &'a str,
        files_changed: usize,
        total_added: usize,
        total_removed: usize,
        outputs: Vec<&'a str>,
    },
    Apply {
        dry_run: bool,
        applied_files: usize,
        refused: bool,
    },
}

/// Append one event to the JSONL log (no-op if path is None).
pub fn append(log_path: Option<&Path>, event: &Event<'_>) -> std::io::Result<()> {
    let Some(path) = log_path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line =
        serde_json::to_string(event).map_err(|e| std::io::Error::other(e.to_string()))?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    f.flush()
}
