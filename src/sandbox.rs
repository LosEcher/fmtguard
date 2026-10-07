//! `--sandbox` — verify the to-be-applied patch in an isolated working copy
//! before touching the main working tree.
//!
//! * git: `git worktree add --detach <tmp> <base>` → copy the caller's edited
//!   files in → apply the fmtguard patch there → `git diff --check` → only if
//!   that passes, write the main tree.
//! * jj: `jj workspace add <tmp> --name <name>` (the new workspace's
//!   working-copy commit sits on the current commit's parent, i.e. the
//!   pre-change state) → copy the caller's edited files in → apply the patch →
//!   `cargo check`. Verification uses `cargo check` only, because `jj diff` has
//!   no `--check` equivalent; fmtguard's own `whitespace.clean` gate already
//!   covers trailing whitespace on the added lines.
//!
//! The isolated tree is removed on every exit path, and on jj the orphan
//! working-copy change is abandoned afterwards (a leftover is a bug).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::engine::FormatResult;
use crate::gates::GateResult;
use crate::types::{Scope, Vcs};

/// Run the sandbox verification. Returns a gate result (rejected on failure);
/// Err only for environment-level failures (the isolated tree cannot be
/// created or cleaned up).
pub fn verify(
    scope: &Scope,
    results: &[FormatResult],
    cwd: &Path,
    run_id: &str,
) -> Result<GateResult, String> {
    match scope.vcs {
        Some(Vcs::Git) => verify_git(scope, results, cwd, run_id),
        Some(Vcs::Jj) => verify_jj(scope, results, cwd, run_id),
        None => Err(
            "--sandbox needs a git or jj repository: an explicit --changeset scope has no VCS \
             to isolate from"
                .to_string(),
        ),
    }
}

/// Copy the caller's edited files (with the formatter patch applied) into an
/// isolated tree. Shared by both backends.
fn sync_files(results: &[FormatResult], src: &Path, dst_root: &Path) -> Result<usize, String> {
    let mut copied = 0usize;
    for r in results {
        if !r.changed {
            continue;
        }
        let dst = dst_root.join(&r.path);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("sandbox: cannot create {}: {e}", parent.display()))?;
        }
        std::fs::copy(src.join(&r.path), &dst)
            .map_err(|e| format!("sandbox: cannot copy {}: {e}", r.path))?;
        if let Some(content) = &r.new_content {
            std::fs::write(&dst, content)
                .map_err(|e| format!("sandbox: cannot write {}: {e}", r.path))?;
        }
        copied += 1;
    }
    Ok(copied)
}

/// `cargo check --quiet` in the isolated tree; `Ok(None)` = passed.
fn cargo_check(dir: &Path) -> Result<Option<String>, String> {
    let cargo = Command::new("cargo")
        .arg("check")
        .arg("--quiet")
        .current_dir(dir)
        .output()
        .map_err(|e| format!("sandbox: cargo check failed to spawn: {e}"))?;
    if cargo.status.success() {
        Ok(None)
    } else {
        Ok(Some(
            String::from_utf8_lossy(&cargo.stderr).trim().to_string(),
        ))
    }
}

fn verify_git(
    scope: &Scope,
    results: &[FormatResult],
    cwd: &Path,
    run_id: &str,
) -> Result<GateResult, String> {
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
        // 1+2. sync the caller's edited files with the patch applied
        let copied = sync_files(results, cwd, &tmp)?;

        // 3. verify in isolation
        let check = Command::new("git")
            .arg("diff")
            .arg("--check")
            .current_dir(&tmp)
            .output()
            .map_err(|e| format!("sandbox: git diff --check failed to spawn: {e}"))?;
        if !check.status.success() {
            return Ok(GateResult {
                gate: "sandbox.verify".to_string(),
                pass: false,
                file: None,
                metric: None,
                limit: None,
                detail: format!(
                    "sandbox verification failed: git diff --check reported problems:\n{}",
                    String::from_utf8_lossy(&check.stderr).trim()
                ),
            });
        }

        match cargo_check(&tmp)? {
            None => Ok(GateResult {
                gate: "sandbox.verify".to_string(),
                pass: true,
                file: None,
                metric: None,
                limit: None,
                detail: format!(
                    "sandbox worktree verified: {copied} file(s) applied, git diff --check clean, \
                     cargo check passed"
                ),
            }),
            Some(err) => Ok(GateResult {
                gate: "sandbox.cargo_check".to_string(),
                pass: false,
                file: None,
                metric: None,
                limit: None,
                detail: format!("sandbox cargo check failed:\n{err}"),
            }),
        }
    })();

    cleanup();
    verdict
}

/// jj backend: an isolated workspace on top of the current commit's parent.
/// `jj diff` has no `--check`, so isolation proves compilation, not whitespace
/// (the `whitespace.clean` gate still covers the added lines).
fn verify_jj(
    _scope: &Scope,
    results: &[FormatResult],
    cwd: &Path,
    run_id: &str,
) -> Result<GateResult, String> {
    let name = format!("fmtguard-sandbox-{run_id}");
    let tmp: PathBuf = std::env::temp_dir().join(format!("fmtguard-sandbox-{run_id}"));
    let _ = std::fs::remove_dir_all(&tmp);

    let add = Command::new("jj")
        .arg("workspace")
        .arg("add")
        .arg(&tmp)
        .arg("--name")
        .arg(&name)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("jj workspace add failed to spawn: {e}"))?;
    if !add.status.success() {
        return Err(format!(
            "jj workspace add {} failed: {}",
            tmp.display(),
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }

    // Anchor for cleanup: the workspace's own working-copy change id. Collected
    // before `workspace forget` because that is what unbinds it.
    let change_id = Command::new("jj")
        .arg("log")
        .arg("--no-graph")
        .arg("-r")
        .arg(format!("{name}@"))
        .arg("-T")
        .arg("change_id")
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    let cleanup = || {
        let _ = Command::new("jj")
            .arg("workspace")
            .arg("forget")
            .arg(&name)
            .current_dir(cwd)
            .output();
        if let Some(id) = &change_id {
            let _ = Command::new("jj")
                .arg("abandon")
                .arg(id)
                .current_dir(cwd)
                .output();
        }
        let _ = std::fs::remove_dir_all(&tmp);
    };

    let verdict = (|| -> Result<GateResult, String> {
        let copied = sync_files(results, cwd, &tmp)?;
        match cargo_check(&tmp)? {
            None => Ok(GateResult {
                gate: "sandbox.verify".to_string(),
                pass: true,
                file: None,
                metric: None,
                limit: None,
                detail: format!(
                    "sandbox jj workspace verified: {copied} file(s) applied, cargo check passed \
                     (no git-style whitespace check in a jj repo)"
                ),
            }),
            Some(err) => Ok(GateResult {
                gate: "sandbox.cargo_check".to_string(),
                pass: false,
                file: None,
                metric: None,
                limit: None,
                detail: format!("sandbox cargo check failed:\n{err}"),
            }),
        }
    })();

    cleanup();

    // A leftover workspace is a bug: verify the bookkeeping really happened.
    let list = Command::new("jj")
        .arg("workspace")
        .arg("list")
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("sandbox: jj workspace list failed to spawn: {e}"))?;
    let listing = String::from_utf8_lossy(&list.stdout);
    if listing.contains(&name) {
        return Err(format!(
            "sandbox cleanup failed: jj workspace `{name}` is still registered"
        ));
    }
    verdict
}
