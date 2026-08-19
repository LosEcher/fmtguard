//! L3 — Mechanical gates. Every invariant that can be checked mechanically is
//! a gate; a failing gate rejects the run (exit 1) and refuses to apply.

use crate::engine::FormatResult;
use crate::types::Scope;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GateResult {
    pub gate: String,
    pub pass: bool,
    pub file: Option<String>,
    pub metric: Option<f64>,
    pub limit: Option<f64>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct Budget {
    pub max_added_lines: usize,
    pub max_files: usize,
    pub max_ratio: f64,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_added_lines: 200,
            max_files: 5,
            max_ratio: 3.0,
        }
    }
}

fn gate_ok(gate: &str, detail: &str) -> GateResult {
    GateResult {
        gate: gate.to_string(),
        pass: true,
        file: None,
        metric: None,
        limit: None,
        detail: detail.to_string(),
    }
}

fn gate_fail(gate: &str, file: &str, metric: f64, limit: f64, detail: &str) -> GateResult {
    GateResult {
        gate: gate.to_string(),
        pass: false,
        file: Some(file.to_string()),
        metric: Some(metric),
        limit: Some(limit),
        detail: detail.to_string(),
    }
}

/// Run all gates over the results. Returns (all_pass, gate_results).
pub fn check(scope: &Scope, results: &[FormatResult], budget: &Budget) -> (bool, Vec<GateResult>) {
    let mut gates = Vec::new();

    // G0 — scope containment: files with edits must be within the scope.
    {
        let scoped: std::collections::HashSet<&str> =
            scope.files.iter().map(|f| f.path.as_str()).collect();
        let out_of_scope: Vec<&str> = results
            .iter()
            .filter(|r| r.changed)
            .map(|r| r.path.as_str())
            .filter(|p| !scoped.contains(p))
            .collect();
        if out_of_scope.is_empty() {
            gates.push(gate_ok(
                "scope.containment",
                "all edited files are within scope",
            ));
        } else {
            gates.push(gate_fail(
                "scope.containment",
                &out_of_scope.join(","),
                out_of_scope.len() as f64,
                0.0,
                "formatter touched files outside the scope",
            ));
        }
    }

    // G1a — per-file added lines cap.
    for r in results {
        if !r.changed {
            continue;
        }
        if r.added_lines <= budget.max_added_lines {
            gates.push(GateResult {
                gate: "budget.per_file_added".to_string(),
                pass: true,
                file: Some(r.path.clone()),
                metric: Some(r.added_lines as f64),
                limit: Some(budget.max_added_lines as f64),
                detail: format!(
                    "{} added lines (removed {}) after formatting",
                    r.added_lines, r.removed_lines
                ),
            });
        } else {
            gates.push(gate_fail(
                "budget.per_file_added",
                &r.path,
                r.added_lines as f64,
                budget.max_added_lines as f64,
                "formatter added too many lines",
            ));
        }
    }

    // G1b — diff ratio: formatter additions vs the caller's own additions.
    for f in &scope.files {
        if let Some(agent_added) = f.agent_added_lines {
            let result = results.iter().find(|r| r.path == f.path);
            let formatted_added = result.map(|r| r.added_lines).unwrap_or(0);
            if formatted_added == 0 {
                continue;
            }
            let ratio = formatted_added as f64 / agent_added.max(1) as f64;
            if ratio <= budget.max_ratio {
                gates.push(GateResult {
                    gate: "budget.diff_ratio".to_string(),
                    pass: true,
                    file: Some(f.path.clone()),
                    metric: Some(ratio),
                    limit: Some(budget.max_ratio),
                    detail: format!(
                        "formatter added {formatted_added} lines vs agent's {agent_added} lines"
                    ),
                });
            } else {
                gates.push(gate_fail(
                    "budget.diff_ratio",
                    &f.path,
                    ratio,
                    budget.max_ratio,
                    "formatter expanded the diff beyond the ratio budget",
                ));
            }
        }
    }

    // G1c — number of changed files cap.
    {
        let changed = results.iter().filter(|r| r.changed).count();
        if changed <= budget.max_files {
            gates.push(GateResult {
                gate: "budget.max_files".to_string(),
                pass: true,
                file: None,
                metric: Some(changed as f64),
                limit: Some(budget.max_files as f64),
                detail: format!("{changed} file(s) changed by formatting"),
            });
        } else {
            gates.push(gate_fail(
                "budget.max_files",
                "(aggregate)",
                changed as f64,
                budget.max_files as f64,
                "too many files changed by formatting",
            ));
        }
    }

    // G3 — engine idempotency: formatting the formatted output must be a
    // no-op; a formatter that keeps moving would fight the next run.
    for r in results {
        if r.changed && !r.idempotent {
            gates.push(gate_fail(
                "engine.idempotent",
                &r.path,
                1.0,
                0.0,
                "formatter is not idempotent: formatting the formatted output changed it again",
            ));
        } else if r.changed {
            gates.push(gate_ok(
                "engine.idempotent",
                &format!("{}: formatter reached a stable point", r.path),
            ));
        }
    }

    // G2 — whitespace hygiene on added lines (mirrors `git diff --check` for
    // the formatter's own additions: trailing whitespace, whitespace-only).
    for r in results {
        if !r.changed {
            continue;
        }
        let patch = r.patch.as_deref().unwrap_or("");
        let mut bad: Vec<String> = Vec::new();
        for line in patch.lines() {
            if let Some(content) = line.strip_prefix('+') {
                if content.is_empty() {
                    continue; // a bare "+" marks an added empty line; fine
                }
                let trailing = content.ends_with(' ') || content.ends_with('\t');
                let whitespace_only = content.trim().is_empty();
                if trailing || whitespace_only {
                    bad.push(line.to_string());
                }
            }
        }
        if bad.is_empty() {
            gates.push(gate_ok(
                "whitespace.clean",
                &format!("{}: no trailing whitespace", r.path),
            ));
        } else {
            gates.push(gate_fail(
                "whitespace.clean",
                &r.path,
                bad.len() as f64,
                0.0,
                "formatter produced trailing/whitespace-only added lines",
            ));
        }
    }

    let all_pass = gates.iter().all(|g| g.pass);
    (all_pass, gates)
}
