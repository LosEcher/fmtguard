//! G3 — Event-sourced mutation log. Append-only JSONL is the single source of
//! truth; reports, stats and audits are derived projections. Replay/audit
//! recovery is structurally free.

use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);
/// The tail is inspected once per process: every append this process makes ends
/// with a newline, so re-reading the tail for each event would be pure I/O.
static HEALTH_CHECKED: AtomicBool = AtomicBool::new(false);

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
        /// Untracked `.rs` files the VCS diff cannot see (P1f-2).
        #[serde(default)]
        untracked: Vec<String>,
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
        out_of_scope_hunks: usize,
        rustfmt_duration_ms: u128,
        rustfmt_first_pass_ms: u128,
        rustfmt_idempotency_pass_ms: u128,
        /// Time spent in fmtguard's own diff/clip step (0 in logs written
        /// before this field existed).
        clip_ms: u128,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_cross_gap: Option<usize>,
        kept_lines: usize,
        scope_lines: usize,
        hunk_context: usize,
        /// Clipped unified diff for this file; stored so `fmtguard replay`
        /// can rebuild the original patch byte-for-byte.
        #[serde(skip_serializing_if = "Option::is_none")]
        patch: Option<&'a str>,
    },
    /// Result of the post-format cleanliness check for one file (P1d). Kept as
    /// its own event because the check runs after the per-file fmt_result.
    FmtCheck {
        file: &'a str,
        mode: &'a str,
        fmt_clean: bool,
        in_scope_debt_hunks: usize,
        out_of_scope_debt_hunks: usize,
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
    /// A run that aborted with exit 2 before any report could be emitted.
    /// Without this, a failed run left only a dangling `run_start` and the
    /// reason was lost (2026-10-07 sweep: cankey's timeouts were provable only
    /// from stderr in someone's scrollback).
    RunError {
        kind: &'a str,
        message: String,
        files_failed: usize,
    },
}

/// RFC 3339 UTC with millisecond precision, e.g. `2026-10-07T11:41:02.123Z`.
/// Implemented locally: the tool ships no date dependency.
pub fn now_rfc3339_millis() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // Howard Hinnant's civil-from-days: exact for the proleptic Gregorian
    // calendar, which is all an audit timestamp needs.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Append one event to the JSONL log (no-op if path is None).
pub fn append(log_path: Option<&Path>, event: &Event<'_>) -> std::io::Result<()> {
    let Some(path) = log_path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // A log whose last line is a partial write is renamed aside before we
    // touch it (P1e): appending after a torn line would corrupt the record for
    // good, and replay would refuse the whole file.
    if !HEALTH_CHECKED.swap(true, Ordering::Relaxed) {
        if let Some(moved) = crate::logctl::quarantine_if_corrupt(path)? {
            eprintln!(
                "fmtguard: event log had a torn tail; moved it to {}",
                moved.display()
            );
        }
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    // Stamp every event in one place: a timestamp that call sites could forget
    // is a timestamp the audit cannot rely on.
    let mut value =
        serde_json::to_value(event).map_err(|e| std::io::Error::other(e.to_string()))?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "ts".to_string(),
            serde_json::Value::String(now_rfc3339_millis()),
        );
    }
    let mut line =
        serde_json::to_string(&value).map_err(|e| std::io::Error::other(e.to_string()))?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::Event;

    #[test]
    fn fmt_result_serializes_rustfmt_duration() {
        let event = Event::FmtResult {
            file: "src/main.rs",
            changed: false,
            idempotent: true,
            added_lines: 0,
            removed_lines: 0,
            hunks_total: 0,
            hunks_kept: 0,
            out_of_scope_hunks: 0,
            rustfmt_duration_ms: 42,
            rustfmt_first_pass_ms: 25,
            rustfmt_idempotency_pass_ms: 17,
            clip_ms: 9,
            min_cross_gap: None,
            kept_lines: 0,
            scope_lines: 0,
            hunk_context: 3,
            patch: None,
        };
        let value = serde_json::to_value(event).unwrap();

        assert_eq!(value["t"], "fmt_result");
        assert_eq!(value["rustfmt_duration_ms"], 42);
        assert_eq!(value["rustfmt_first_pass_ms"], 25);
    }

    #[test]
    fn timestamps_look_like_rfc3339_utc() {
        let ts = super::now_rfc3339_millis();
        assert_eq!(ts.len(), 24, "unexpected timestamp shape: {ts}");
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[19..20], ".");
        let year: u32 = ts[..4].parse().expect("year");
        assert!((2020..2100).contains(&year), "implausible year in {ts}");
    }

    #[test]
    fn append_stamps_every_event() {
        let dir = std::env::temp_dir().join(format!("fmtguard-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("runs.jsonl");
        let _ = std::fs::remove_file(&log);
        super::append(
            Some(&log),
            &super::Event::ReportEmit {
                verdict: "error",
                files_changed: 0,
                total_added: 0,
                total_removed: 0,
                outputs: vec![],
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["t"], "report_emit");
        assert!(v["ts"].as_str().unwrap().ends_with('Z'));
        let _ = std::fs::remove_file(&log);
    }
}
