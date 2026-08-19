//! fmtguard — scoped, gated Rust formatting for AI agents and incremental
//! workflows.
//!
//! Exit codes:
//!   0  ok (or nothing to do)
//!   1  rejected by a mechanical gate (scope/budget/whitespace) — no writes
//!   2  error (usage, VCS, engine, I/O)

mod engine;
mod events;
mod gates;
mod replay;
mod report;
mod sandbox;
mod scope;
mod types;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::types::{ChangeSet, Scope, ScopedFile, Vcs};

const USAGE: &str = "\
fmtguard — scoped, gated Rust formatting for AI agents.

USAGE:
  fmtguard [OPTIONS]

SCOPE (choose one; default: auto-detect git or jj and diff against base):
  --scope-from-git            force git scope detection
  --scope-from-jj             force jj scope detection
  --changeset <file.json>     explicit scope; see CHANGESET below

OPTIONS:
  --base <ref>                VCS base for git (default: HEAD)
  --emit <json|patch>         machine output (default: json; 'diff' == 'patch')
  --apply                     write the validated patch (default: dry-run)
  --sandbox                   with --apply: verify the patch in an isolated
                              git worktree (git diff --check) before writing
                              the main tree (git only)
  --exclude <glob,...>        extra exclusion globs (defaults: generated/**,
                              vendor/**, target/**, node_modules/**)
  --budget-max-added-lines N  per-file formatter added-line cap (default 200)
  --budget-max-files N        max files changed by formatting (default 5)
  --budget-max-ratio R        formatter/agent added-line ratio cap (default 3.0)
  --rustfmt <path>            rustfmt binary (default: from PATH)
  --engine-timeout-secs N     rustfmt timeout (default 30)
  --log <path>                event log (default: .fmtguard/runs.jsonl)
  --no-log                    disable the event log
  --version                   print version
  --help                      print this help

CHANGESET (explicit scope, caller decides ranges):
  { \"base_ref\": \"HEAD\", \"files\": [
      { \"path\": \"src/router.rs\",
        \"ranges\": [{ \"start\": 120, \"end\": 180, \"reason\": \"added_handler\" }],
        \"agent_added_lines\": 30 }
  ] }
  Ranges are 1-based inclusive line ranges in the working tree; omitting
  ranges formats the whole file. agent_added_lines feeds the diff-ratio gate.

EXIT CODES: 0 ok · 1 rejected by a gate · 2 error.

SUBCOMMANDS:
  replay <runId> [--emit json|patch] [--log <path>]
                        rebuild a run's report/patch from the event log
                        (audit only; no re-apply).
";

#[derive(Debug)]
struct Config {
    vcs: Option<Vcs>,
    base: String,
    changeset: Option<PathBuf>,
    emit: Emit,
    apply: bool,
    sandbox: bool,
    excludes: Vec<String>,
    budget: gates::Budget,
    rustfmt: String,
    engine_timeout_secs: u64,
    log: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Emit {
    Json,
    Patch,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vcs: None,
            base: "HEAD".to_string(),
            changeset: None,
            emit: Emit::Json,
            apply: false,
            sandbox: false,
            excludes: scope::DEFAULT_EXCLUDES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            budget: gates::Budget::default(),
            rustfmt: "rustfmt".to_string(),
            engine_timeout_secs: 30,
            log: Some(PathBuf::from(".fmtguard/runs.jsonl")),
        }
    }
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("fmtguard {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--scope-from-git" => cfg.vcs = Some(Vcs::Git),
            "--scope-from-jj" => cfg.vcs = Some(Vcs::Jj),
            "--apply" => cfg.apply = true,
            "--sandbox" => cfg.sandbox = true,
            "--no-log" => cfg.log = None,
            "--base" => cfg.base = take_value(&mut it, "--base")?,
            "--changeset" => {
                cfg.changeset = Some(PathBuf::from(take_value(&mut it, "--changeset")?))
            }
            "--emit" => {
                let v = take_value(&mut it, "--emit")?;
                cfg.emit = match v.as_str() {
                    "json" => Emit::Json,
                    "patch" | "diff" => Emit::Patch,
                    other => return Err(format!("unknown --emit value: {other} (json|patch)")),
                };
            }
            "--exclude" => {
                let v = take_value(&mut it, "--exclude")?;
                cfg.excludes.extend(
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                );
            }
            "--budget-max-added-lines" => {
                cfg.budget.max_added_lines =
                    parse_usize(&take_value(&mut it, "--budget-max-added-lines")?)?;
            }
            "--budget-max-files" => {
                cfg.budget.max_files = parse_usize(&take_value(&mut it, "--budget-max-files")?)?;
            }
            "--budget-max-ratio" => {
                cfg.budget.max_ratio = parse_f64(&take_value(&mut it, "--budget-max-ratio")?)?;
            }
            "--rustfmt" => cfg.rustfmt = take_value(&mut it, "--rustfmt")?,
            "--engine-timeout-secs" => {
                cfg.engine_timeout_secs =
                    parse_usize(&take_value(&mut it, "--engine-timeout-secs")?)? as u64;
            }
            "--log" => cfg.log = Some(PathBuf::from(take_value(&mut it, "--log")?)),
            other => return Err(format!("unknown argument: {other} (see --help)")),
        }
    }
    Ok(cfg)
}

fn take_value<'a>(
    it: &mut std::iter::Peekable<std::slice::Iter<'a, String>>,
    flag: &str,
) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_usize(v: &str) -> Result<usize, String> {
    v.parse::<usize>()
        .map_err(|_| format!("invalid number: {v}"))
}

fn parse_f64(v: &str) -> Result<f64, String> {
    v.parse::<f64>().map_err(|_| format!("invalid number: {v}"))
}

fn run(cfg: &Config, cwd: &Path) -> Result<i32, String> {
    let run_id = events::new_run_id();
    let log = cfg.log.as_deref();
    let dry_run = !cfg.apply;

    if cfg.sandbox && !cfg.apply {
        return Err(
            "--sandbox requires --apply (it verifies the to-be-applied patch in an isolated worktree)"
                .to_string(),
        );
    }

    // ---- L1: scope -----------------------------------------------------
    let scope: Scope = if let Some(cs_path) = &cfg.changeset {
        let text = std::fs::read_to_string(cs_path)
            .map_err(|e| format!("cannot read changeset {}: {e}", cs_path.display()))?;
        let cs: ChangeSet = serde_json::from_str(&text)
            .map_err(|e| format!("invalid changeset {}: {e}", cs_path.display()))?;
        // exclude patterns apply to explicit changesets too
        let files: Vec<ScopedFile> = cs
            .files
            .into_iter()
            .filter(|f| !scope::is_excluded(&f.path, &cfg.excludes))
            .collect();
        Scope {
            vcs: None,
            base: cs.base_ref,
            source: "changeset".to_string(),
            files,
        }
    } else {
        let vcs = cfg.vcs.or_else(|| scope::detect_vcs(cwd));
        match vcs {
            Some(v) => {
                let mut s = scope::detect_scope(cwd, Some(v), &cfg.base, &cfg.excludes)
                    .map_err(|e| e.to_string())?;
                // VCS-detected files with no new-side ranges (pure deletions)
                // have nothing to format: drop them.
                s.files.retain(|f| !f.ranges.is_empty());
                s
            }
            None => return Err(scope::ScopeError::NotARepository.to_string()),
        }
    };

    events::append(
        log,
        &events::Event::RunStart {
            run_id: run_id.clone(),
            version: env!("CARGO_PKG_VERSION"),
            cwd: &cwd.display().to_string(),
            base: &scope.base,
            vcs: scope.vcs.map(|v| match v {
                Vcs::Git => "git",
                Vcs::Jj => "jj",
            }),
            source: &scope.source,
            rustfmt: &cfg.rustfmt,
            toolchain: &rustfmt_version(&cfg.rustfmt).unwrap_or_else(|| "unknown".to_string()),
            dry_run,
        },
    )
    .map_err(|e| format!("cannot write event log: {e}"))?;

    events::append(
        log,
        &events::Event::ScopeDetect {
            source: &scope.source,
            files: scope.files.iter().map(|f| f.path.clone()).collect(),
            excluded: Vec::new(),
        },
    )
    .map_err(|e| format!("cannot write event log: {e}"))?;

    if scope.files.is_empty() {
        let report = report::build_report(
            run_id.clone(),
            &scope,
            &[],
            &[],
            true,
            if dry_run { "dry-run" } else { "apply" },
        );
        emit_output(&report, cfg.emit);
        eprintln!("fmtguard: nothing to do (no scoped .rs files)");
        return Ok(0);
    }

    // ---- L2: engine ----------------------------------------------------
    let engine = engine::Engine {
        rustfmt_path: cfg.rustfmt.clone(),
        timeout_secs: cfg.engine_timeout_secs,
    };
    let config_path = engine::find_rustfmt_config(cwd);

    let mut results = Vec::new();
    let mut engine_errors = Vec::new();
    for file in &scope.files {
        let edition = engine::detect_edition(cwd, &file.path);
        events::append(
            log,
            &events::Event::EngineSelect {
                file: &file.path,
                engine: "rustfmt-diff-intersect",
                edition,
            },
        )
        .map_err(|e| format!("cannot write event log: {e}"))?;
        match engine::format_file(&engine, cwd, file, config_path.as_deref()) {
            Ok(r) => {
                events::append(
                    log,
                    &events::Event::FmtResult {
                        file: &r.path,
                        changed: r.changed,
                        idempotent: r.idempotent,
                        added_lines: r.added_lines,
                        removed_lines: r.removed_lines,
                        hunks_total: r.hunks_total,
                        hunks_kept: r.hunks_kept,
                        patch: r.patch.as_deref(),
                    },
                )
                .map_err(|e| format!("cannot write event log: {e}"))?;
                results.push(r);
            }
            Err(e) => {
                eprintln!("fmtguard: engine error: {e}");
                engine_errors.push(e.to_string());
            }
        }
    }

    if !engine_errors.is_empty() {
        return Err(format!(
            "{} file(s) could not be formatted; run aborted (fail-closed): {}",
            engine_errors.len(),
            engine_errors.join("; ")
        ));
    }

    // ---- L3: gates -----------------------------------------------------
    let (mut all_pass, mut gate_results) = gates::check(&scope, &results, &cfg.budget);

    // ---- L3b: sandbox verification (only when applying) -----------------
    if cfg.apply && cfg.sandbox && all_pass {
        let sandbox_gate = sandbox::verify(&scope, &results, cwd, &run_id)?;
        let pass = sandbox_gate.pass;
        gate_results.push(sandbox_gate);
        if !pass {
            all_pass = false;
        }
    }

    for g in &gate_results {
        events::append(
            log,
            &events::Event::GateCheck {
                gate: &g.gate,
                pass: g.pass,
                file: g.file.as_deref(),
                metric: g.metric,
                limit: g.limit,
                detail: Some(&g.detail),
            },
        )
        .map_err(|e| format!("cannot write event log: {e}"))?;
    }

    // ---- L4: apply (only when every gate passed) ------------------------
    let mut applied_files = 0usize;
    if cfg.apply && all_pass {
        for r in &results {
            if r.changed {
                if let Some(content) = &r.new_content {
                    let abs = cwd.join(&r.path);
                    std::fs::write(&abs, content)
                        .map_err(|e| format!("cannot write {}: {e}", abs.display()))?;
                    applied_files += 1;
                }
            }
        }
    }
    // if !all_pass: refuse — nothing is written (fail-closed)
    events::append(
        log,
        &events::Event::Apply {
            dry_run,
            applied_files,
            refused: cfg.apply && !all_pass,
        },
    )
    .map_err(|e| format!("cannot write event log: {e}"))?;

    let mode = if dry_run { "dry-run" } else { "apply" };
    let report = report::build_report(
        run_id.clone(),
        &scope,
        &results,
        &gate_results,
        all_pass,
        mode,
    );
    events::append(
        log,
        &events::Event::ReportEmit {
            verdict: &report.verdict,
            files_changed: report.stats.files_changed,
            total_added: report.stats.added_lines,
            total_removed: report.stats.removed_lines,
            outputs: vec![match cfg.emit {
                Emit::Json => "json",
                Emit::Patch => "patch",
            }],
        },
    )
    .map_err(|e| format!("cannot write event log: {e}"))?;

    emit_output(&report, cfg.emit);
    emit_human_summary(&report);

    if all_pass {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn rustfmt_version(rustfmt: &str) -> Option<String> {
    let out = std::process::Command::new(rustfmt)
        .arg("--version")
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn emit_output(report: &report::Report, emit: Emit) {
    match emit {
        Emit::Json => {
            let json = serde_json::to_string_pretty(report).expect("report serializes");
            println!("{json}");
        }
        Emit::Patch => {
            if let Some(patch) = &report.patch {
                print!("{patch}");
            }
        }
    }
}

fn emit_human_summary(report: &report::Report) {
    let v = report.verdict.as_str();
    eprintln!(
        "fmtguard {}: {v} — {} file(s) changed, +{} −{} (scanned {})",
        report.version,
        report.stats.files_changed,
        report.stats.added_lines,
        report.stats.removed_lines,
        report.stats.files_scanned
    );
    if !report.rejections.is_empty() {
        eprintln!("fmtguard: rejected by gates:");
        for r in &report.rejections {
            let file = r.file.as_deref().unwrap_or("");
            eprintln!(
                "  - {} [{file}]: {} (metric={:?} limit={:?})",
                r.gate, r.detail, r.metric, r.limit
            );
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Subcommand dispatch: `fmtguard replay <runId> ...`
    if args.first().map(String::as_str) == Some("replay") {
        return match run_replay(&args[1..]) {
            Ok(code) => ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("fmtguard: replay: {e}");
                ExitCode::from(2)
            }
        };
    }

    let cfg = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fmtguard: {e}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let cwd = std::env::current_dir().expect("current dir");
    match run(&cfg, &cwd) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("fmtguard: error: {e}");
            ExitCode::from(2)
        }
    }
}

/// `fmtguard replay <runId> [--emit json|patch] [--log <path>]`
fn run_replay(args: &[String]) -> Result<i32, String> {
    let mut run_id: Option<String> = None;
    let mut emit = Emit::Json;
    let mut log = PathBuf::from(".fmtguard/runs.jsonl");

    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--emit" => {
                let v = take_value(&mut it, "--emit")?;
                emit = match v.as_str() {
                    "json" => Emit::Json,
                    "patch" | "diff" => Emit::Patch,
                    other => return Err(format!("unknown --emit value: {other} (json|patch)")),
                };
            }
            "--log" => log = PathBuf::from(take_value(&mut it, "--log")?),
            s if s.starts_with("--") => return Err(format!("unknown replay option: {s}")),
            s => {
                if run_id.is_some() {
                    return Err(format!("unexpected extra argument: {s}"));
                }
                run_id = Some(s.to_string());
            }
        }
    }
    let run_id = run_id.ok_or_else(|| {
        "missing <runId> (find ids in the event log or --emit json output)".to_string()
    })?;

    let report = replay::replay(&run_id, &log).map_err(|e| e.to_string())?;
    emit_output(&report, emit);
    eprintln!(
        "fmtguard replay {run_id}: verdict {} — {} file(s) changed, +{} −{}",
        report.verdict,
        report.stats.files_changed,
        report.stats.added_lines,
        report.stats.removed_lines
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_roundtrip() {
        let args = vec![
            "--scope-from-git".to_string(),
            "--base".to_string(),
            "main".to_string(),
            "--emit".to_string(),
            "patch".to_string(),
            "--budget-max-added-lines".to_string(),
            "50".to_string(),
            "--exclude".to_string(),
            "foo/**,bar.rs".to_string(),
            "--no-log".to_string(),
        ];
        let cfg = parse_args(&args).unwrap();
        assert_eq!(cfg.vcs, Some(Vcs::Git));
        assert_eq!(cfg.base, "main");
        assert_eq!(cfg.emit, Emit::Patch);
        assert_eq!(cfg.budget.max_added_lines, 50);
        assert!(cfg.excludes.iter().any(|e| e == "foo/**"));
        assert!(cfg.excludes.iter().any(|e| e == "bar.rs"));
        assert!(cfg.log.is_none());
    }

    #[test]
    fn parse_args_rejects_unknown() {
        assert!(parse_args(&["--nope".to_string()]).is_err());
        assert!(parse_args(&["--emit".to_string(), "yaml".to_string()]).is_err());
    }
}
