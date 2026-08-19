//! `--sandbox` — verify the to-be-applied patch in an isolated git worktree
//! before touching the main working tree.
//!
//! Flow: `git worktree add --detach <tmp> <base>` → copy the caller's edited
//! files into the worktree → apply the fmtguard patch there → run
//! `git diff --check` inside the worktree → only if that passes, write the
//! main tree. The worktree is removed on every exit path (a leftover is a
//! bug). jj is explicitly unsupported: jj worktrees are change-bound and need
//! separate design (fail-closed, no silent fallback).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::engine::FormatResult;
use crate::gates::GateResult;
use crate::types::{Scope, Vcs};

/// Run the sandbox verification. Returns a gate result (rejected on failure);
/// Err only for environment-level failures (worktree cannot be created).
pub fn verify(
    scope: &Scope,
    results: &[FormatResult],
    cwd: &Path,
    run_id: &str,
) -> Result<GateResult, String> {
    if scope.vcs != Some(Vcs::Git) {
        return Err(
            "--sandbox currently requires a git repository (jj worktrees are \
             change-bound and not yet supported)"
                .to_string(),
        );
    }

    let tmp: PathBuf = std::env::temp_dir().join(format!("fmtguard-sandbox-{run_id}"));
    let _ = std::fs::remove_dir_all(&tmp);

    let add = Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(&tmp)
        .arg(&scope.base)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git worktree add failed to spawn: {e}"))?;
    if !add.status.success() {
        return Err(format!(
            "git worktree add {} failed: {}",
            tmp.display(),
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }

    let cleanup = || {
        let _ = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&tmp)
            .current_dir(cwd)
            .output();
        let _ = std::fs::remove_dir_all(&tmp);
    };

    let verdict = (|| -> Result<GateResult, String> {
        // 1. sync the caller's edited files (agent content) into the worktree
        for r in results {
            if !r.changed {
                continue;
            }
            let dst = tmp.join(&r.path);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("sandbox: cannot create {}: {e}", parent.display()))?;
            }
            std::fs::copy(cwd.join(&r.path), &dst)
                .map_err(|e| format!("sandbox: cannot copy {}: {e}", r.path))?;
            // 2. apply the formatter patch inside the worktree
            if let Some(content) = &r.new_content {
                std::fs::write(&dst, content)
                    .map_err(|e| format!("sandbox: cannot write {}: {e}", r.path))?;
            }
        }

        // 3. verify in isolation
        let check = Command::new("git")
            .arg("diff")
            .arg("--check")
            .current_dir(&tmp)
            .output()
            .map_err(|e| format!("sandbox: git diff --check failed to spawn: {e}"))?;
        if check.status.success() {
            Ok(GateResult {
                gate: "sandbox.verify".to_string(),
                pass: true,
                file: None,
                metric: None,
                limit: None,
                detail: format!(
                    "sandbox worktree verified: {} file(s) applied, git diff --check clean",
                    results.iter().filter(|r| r.changed).count()
                ),
            })
        } else {
            Ok(GateResult {
                gate: "sandbox.verify".to_string(),
                pass: false,
                file: None,
                metric: None,
                limit: None,
                detail: format!(
                    "sandbox verification failed: git diff --check reported problems:\n{}",
                    String::from_utf8_lossy(&check.stderr).trim()
                ),
            })
        }
    })();

    cleanup();
    verdict
}
