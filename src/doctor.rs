//! `fmtguard doctor` — read-only environment probe.
//!
//! Motivation (2026-10-07 cross-project sweep): the whole fleet was still
//! running an old fmtguard binary (`run_start.version = 0.2.0` in 672 of 673
//! logged runs) while the repo and crates.io were several releases ahead, so
//! every capability added since never reached a user. A doctor with a machine
//! readable `--require-version` turns that into an exit code instead of prose.
//!
//! Exit codes follow the tool contract: 0 = healthy, 2 = this environment
//! cannot do the job (missing/broken formatter, unwritable log, version
//! requirement unmet). Nothing here is a formatting verdict.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine::{self, Engine, RustfmtProbe};

#[derive(Debug, Serialize)]
pub struct Engines {
    /// E3 is the default engine: available iff the probe succeeded.
    pub e3: String,
    /// E1 needs a rust-analyzer binary plus `rustfmt.rangeFormatting.enable`.
    pub e1: String,
    pub rust_analyzer: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LogInfo {
    pub path: String,
    pub exists: bool,
    pub bytes: Option<u64>,
    pub events: Option<u64>,
    /// `null` = unknown without creating anything (the log does not exist yet;
    /// see `parent_exists`). Never probed by writing.
    pub writable: Option<bool>,
    pub parent_exists: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfigInfo {
    pub rustfmt_config: Option<String>,
    pub edition: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VersionCheck {
    pub required: String,
    pub current: String,
    pub satisfied: bool,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub tool: &'static str,
    pub version: &'static str,
    pub cwd: String,
    pub vcs: Option<String>,
    pub rustfmt: RustfmtProbe,
    pub engines: Engines,
    pub config: ConfigInfo,
    pub log: LogInfo,
    pub require_version: Option<VersionCheck>,
    pub verdict: String,
    pub problems: Vec<String>,
}

/// Parse `X.Y.Z` / `vX.Y.Z`. Anything else is a usage error: a version gate
/// that silently accepts nonsense would defeat its own purpose.
pub fn parse_version(s: &str) -> Result<(u64, u64, u64), String> {
    let t = s.trim().trim_start_matches('v');
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("invalid version: {s} (expected MAJOR.MINOR.PATCH)"));
    }
    let mut nums = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        nums[i] = p
            .parse::<u64>()
            .map_err(|_| format!("invalid version: {s} (expected MAJOR.MINOR.PATCH)"))?;
    }
    Ok((nums[0], nums[1], nums[2]))
}

/// Is `current` at least `required`?
pub fn version_satisfies(current: &str, required: &str) -> Result<bool, String> {
    let c = parse_version(current)?;
    let r = parse_version(required)?;
    Ok(c >= r)
}

fn probe_rust_analyzer() -> Option<String> {
    let mut cmd = std::process::Command::new("rust-analyzer");
    cmd.arg("--version");
    let out = cmd.output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn inspect_log(path: &Path) -> LogInfo {
    let exists = path.is_file();
    let bytes = std::fs::metadata(path).ok().map(|m| m.len());
    let events = if exists {
        std::fs::read_to_string(path)
            .ok()
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count() as u64)
    } else {
        None
    };
    let writable = if exists {
        // Open for append without writing: this cannot truncate or create.
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .ok()
            .map(|_| true)
    } else {
        None
    };
    LogInfo {
        path: path.display().to_string(),
        exists,
        bytes,
        events,
        writable,
        parent_exists: path.parent().map(|p| p.is_dir()).unwrap_or(false),
    }
}

/// Build the doctor report. Read-only: no file is created, nothing is written.
pub fn diagnose(
    cwd: &Path,
    rustfmt_path: &str,
    timeout_secs: u64,
    log_path: &Path,
    require_version: Option<&str>,
) -> Result<DoctorReport, String> {
    let engine = Engine {
        rustfmt_path: rustfmt_path.to_string(),
        timeout_secs,
        hunk_context: 3,
        diff_timeout_secs: 10,
    };
    let rustfmt = engine::probe_rustfmt(&engine);
    let rust_analyzer = probe_rust_analyzer();

    let mut problems = Vec::new();
    if !rustfmt.probe_ok {
        problems.push(format!(
            "formatter is unusable: {}",
            rustfmt
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string())
        ));
    }

    let require_version = match require_version {
        Some(req) => {
            let satisfied = version_satisfies(env!("CARGO_PKG_VERSION"), req)?;
            if !satisfied {
                problems.push(format!(
                    "version requirement unmet: running {}, required >= {}",
                    env!("CARGO_PKG_VERSION"),
                    req
                ));
            }
            Some(VersionCheck {
                required: req.to_string(),
                current: env!("CARGO_PKG_VERSION").to_string(),
                satisfied,
            })
        }
        None => None,
    };

    let log = inspect_log(log_path);
    if log.exists && log.writable == Some(false) {
        problems.push(format!("event log is not writable: {}", log.path));
    }

    let vcs = crate::scope::detect_vcs(cwd).map(|v| match v {
        crate::types::Vcs::Git => "git".to_string(),
        crate::types::Vcs::Jj => "jj".to_string(),
    });

    let edition = engine::detect_edition(cwd, "probe.rs");

    let e3 = if rustfmt.probe_ok {
        "rustfmt-diff-intersect".to_string()
    } else {
        "unavailable (formatter probe failed)".to_string()
    };

    Ok(DoctorReport {
        tool: "fmtguard",
        version: env!("CARGO_PKG_VERSION"),
        cwd: cwd.display().to_string(),
        vcs,
        rustfmt,
        engines: Engines {
            e3,
            e1: if rust_analyzer.is_some() {
                "rust-analyzer-range (experimental; needs rustfmt.rangeFormatting.enable)"
                    .to_string()
            } else {
                "unavailable (rust-analyzer not found on PATH)".to_string()
            },
            rust_analyzer,
        },
        config: ConfigInfo {
            rustfmt_config: engine::find_rustfmt_config(cwd).map(|p| p.display().to_string()),
            edition,
        },
        log,
        require_version,
        verdict: if problems.is_empty() { "ok" } else { "error" }.to_string(),
        problems,
    })
}

pub fn default_doctor_log() -> PathBuf {
    PathBuf::from(".fmtguard/runs.jsonl")
}
