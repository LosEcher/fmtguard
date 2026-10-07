# fmtguard — design

> Status: current as of v0.4.2 (2026-10-07). This is the in-repo design
> document; the cross-project feedback ledger lives in
> [`FEEDBACK.md`](FEEDBACK.md) and the agent-facing usage guide in
> [`AGENT-GUIDE.md`](AGENT-GUIDE.md).

## 1. Problem

`cargo fmt` / `rustfmt` own the scope: they rewrite the whole file (or the whole
workspace), including code the caller never touched. For an agent that edited
three lines, that produces a diff nobody can attribute, review churn across
untouched crates, and — in a repo with pre-existing formatting debt — a
"formatting fix" that is really a 400-line reformat.

The inverted contract:

> **The caller owns the scope. fmtguard executes it and validates it. If the
> scope is not declared clearly enough, fmtguard refuses (fail-closed) rather
> than guessing.**

## 2. Core abstraction

```
scope (L1) ──► formatter (L2) ──► clip + gates (L3) ──► apply / audit (L4)
```

| Layer | What it does | Where |
|-------|--------------|-------|
| L1 scope | git/jj diff hunks, or an explicit `--changeset` with 1-based line ranges; exclusion globs; untracked-file detection | `src/scope.rs` |
| L2 engine | whole-file rustfmt via stdin (E3, default); optional rust-analyzer `rangeFormatting` (E1, experimental) | `src/engine.rs`, `src/lsp.rs` |
| L3 clip + gates | diff `original` vs `formatted`, keep only hunks that intersect the declared ranges, then run mechanical gates | `src/engine.rs`, `src/gates.rs` |
| L4 output | dry-run by default; `--emit json|patch`; append-only event log; `replay`; `log prune`; `--sandbox` isolation | `src/report.rs`, `src/events.rs`, `src/replay.rs`, `src/logctl.rs`, `src/sandbox.rs` |

Exit codes are part of the contract: `0` ok / nothing to do, `1` rejected by a
gate (nothing written), `2` error — including "the environment cannot do the
job" (`doctor`) and "the clip budget expired".

## 3. Decisions that follow from the contract

1. **Clip whole-file output instead of asking rustfmt to format a range.**
   `rustfmt --file-lines` is nightly-only; `cargo fmt` is unscoped. Clipping
   works on stable and keeps the formatter a *transformation*, not a writer.
2. **`verdict: ok` is scoped, never a claim about the repo.** Hunks the clip
   dropped are reported (`out_of_scope_hunks`), and a non-zero drop count prints
   an explicit "this verdict does NOT assert repo cleanliness" line.
3. **Debt is charged to the change that introduces it** (`--verify-fmt-check`
   default `delta`): pre-existing, out-of-scope debt is counted and reported,
   not used to reject an unrelated run. `=strict` (whole-file fixed point)
   remains available for repos that are already clean.
4. **Fail-closed everywhere.** Not a repo, engine failure, budget overflow,
   expired clip budget, unreadable log — none of them write; each is an exit 2
   with an actionable message.
5. **Event-sourced audit.** `.fmtguard/runs.jsonl` is the source of truth;
   reports, patches and statistics are derived projections, and `replay`
   rebuilds a run's report/patch byte-for-byte. Failures leave evidence
   (`run_error`), a dangling `run_start` replays as `interrupted` — never as
   `ok`. Retention is explicit (`log prune`, dry-run by default, archive not
   delete) and a torn log tail is quarantined rather than appended to.

## 4. Rejected alternatives

| Rejected | Why |
|----------|-----|
| `cargo fmt` / whole-workspace formatting | The formatter owns the scope; attribution and review are lost. |
| `rustfmt --file-lines` as the only engine | Nightly-only; `RUSTC_BOOTSTRAP=1` is not portable. Kept as a benchmark reference only. |
| Tree-sitter (or hand-written) patch formatter | Diverges from rustfmt output; two formatters means no arbiter. |
| Formatting daemon | Process-model cost with no benefit at this invocation rate. |
| Prose-only CI rules ("agents should format carefully") | Agents do not follow prose; only exit codes are followed. |
| Auto-expanding the scope until the file is rustfmt-clean | Hands the scope decision back to the formatter, which is the problem being solved. |
| Rejecting every run in a repo that carries debt (`--verify-fmt-check` strict as the default) | Measured: an untouched 1-line debt rejects an unrelated, correctly formatted change. Hence `delta`. |
| Raising the engine timeout to "fix" large-file timeouts | Measured: the failure was a pipe deadlock (stdout/stderr drained only after exit), not a slow formatter. Timeouts are bounded, not padded. |
| `log prune` deleting runs by default | The ledger is the audit trail; pruning archives, and only with `--apply`. |
| A config file that can enable `--apply` / `--sandbox` | A config file that silently enables writing is exactly the surprise fail-closed design exists to prevent. |

## 5. Known limits

- Scope isolation is exact only when the neighbouring foreign change is ≥ 7
  lines away (hunk grouping merges changes within `2×context`; see
  `--hunk-context`).
- The clip diff costs O(changed lines) time; `--diff-timeout-secs` bounds it and
  fails closed rather than approximating the scope.
- E1 (`--engine e1`) needs rust-analyzer with `rustfmt.rangeFormatting.enable`;
  it is not the default and its output is not yet benchmarked against E3.
- `--sandbox` needs a git or jj repository; an explicit `--changeset` scope has
  no VCS to isolate from.
- `doctor` never talks to the network, so "latest version" checks are the
  caller's job (`--require-version` takes the baseline as an argument).
