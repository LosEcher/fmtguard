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

/// Why a budget gate fired, in the caller's terms. The common cause is not a
/// runaway formatter but hunk merging across the scope boundary: changes within
/// `2*context` lines become one hunk, so a neighbouring pre-existing formatting
/// fix rides along with the caller's edit.
fn merge_hint(r: &FormatResult) -> String {
    if let Some(gap) = r.min_cross_gap {
        if gap <= 2 * r.hunk_context {
            return format!(
                " (nearest out-of-scope formatting hunk is {gap} line(s) away; changes within {} \
                 lines merge into one hunk — separate the edit from that hunk or pass \
                 --hunk-context {} to isolate it)",
                2 * r.hunk_context,
                gap / 2
            );
        }
        return format!(" (nearest out-of-scope formatting hunk is {gap} line(s) away)");
    }
    if r.scope_lines > 0 && r.kept_lines > r.scope_lines * 2 + 3 {
        return format!(
            " (the kept hunk spans {} line(s) although the scope declares {}: changes within {} \
             lines of each other merge into one hunk — pass --hunk-context 0 to isolate your edit, \
             or separate it from the neighbouring formatting debt)",
            r.kept_lines,
            r.scope_lines,
            2 * r.hunk_context
        );
    }
    String::new()
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
                &format!("formatter added too many lines{}", merge_hint(r)),
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
                let hint = result.map(merge_hint).unwrap_or_default();
                gates.push(gate_fail(
                    "budget.diff_ratio",
                    &f.path,
                    ratio,
                    budget.max_ratio,
                    &format!("formatter expanded the diff beyond the ratio budget{hint}"),
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
