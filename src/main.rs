//! fmtguard — scoped, gated Rust formatting for AI agents and incremental
//! workflows.
//!
//! Exit codes:
//!   0  ok (or nothing to do)
//!   1  rejected by a mechanical gate (scope/budget/whitespace) — no writes
//!   2  error (usage, VCS, engine, I/O)

mod configfile;
mod doctor;
mod engine;
mod events;
mod gates;
#[allow(dead_code)]
mod logctl;
mod lsp;
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
                              working copy before writing the main tree
                              (git: worktree + `git diff --check`; jj:
                              workspace + `cargo check`)
  --diff-timeout-secs N       budget for fmtguard's own diff/clip step
                              (default 10, 0 = unlimited). On expiry the run
                              fails closed (exit 2) rather than applying a diff
                              whose scope isolation could not be computed
  --hunk-context N            diff context for hunk grouping (default 3, 0-6).
                              Changes within 2*N lines merge into one hunk, so a
                              smaller N isolates your edit from neighbouring
                              formatting debt more precisely
  --include-untracked         also format untracked *.rs files (whole file);
                              without it they are reported as a warning, since
                              a VCS diff cannot see them at all
  --verify-fmt-check[=delta|strict]
                              post-format cleanliness gate. 'delta' (default)
                              rejects only NEW formatting debt inside the
                              scoped ranges and reports pre-existing debt;
                              'strict' requires the whole file to be a rustfmt
                              fixed point (rejects runs in a repo that still
                              carries untouched formatting debt)
  --exclude <glob,...>        extra exclusion globs (defaults: generated/**,
                              vendor/**, target/**, node_modules/**)
  --budget-max-added-lines N  per-file formatter added-line cap (default 200)
  --budget-max-files N        max files changed by formatting (default 5)
  --budget-max-ratio R        formatter/agent added-line ratio cap (default 3.0)
  --rustfmt <path>            rustfmt binary (default: from PATH)
  --engine <e3|e1>            formatting engine (default: e3; e1 uses rust-analyzer)
  --engine-timeout-secs N     rustfmt timeout (default 30)
  --log <path>                event log (default: .fmtguard/runs.jsonl)
  --no-log                    disable the event log
  --no-config                 ignore ~/.config/fmtguard/config.toml and
                              ./.fmtguard.toml
  --dump-config               print the effective config with the source of
                              each key, then exit 0
  --require-version X.Y.Z     fail closed (exit 2) unless this binary is at
                              least X.Y.Z; use it in AGENTS.md to catch a
                              stale install before it formats anything
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
  doctor [--emit json] [--require-version X.Y.Z] [--rustfmt <path>]
         [--engine-timeout-secs N] [--log <path>]
                        read-only environment probe: formatter version and
                        behaviour, engine availability, config/edition
                        discovery, event-log health. exit 2 when the
                        environment cannot do the job.
  log prune (--keep-runs N | --older-than 7d) [--log <path>] [--apply]
            [--emit json]
                        retention for the event log: dropped runs are appended
                        to .fmtguard/archive/ (never deleted) and the live log
                        is rewritten atomically. dry-run unless --apply.
";

#[derive(Debug)]
struct Config {
    vcs: Option<Vcs>,
    base: String,
    changeset: Option<PathBuf>,
    emit: Emit,
    apply: bool,
    sandbox: bool,
    verify_fmt_check: Option<engine::FmtCheckMode>,
    include_untracked: bool,
    hunk_context: usize,
    diff_timeout_secs: u64,
    excludes: Vec<String>,
    budget: gates::Budget,
    rustfmt: String,
    engine: String,
    engine_timeout_secs: u64,
    log: Option<PathBuf>,
    require_version: Option<String>,
    /// Which source each policy key came from (`default`, `user:<path>`,
    /// `repo:<path>`, `cli`) — reported by `--dump-config`.
    sources: std::collections::BTreeMap<String, String>,
    /// Keys the command line actually set (config files must not override them).
    cli_keys: Vec<String>,
    no_config: bool,
    dump_config: bool,
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
            verify_fmt_check: None,
            include_untracked: false,
            hunk_context: 3,
            diff_timeout_secs: 10,
            excludes: scope::DEFAULT_EXCLUDES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            budget: gates::Budget::default(),
            rustfmt: "rustfmt".to_string(),
            engine: "e3".to_string(),
            engine_timeout_secs: 30,
            log: Some(PathBuf::from(".fmtguard/runs.jsonl")),
            require_version: None,
            sources: {
                let mut m = std::collections::BTreeMap::new();
                for k in configfile::KNOWN_KEYS {
                    m.insert((*k).to_string(), "default".to_string());
                }
                m
            },
            cli_keys: Vec::new(),
            no_config: false,
            dump_config: false,
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
            "--no-config" => cfg.no_config = true,
            "--dump-config" => cfg.dump_config = true,
            "--verify-fmt-check" | "--verify-fmt-check=delta" => {
                cfg.verify_fmt_check = Some(engine::FmtCheckMode::Delta);
                cfg.cli_keys.push("verify_fmt_check".to_string());
            }
            "--verify-fmt-check=strict" => {
                cfg.verify_fmt_check = Some(engine::FmtCheckMode::Strict);
                cfg.cli_keys.push("verify_fmt_check".to_string());
            }
            "--verify-fmt-check=off" => {
                cfg.verify_fmt_check = None;
                cfg.cli_keys.push("verify_fmt_check".to_string());
            }
            "--include-untracked" => {
                cfg.include_untracked = true;
                cfg.cli_keys.push("include_untracked".to_string());
            }
            "--diff-timeout-secs" => {
                cfg.cli_keys.push("diff_timeout_secs".to_string());
                cfg.diff_timeout_secs =
                    parse_usize(&take_value(&mut it, "--diff-timeout-secs")?)? as u64;
            }
            "--hunk-context" => {
                cfg.cli_keys.push("hunk_context".to_string());
                let n = parse_usize(&take_value(&mut it, "--hunk-context")?)?;
                if n > 6 {
                    return Err(format!(
                        "--hunk-context {n} is too large to be useful (0-6); a bigger context only \
                         merges more foreign hunks into your change"
                    ));
                }
                cfg.hunk_context = n;
            }
            "--no-log" => {
                cfg.log = None;
                cfg.cli_keys.push("log".to_string());
            }
            "--base" => {
                cfg.base = take_value(&mut it, "--base")?;
                cfg.cli_keys.push("base".to_string());
            }
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
                cfg.cli_keys.push("exclude".to_string());
                cfg.excludes.extend(
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                );
            }
            "--budget-max-added-lines" => {
                cfg.cli_keys.push("budget.max_added_lines".to_string());
                cfg.budget.max_added_lines =
                    parse_usize(&take_value(&mut it, "--budget-max-added-lines")?)?;
            }
            "--budget-max-files" => {
                cfg.cli_keys.push("budget.max_files".to_string());
                cfg.budget.max_files = parse_usize(&take_value(&mut it, "--budget-max-files")?)?;
            }
            "--budget-max-ratio" => {
                cfg.cli_keys.push("budget.max_ratio".to_string());
                cfg.budget.max_ratio = parse_f64(&take_value(&mut it, "--budget-max-ratio")?)?;
            }
            "--rustfmt" => {
                cfg.rustfmt = take_value(&mut it, "--rustfmt")?;
                cfg.cli_keys.push("rustfmt".to_string());
            }
            "--engine" => {
                cfg.cli_keys.push("engine".to_string());
                cfg.engine = take_value(&mut it, "--engine")?;
                if cfg.engine != "e1" && cfg.engine != "e3" {
                    return Err("unknown --engine value (e1|e3)".to_string());
                }
            }
            "--engine-timeout-secs" => {
                cfg.cli_keys.push("engine_timeout_secs".to_string());
                cfg.engine_timeout_secs =
                    parse_usize(&take_value(&mut it, "--engine-timeout-secs")?)? as u64;
            }
            "--log" => {
                cfg.log = Some(PathBuf::from(take_value(&mut it, "--log")?));
                cfg.cli_keys.push("log".to_string());
            }
            "--require-version" => {
                let v = take_value(&mut it, "--require-version")?;
                doctor::parse_version(&v)?; // reject nonsense at parse time
                cfg.require_version = Some(v);
            }
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

fn required_str<'a>(v: &'a configfile::Value, key: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("config key `{key}` must be a quoted string"))
}
fn required_int(v: &configfile::Value, key: &str) -> Result<i64, String> {
    v.as_int()
        .ok_or_else(|| format!("config key `{key}` must be an integer"))
}
fn required_float(v: &configfile::Value, key: &str) -> Result<f64, String> {
    v.as_float()
        .ok_or_else(|| format!("config key `{key}` must be a number"))
}
fn required_bool(v: &configfile::Value, key: &str) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("config key `{key}` must be true or false"))
}
fn required_array(v: &configfile::Value, key: &str) -> Result<Vec<String>, String> {
    v.as_array()
        .map(|a| a.to_vec())
        .ok_or_else(|| format!("config key `{key}` must be an array of quoted strings"))
}

/// Apply one config file on top of the current values.
fn apply_file_config(cfg: &mut Config, file: &configfile::FileConfig) -> Result<(), String> {
    let label = format!("file:{}", file.path.display());
    for (key, value) in &file.values {
        match key.as_str() {
            "rustfmt" => cfg.rustfmt = required_str(value, key)?.to_string(),
            "engine" => {
                let e = required_str(value, key)?;
                if e != "e1" && e != "e3" {
                    return Err(format!("{}: engine must be e1 or e3", file.path.display()));
                }
                cfg.engine = e.to_string();
            }
            "engine_timeout_secs" => {
                let n = required_int(value, key)?;
                if n <= 0 {
                    return Err(format!(
                        "{}: engine_timeout_secs must be positive",
                        file.path.display()
                    ));
                }
                cfg.engine_timeout_secs = n as u64;
            }
            "hunk_context" => {
                let n = required_int(value, key)?;
                if !(0..=6).contains(&n) {
                    return Err(format!(
                        "{}: hunk_context must be between 0 and 6",
                        file.path.display()
                    ));
                }
                cfg.hunk_context = n as usize;
            }
            "include_untracked" => cfg.include_untracked = required_bool(value, key)?,
            "diff_timeout_secs" => cfg.diff_timeout_secs = required_int(value, key)?.max(0) as u64,
            "verify_fmt_check" => {
                cfg.verify_fmt_check = match required_str(value, key)? {
                    "off" | "none" => None,
                    "delta" => Some(engine::FmtCheckMode::Delta),
                    "strict" => Some(engine::FmtCheckMode::Strict),
                    other => {
                        return Err(format!(
                            "{}: verify_fmt_check must be off|delta|strict, got `{other}`",
                            file.path.display()
                        ))
                    }
                };
            }
            "log" => cfg.log = Some(PathBuf::from(required_str(value, key)?)),
            "base" => cfg.base = required_str(value, key)?.to_string(),
            "exclude" => cfg.excludes.extend(required_array(value, key)?),
            "budget.max_added_lines" => {
                cfg.budget.max_added_lines = required_int(value, key)?.max(0) as usize
            }
            "budget.max_files" => cfg.budget.max_files = required_int(value, key)?.max(0) as usize,
            "budget.max_ratio" => cfg.budget.max_ratio = required_float(value, key)?,
            other => {
                return Err(format!(
                    "{}: unknown config key `{other}`",
                    file.path.display()
                ))
            }
        }
        cfg.sources.insert(key.clone(), label.clone());
    }
    Ok(())
}

/// Overlay the policy keys the command line actually set.
fn apply_cli_config(base: &mut Config, cli: &Config) {
    for key in &cli.cli_keys {
        match key.as_str() {
            "rustfmt" => base.rustfmt = cli.rustfmt.clone(),
            "engine" => base.engine = cli.engine.clone(),
            "engine_timeout_secs" => base.engine_timeout_secs = cli.engine_timeout_secs,
            "hunk_context" => base.hunk_context = cli.hunk_context,
            "include_untracked" => base.include_untracked = cli.include_untracked,
            "diff_timeout_secs" => base.diff_timeout_secs = cli.diff_timeout_secs,
            "verify_fmt_check" => base.verify_fmt_check = cli.verify_fmt_check,
            "log" => base.log = cli.log.clone(),
            "base" => base.base = cli.base.clone(),
            "exclude" => base.excludes = cli.excludes.clone(),
            "budget.max_added_lines" => base.budget.max_added_lines = cli.budget.max_added_lines,
            "budget.max_files" => base.budget.max_files = cli.budget.max_files,
            "budget.max_ratio" => base.budget.max_ratio = cli.budget.max_ratio,
            _ => {}
        }
        base.sources.insert(key.clone(), "cli".to_string());
    }
}

/// Effective config: defaults → user file → repo file → command line.
fn resolve_config(
    cli: Config,
    cwd: &Path,
) -> Result<(Config, Option<PathBuf>, Option<PathBuf>), String> {
    let mut cfg = Config::default();
    let mut user_used = None;
    let mut repo_used = None;
    if !cli.no_config {
        if let Some(p) = configfile::user_config_path() {
            if let Some(f) = configfile::load(&p)? {
                apply_file_config(&mut cfg, &f)?;
                user_used = Some(p);
            }
        }
        let p = configfile::repo_config_path(cwd);
        if let Some(f) = configfile::load(&p)? {
            apply_file_config(&mut cfg, &f)?;
            repo_used = Some(p);
        }
    }
    // per-invocation switches never come from a file
    cfg.vcs = cli.vcs;
    cfg.changeset = cli.changeset.clone();
    cfg.emit = cli.emit;
    cfg.apply = cli.apply;
    cfg.sandbox = cli.sandbox;
    cfg.require_version = cli.require_version.clone();
    cfg.no_config = cli.no_config;
    cfg.dump_config = cli.dump_config;
    apply_cli_config(&mut cfg, &cli);
    Ok((cfg, user_used, repo_used))
}

fn dump_config(cfg: &Config, cwd: &Path, user: &Option<PathBuf>, repo: &Option<PathBuf>) {
    let effective = serde_json::json!({
        "rustfmt": cfg.rustfmt,
        "engine": cfg.engine,
        "engine_timeout_secs": cfg.engine_timeout_secs,
        "hunk_context": cfg.hunk_context,
        "diff_timeout_secs": cfg.diff_timeout_secs,
        "include_untracked": cfg.include_untracked,
        "verify_fmt_check": match cfg.verify_fmt_check {
            None => "off",
            Some(engine::FmtCheckMode::Delta) => "delta",
            Some(engine::FmtCheckMode::Strict) => "strict",
        },
        "log": cfg.log.as_ref().map(|p| p.display().to_string()),
        "base": cfg.base,
        "exclude": cfg.excludes,
        "budget": {
            "max_added_lines": cfg.budget.max_added_lines,
            "max_files": cfg.budget.max_files,
            "max_ratio": cfg.budget.max_ratio,
        },
    });
    let out = serde_json::json!({
        "tool": "fmtguard",
        "version": env!("CARGO_PKG_VERSION"),
        "cwd": cwd.display().to_string(),
        "config_files": {
            "user": user.as_ref().map(|p| p.display().to_string()),
            "repo": repo.as_ref().map(|p| p.display().to_string()),
            "disabled": cfg.no_config,
        },
        "effective": effective,
        "sources": cfg.sources,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&out).expect("dump serializes")
    );
}

fn run(cfg: &Config, cwd: &Path) -> Result<i32, String> {
    if let Some(req) = &cfg.require_version {
        if !doctor::version_satisfies(env!("CARGO_PKG_VERSION"), req)? {
            return Err(format!(
                "version requirement unmet: running {} but >= {} is required (upgrade the binary)",
                env!("CARGO_PKG_VERSION"),
                req
            ));
        }
    }
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
            untracked: Vec::new(),
        }
    } else {
        let vcs = cfg.vcs.or_else(|| scope::detect_vcs(cwd));
        match vcs {
            Some(v) => {
                let mut s = scope::detect_scope(
                    cwd,
                    Some(v),
                    &cfg.base,
                    &cfg.excludes,
                    cfg.include_untracked,
                )
                .map_err(|e| e.to_string())?;
                // VCS-detected files with no new-side ranges (pure deletions)
                // have nothing to format: drop them — but keep the files that
                // --include-untracked deliberately added whole.
                s.files.retain(|f| !f.ranges.is_empty() || f.untracked);
                if !s.untracked.is_empty() && !cfg.include_untracked {
                    eprintln!(
                        "fmtguard: warning: {} untracked .rs file(s) are NOT in the diff scope \
                         (git add them, declare a --changeset, or pass --include-untracked): {}",
                        s.untracked.len(),
                        s.untracked.join(", ")
                    );
                }
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
            untracked: scope.untracked.clone(),
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
        hunk_context: cfg.hunk_context,
        diff_timeout_secs: cfg.diff_timeout_secs,
    };
    let config_path = engine::find_rustfmt_config(cwd);

    // Any exit-2 path from here on leaves evidence in the log: a bare
    // dangling `run_start` told an auditor nothing about *why* a run died
    // (2026-10-07 cross-project sweep).
    let abort = |kind: &str, message: String, files_failed: usize| -> Result<i32, String> {
        let _ = events::append(
            log,
            &events::Event::RunError {
                kind,
                message: message.clone(),
                files_failed,
            },
        );
        let _ = events::append(
            log,
            &events::Event::ReportEmit {
                verdict: "error",
                files_changed: 0,
                total_added: 0,
                total_removed: 0,
                outputs: vec![],
            },
        );
        Err(message)
    };

    let mut results = Vec::new();
    let mut engine_errors = Vec::new();
    for file in &scope.files {
        let edition = engine::detect_edition(cwd, &file.path);
        events::append(
            log,
            &events::Event::EngineSelect {
                file: &file.path,
                engine: if cfg.engine == "e1" {
                    "rust-analyzer-range"
                } else {
                    "rustfmt-diff-intersect"
                },
                edition,
            },
        )
        .map_err(|e| format!("cannot write event log: {e}"))?;
        let formatted = if cfg.engine == "e1" {
            engine::format_file_e1(cwd, file, "rust-analyzer", cfg.engine_timeout_secs)
        } else {
            engine::format_file(&engine, cwd, file, config_path.as_deref())
        };
        match formatted {
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
                        out_of_scope_hunks: r.hunks_total.saturating_sub(r.hunks_kept),
                        rustfmt_duration_ms: r.rustfmt_duration_ms,
                        rustfmt_first_pass_ms: r.rustfmt_first_pass_ms,
                        rustfmt_idempotency_pass_ms: r.rustfmt_idempotency_pass_ms,
                        clip_ms: r.clip_ms,
                        min_cross_gap: r.min_cross_gap,
                        kept_lines: r.kept_lines,
                        scope_lines: r.scope_lines,
                        hunk_context: r.hunk_context,
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
        return abort(
            "engine",
            format!(
                "{} file(s) could not be formatted; run aborted (fail-closed): {}",
                engine_errors.len(),
                engine_errors.join("; ")
            ),
            engine_errors.len(),
        );
    }

    // ---- L3: gates -----------------------------------------------------
    let (mut all_pass, mut gate_results) = gates::check(&scope, &results, &cfg.budget);

    if let Some(mode) = cfg.verify_fmt_check {
        for result in results.iter_mut() {
            if !result.changed {
                continue;
            }
            let check = match engine::verify_fmt_check(
                &engine,
                cwd,
                result,
                config_path.as_deref(),
                mode,
            ) {
                Ok(Some(c)) => c,
                Ok(None) => continue,
                Err(e) => return abort("engine", format!("fmt-check failed: {e}"), 1),
            };
            result.fmt_clean = Some(check.fmt_clean);
            result.in_scope_debt_hunks = Some(check.in_scope_debt_hunks);
            let pass = check.pass();
            events::append(
                log,
                &events::Event::FmtCheck {
                    file: &result.path,
                    mode: match mode {
                        engine::FmtCheckMode::Delta => "delta",
                        engine::FmtCheckMode::Strict => "strict",
                    },
                    fmt_clean: check.fmt_clean,
                    in_scope_debt_hunks: check.in_scope_debt_hunks,
                    out_of_scope_debt_hunks: check.out_of_scope_debt_hunks,
                },
            )
            .map_err(|e| format!("cannot write event log: {e}"))?;
            gate_results.push(gates::GateResult {
                gate: "engine.fmt_check".to_string(),
                pass,
                file: Some(result.path.clone()),
                metric: Some(check.in_scope_debt_hunks as f64),
                limit: Some(0.0),
                detail: check.detail(),
            });
            if !pass {
                all_pass = false;
            }
        }
    }

    // ---- L3b: sandbox verification (only when applying) -----------------
    if cfg.apply && cfg.sandbox && all_pass {
        let sandbox_gate = match sandbox::verify(&scope, &results, cwd, &run_id) {
            Ok(g) => g,
            Err(e) => return abort("sandbox", e, 0),
        };
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
                    if let Err(e) = std::fs::write(&abs, content) {
                        return abort("io", format!("cannot write {}: {e}", abs.display()), 1);
                    }
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
    if report.stats.out_of_scope_hunks > 0 {
        let files = report
            .files
            .iter()
            .filter(|f| f.out_of_scope_hunks > 0)
            .count();
        eprintln!(
            "fmtguard: scope-clipped formatter: {} hunk(s) dropped in {} file(s) — \
             this verdict does NOT assert repo cleanliness (use `cargo fmt --all --check` for that)",
            report.stats.out_of_scope_hunks, files
        );
    }
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
    if args.first().map(String::as_str) == Some("log") {
        return match run_log(&args[1..]) {
            Ok(code) => ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("fmtguard: log: {e}");
                ExitCode::from(2)
            }
        };
    }
    if args.first().map(String::as_str) == Some("doctor") {
        return match run_doctor(&args[1..]) {
            Ok(code) => ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("fmtguard: doctor: {e}");
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
    let (cfg, user_cfg, repo_cfg) = match resolve_config(cfg, &cwd) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fmtguard: config: {e}");
            return ExitCode::from(2);
        }
    };
    if cfg.dump_config {
        dump_config(&cfg, &cwd, &user_cfg, &repo_cfg);
        return ExitCode::from(0);
    }
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

/// `fmtguard log prune (--keep-runs N | --older-than DUR) [--log <path>]
/// [--apply] [--emit json]`
fn run_log(args: &[String]) -> Result<i32, String> {
    let Some(action) = args.first().map(String::as_str) else {
        return Err("missing action (prune)".to_string());
    };
    if action != "prune" {
        return Err(format!("unknown log action: {action} (prune)"));
    }

    let mut keep_runs: Option<usize> = None;
    let mut older_than: Option<std::time::Duration> = None;
    let mut apply = false;
    let mut emit = Emit::Json;
    let mut log = doctor::default_doctor_log();

    let mut it = args[1..].iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--keep-runs" => keep_runs = Some(parse_usize(&take_value(&mut it, "--keep-runs")?)?),
            "--older-than" => {
                older_than = Some(logctl::parse_duration(&take_value(
                    &mut it,
                    "--older-than",
                )?)?)
            }
            "--apply" => apply = true,
            "--log" => log = PathBuf::from(take_value(&mut it, "--log")?),
            "--emit" => {
                let v = take_value(&mut it, "--emit")?;
                emit = match v.as_str() {
                    "json" => Emit::Json,
                    other => return Err(format!("unknown --emit value: {other} (json)")),
                };
            }
            s if s.starts_with("--") => return Err(format!("unknown log option: {s}")),
            s => return Err(format!("unexpected argument: {s}")),
        }
    }

    let plan = logctl::prune(&log, keep_runs, older_than, apply)?;
    match emit {
        Emit::Json => println!(
            "{}",
            serde_json::to_string_pretty(&plan).expect("prune plan serializes")
        ),
        Emit::Patch => unreachable!("--emit patch is rejected above"),
    }
    eprintln!(
        "fmtguard log prune: {} — {} run(s) total, {} kept, {} pruned{}",
        plan.mode,
        plan.total_runs,
        plan.kept_runs,
        plan.pruned_runs.len(),
        plan.archive
            .as_deref()
            .map(|a| format!(" (archived to {a})"))
            .unwrap_or_default()
    );
    Ok(0)
}

/// `fmtguard doctor [--emit json] [--require-version X.Y.Z] [--rustfmt <path>]
/// [--engine-timeout-secs N] [--log <path>]`
fn run_doctor(args: &[String]) -> Result<i32, String> {
    let mut emit = Emit::Json;
    let mut rustfmt = "rustfmt".to_string();
    let mut timeout_secs = 30u64;
    let mut log = doctor::default_doctor_log();
    let mut require_version: Option<String> = None;

    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--emit" => {
                let v = take_value(&mut it, "--emit")?;
                emit = match v.as_str() {
                    "json" => Emit::Json,
                    "patch" | "diff" => {
                        return Err("doctor has no patch output; use --emit json".to_string())
                    }
                    other => return Err(format!("unknown --emit value: {other} (json)")),
                };
            }
            "--rustfmt" => rustfmt = take_value(&mut it, "--rustfmt")?,
            "--engine-timeout-secs" => {
                timeout_secs = parse_usize(&take_value(&mut it, "--engine-timeout-secs")?)? as u64
            }
            "--log" => log = PathBuf::from(take_value(&mut it, "--log")?),
            "--require-version" => {
                let v = take_value(&mut it, "--require-version")?;
                doctor::parse_version(&v)?;
                require_version = Some(v);
            }
            s if s.starts_with("--") => return Err(format!("unknown doctor option: {s}")),
            s => return Err(format!("unexpected argument: {s}")),
        }
    }

    let cwd = std::env::current_dir().expect("current dir");
    let report = doctor::diagnose(
        &cwd,
        &rustfmt,
        timeout_secs,
        &log,
        require_version.as_deref(),
    )?;

    match emit {
        Emit::Json => {
            let json = serde_json::to_string_pretty(&report).expect("doctor report serializes");
            println!("{json}");
        }
        Emit::Patch => unreachable!("--emit patch is rejected above"),
    }

    eprintln!(
        "fmtguard doctor: {} — fmtguard {} · rustfmt {} · {} problem(s)",
        report.verdict,
        report.version,
        report
            .rustfmt
            .version
            .clone()
            .unwrap_or_else(|| "not found".to_string()),
        report.problems.len()
    );
    for p in &report.problems {
        eprintln!("  - {p}");
    }

    Ok(if report.problems.is_empty() { 0 } else { 2 })
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
