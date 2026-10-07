//! Event-log lifecycle (P1e): corruption quarantine and retention pruning.
//!
//! The ledger is the single source of truth, so both operations are
//! conservative: nothing is ever deleted, and anything moved aside keeps its
//! bytes. Rotation renames a damaged log (`runs.jsonl.corrupt-<ts>`), pruning
//! appends the dropped runs to `.fmtguard/archive/` before rewriting the log
//! through a temp file + rename.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How much of the tail we inspect when checking log health.
const TAIL_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunMeta {
    pub run_id: String,
    pub events: usize,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrunePlan {
    pub log: String,
    pub mode: String,
    pub total_runs: usize,
    pub kept_runs: usize,
    pub pruned_runs: Vec<RunMeta>,
    pub archive: Option<String>,
}

/// Signed-integer tail check: returns the path the damaged log was renamed to.
///
/// A log is healthy when it is empty, or ends with a newline **and** its last
/// non-empty line parses as JSON. A partial last line means a writer died
/// mid-append; appending after that would corrupt the record permanently.
pub fn quarantine_if_corrupt(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let len = std::fs::metadata(path)?.len();
    if len == 0 {
        return Ok(None);
    }
    let start = len.saturating_sub(TAIL_BYTES);
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail)?;

    let ends_with_newline = tail.last() == Some(&b'\n');
    let last_line = tail
        .split(|b| *b == b'\n')
        .rev()
        .find(|l| !l.iter().all(|c| c.is_ascii_whitespace()))
        .unwrap_or(&[]);
    let parses =
        last_line.is_empty() || serde_json::from_slice::<serde_json::Value>(last_line).is_ok();

    if ends_with_newline && parses {
        return Ok(None);
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut dest = path.as_os_str().to_owned();
    dest.push(format!(".corrupt-{stamp}"));
    let dest = PathBuf::from(dest);
    std::fs::rename(path, &dest)?;
    Ok(Some(dest))
}

/// Parse `90s` / `30m` / `24h` / `7d`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let t = s.trim();
    let (num, unit) = t.split_at(
        t.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("invalid duration: {s} (expected e.g. 7d, 24h, 30m, 90s)"))?,
    );
    let n: u64 = num
        .parse()
        .map_err(|_| format!("invalid duration: {s} (expected e.g. 7d, 24h, 30m, 90s)"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        other => return Err(format!("unknown duration unit: {other} (s|m|h|d)")),
    };
    Ok(Duration::from_secs(secs))
}

fn ts_to_epoch_millis(ts: &str) -> Option<i64> {
    // "YYYY-MM-DDTHH:MM:SS.mmmZ" — our own format only; anything else is
    // treated as unknown rather than guessed.
    let b = ts.as_bytes();
    if b.len() != 24 || b[4] != b'-' || b[10] != b'T' || b[19] != b'.' || b[23] != b'Z' {
        return None;
    }
    let y: i64 = ts[0..4].parse().ok()?;
    let mo: i64 = ts[5..7].parse().ok()?;
    let d: i64 = ts[8..10].parse().ok()?;
    let h: i64 = ts[11..13].parse().ok()?;
    let mi: i64 = ts[14..16].parse().ok()?;
    let s: i64 = ts[17..19].parse().ok()?;
    let ms: i64 = ts[20..23].parse().ok()?;
    // days-from-civil (inverse of the formatter in events.rs)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 + s) * 1000) + ms)
}

/// Split the log into runs (each beginning at a `run_start` line).
fn split_runs(text: &str) -> Result<Vec<(RunMeta, Vec<usize>)>, String> {
    let mut runs: Vec<(RunMeta, Vec<usize>)> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("corrupt event log at line {}: {e}", idx + 1))?;
        let t = v["t"].as_str().unwrap_or("");
        if t == "run_start" {
            runs.push((
                RunMeta {
                    run_id: v["run_id"].as_str().unwrap_or("").to_string(),
                    events: 0,
                    first_ts: v["ts"].as_str().map(|s| s.to_string()),
                    last_ts: v["ts"].as_str().map(|s| s.to_string()),
                },
                Vec::new(),
            ));
        } else if runs.is_empty() {
            return Err(format!(
                "corrupt event log: event `{t}` at line {} has no preceding run_start",
                idx + 1
            ));
        }
        let last = runs.last_mut().expect("non-empty");
        last.1.push(idx);
        last.0.events += 1;
        if let Some(ts) = v["ts"].as_str() {
            last.0.last_ts = Some(ts.to_string());
        }
    }
    Ok(runs)
}

/// Compute (and optionally apply) a retention policy.
///
/// Exactly one selector must be given: `keep` (newest N runs survive) or
/// `older_than` (runs whose first event is older than this are dropped).
/// Refusing to guess is deliberate — a retention command that silently picks a
/// policy is how ledgers disappear.
pub fn prune(
    path: &Path,
    keep: Option<usize>,
    older_than: Option<Duration>,
    apply: bool,
) -> Result<PrunePlan, String> {
    if keep.is_some() == older_than.is_some() {
        return Err(
            "choose exactly one retention selector: --keep-runs N or --older-than DUR".to_string(),
        );
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let runs = split_runs(&text)?;
    let lines: Vec<&str> = text.lines().collect();

    let prune_indices: Vec<usize> = if let Some(n) = keep {
        (0..runs.len().saturating_sub(n)).collect()
    } else {
        let d = older_than.expect("selector checked above");
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
            - d.as_millis() as i64;
        let mut idx = Vec::new();
        for (i, (meta, _)) in runs.iter().enumerate() {
            match meta.first_ts.as_deref().and_then(ts_to_epoch_millis) {
                Some(ms) if ms < cutoff => idx.push(i),
                Some(_) => {}
                None => {
                    return Err(format!(
                        "run {} has no usable `ts` (log written before timestamps existed); \
                         use --keep-runs N instead of --older-than",
                        meta.run_id
                    ))
                }
            }
        }
        idx
    };

    let mut pruned = Vec::new();
    let mut pruned_lines: Vec<&str> = Vec::new();
    for &i in &prune_indices {
        pruned.push(runs[i].0.clone());
        for &li in &runs[i].1 {
            pruned_lines.push(lines[li]);
        }
    }
    let kept_runs: Vec<&(RunMeta, Vec<usize>)> = runs
        .iter()
        .enumerate()
        .filter(|(i, _)| !prune_indices.contains(i))
        .map(|(_, r)| r)
        .collect();

    let mut plan = PrunePlan {
        log: path.display().to_string(),
        mode: if apply { "apply" } else { "dry-run" }.to_string(),
        total_runs: runs.len(),
        kept_runs: kept_runs.len(),
        pruned_runs: pruned,
        archive: None,
    };

    if !apply || pruned_lines.is_empty() {
        return Ok(plan);
    }

    // 1) archive the dropped runs (never a delete)
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("archive");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let from = plan
        .pruned_runs
        .first()
        .and_then(|r| r.first_ts.clone())
        .unwrap_or_else(|| "unknown".to_string())
        .replace([':', '.'], "");
    let to = plan
        .pruned_runs
        .last()
        .and_then(|r| r.last_ts.clone())
        .unwrap_or_else(|| "unknown".to_string())
        .replace([':', '.'], "");
    let archive = dir.join(format!("runs-{from}-{to}.jsonl"));
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&archive)
            .map_err(|e| format!("cannot open {}: {e}", archive.display()))?;
        for l in &pruned_lines {
            writeln!(f, "{l}").map_err(|e| format!("cannot write archive: {e}"))?;
        }
        f.flush()
            .map_err(|e| format!("cannot flush archive: {e}"))?;
    }

    // 2) rewrite the live log atomically: temp file + rename
    let mut kept_text = String::new();
    for (_, indices) in &kept_runs {
        for &li in indices {
            kept_text.push_str(lines[li]);
            kept_text.push('\n');
        }
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, kept_text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))?;

    plan.archive = Some(archive.display().to_string());
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(604_800));
        assert!(parse_duration("7").is_err());
        assert!(parse_duration("7w").is_err());
    }

    #[test]
    fn timestamp_roundtrip_matches_events_formatter() {
        let s = crate::events::now_rfc3339_millis();
        let ms = ts_to_epoch_millis(&s).expect("our own format parses");
        let back = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!((back - ms).abs() < 5000, "roundtrip drifted: {s} -> {ms}");
        assert!(ts_to_epoch_millis("2026-10-07 11:41:02").is_none());
    }

    #[test]
    fn health_check_quarantines_a_truncated_tail() {
        let dir = std::env::temp_dir().join(format!("fmtguard-logctl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("runs.jsonl");
        std::fs::write(
            &log,
            "{\"t\":\"run_start\",\"run_id\":\"r1\"}\n{\"t\":\"scope_de",
        )
        .unwrap();
        let moved = quarantine_if_corrupt(&log)
            .unwrap()
            .expect("must quarantine");
        assert!(moved.exists());
        assert!(!log.exists());
        let _ = std::fs::remove_file(&moved);
        // healthy log stays put
        std::fs::write(&log, "{\"t\":\"run_start\",\"run_id\":\"r1\"}\n").unwrap();
        assert!(quarantine_if_corrupt(&log).unwrap().is_none());
        let _ = std::fs::remove_file(&log);
    }
}
