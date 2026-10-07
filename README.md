# fmtguard

[![CI](https://github.com/LosEcher/fmtguard/actions/workflows/ci.yml/badge.svg)](https://github.com/LosEcher/fmtguard/actions/workflows/ci.yml)

Scoped, gated Rust formatting for AI agents and incremental workflows.

`cargo fmt` rewrites the **whole workspace**. For an AI agent that edited a
few lines of `src/router.rs`, that means: unrequested changes to untouched
crates, generated code, and vendored code — plus a diff your reviewer can't
attribute.

**fmtguard never lets the formatter decide the scope. You decide, fmtguard
executes, gates validate.**

```
agent edit ──► VCS diff / explicit changeset ──► rustfmt (whole file)
                                                     │
                                            diff-hunk intersection
                                                     │
                                          minimal clipped patch
                                                     │
                                        mechanical gates (scope,
                                        budget, whitespace) ──► apply?
```

## How it works

1. **Scope detection (L1)** — the harness decides what may be formatted:
   - `--scope-from-git` / `--scope-from-jj`: changed `.rs` files vs a base,
     with the **caller's own diff hunks** used as line ranges;
   - `--changeset <file.json>`: explicit file/range control;
   - exclusion globs (defaults: `generated/**`, `vendor/**`, `target/**`, `node_modules/**`).
2. **Formatting engine (L2)** — stable `rustfmt` formats the whole file to
   stdout, then a line diff is intersected with the scoped ranges: only hunks
   overlapping the caller's change are kept. The formatter is a
   *transformation*, not a file writer.
3. **Mechanical gates (L3)** — every check is an exit code, not prose:
   - `scope.containment`: formatted files ⊆ scoped files;
   - `budget.per_file_added` (default 200 added lines/file);
   - `budget.diff_ratio` (default 3× the caller's own added lines);
   - `budget.max_files` (default 5);
   - `whitespace.clean` (trailing / whitespace-only added lines).
   Any failing gate rejects the run with `exit 1` — and `--apply` writes
   nothing.
4. **Output & audit (L4)** — default is dry-run. `--emit json` gives a machine
   report, `--emit patch` a unified diff. Every run appends an event-sourced
   JSONL log (`.fmtguard/runs.jsonl`) so reports, stats and audits are
   replayable projections of the log — `fmtguard replay <runId>` rebuilds a
   run's patch byte-for-byte.
5. **Idempotency** — formatting the formatted output must be a no-op; a
   formatter that keeps moving fails the `engine.idempotent` gate.
6. **Sandbox (optional)** — `--apply --sandbox` verifies the to-be-applied
   patch in an isolated `git worktree` (`git diff --check`) before touching
   the main tree; the worktree is always cleaned up (git only).

## Install

```sh
# After a crates.io release
cargo install fmtguard

# or from GitHub
cargo install --git https://github.com/LosEcher/fmtguard

# or from a local checkout
cargo install --path .
```

Requires `rustfmt` (stable) on PATH (`--rustfmt /path/to/rustfmt` to override).
`fmtguard doctor` verifies that the formatter actually behaves (it formats a
probe snippet), and `fmtguard --require-version X.Y.Z` turns a stale install
into exit 2 — put it in your agent instructions and CI.

## Usage

```sh
# Format only the files an agent changed in the working tree (dry-run, patch)
fmtguard --scope-from-git --emit patch

# Same, against a different base
fmtguard --scope-from-git --base main --emit json

# Explicit scope with line ranges (agent claims lines 120-180 of router.rs)
fmtguard --changeset changeset.json --emit patch

# Validate, then write the patch (only if every gate passes)
fmtguard --scope-from-git --apply

# optionally require the post-format candidate to be whole-file rustfmt-clean
fmtguard --scope-from-git --apply --verify-fmt-check

# Same, but verify the patch in an isolated git worktree first
fmtguard --scope-from-git --apply --sandbox

# Rebuild a previous run's report/patch from the event log (audit)
fmtguard replay <runId> --emit patch

# Tighter budgets for CI
fmtguard --scope-from-git --budget-max-added-lines 50 --budget-max-ratio 1.5

# Gate only the debt *this* change introduced (default mode); `=strict` also
# requires the whole file to be rustfmt-clean (fails on pre-existing debt)
fmtguard --scope-from-git --verify-fmt-check --emit json

# Rotate a scope boundary away from pre-existing formatting debt
fmtguard --scope-from-git --hunk-context 0 --emit patch

# Budget fmtguard's own diff step (0 disables); on expiry the run fails closed
fmtguard --scope-from-git --diff-timeout-secs 5 --emit patch

# Read-only environment probe (formatter version + behaviour, engines, log)
fmtguard doctor --emit json

# Fail closed on a stale install, in any run
fmtguard doctor --require-version 0.4.0

# Retention for the event log: dry-run by default, --apply archives (never deletes)
fmtguard log prune --keep-runs 200 --emit json
```

### Configuration

Policy can live in files instead of being retyped on every invocation:

- `<repo>/.fmtguard.toml` (repo policy) and `~/.config/fmtguard/config.toml`
  (user policy), lowest precedence first: **defaults → user → repo → CLI**;
- `fmtguard --dump-config` prints the effective values *and the source of each
  key*; `--no-config` ignores the files entirely.

```toml
rustfmt = "rustfmt"
engine = "e3"
engine_timeout_secs = 60
hunk_context = 3
diff_timeout_secs = 10
include_untracked = false
verify_fmt_check = "delta"     # off | delta | strict
exclude = ["src/legacy/**"]    # appended to the built-in excludes

[budget]
max_added_lines = 200
max_files = 5
max_ratio = 3.0
```

The parser accepts a documented TOML subset (comments, `[section]`, quoted
strings, integers, floats, booleans, arrays of strings) and **rejects unknown
keys with a line number** rather than ignoring them. Per-invocation switches
(`--apply`, `--sandbox`, `--emit`, `--changeset`, `--log`, `--require-version`)
deliberately cannot come from a file: a config file that silently enables
writing is exactly the surprise this tool exists to prevent.

`changeset.json`:

```json
{
  "base_ref": "HEAD",
  "files": [
    {
      "path": "src/router.rs",
      "ranges": [{ "start": 120, "end": 180, "reason": "added_handler" }],
      "agent_added_lines": 30
    }
  ]
}
```

Ranges are 1-based inclusive line ranges in the working tree; omit `ranges` to
format the whole file. `agent_added_lines` feeds the diff-ratio gate.

### Event log

Every run appends to `<repo>/.fmtguard/runs.jsonl` (gitignore it). Each event
carries an RFC 3339 `ts`; a run that aborts with exit 2 writes a `run_error`
plus an `error` report event, so `fmtguard replay <runId>` can explain *why* it
failed instead of leaving a dangling `run_start`. A log whose last line is a
torn write is renamed to `runs.jsonl.corrupt-<ts>` before the next append
(evidence kept, never appended to). `fmtguard log prune --keep-runs N` (or
`--older-than 7d`) archives dropped runs under `.fmtguard/archive/` — dry-run
unless `--apply`.

### Exit codes

| code | meaning |
|------|---------|
| 0 | ok (or nothing to do) |
| 1 | rejected by a mechanical gate — nothing was written |
| 2 | error (usage, VCS, engine, I/O) |

## Design notes

Full rationale and the rejected-alternatives table: [`docs/DESIGN.md`](docs/DESIGN.md).

- **The harness owns the scope.** `rustfmt --file-lines` is unstable and
  `cargo fmt` is unscoped; fmtguard instead clips whole-file rustfmt output to
  the caller's ranges, which works on stable toolchains.
- **Event-sourced by default.** Append-only JSONL is the single source of
  truth; `--emit` outputs are derived projections (same shape as the mutation
  logs of event-sourced agent runtimes).
- **Fail-closed.** Any uncertainty (not a repo, engine error, budget overflow)
  refuses to write. `--apply` only runs after every gate passes.
- **`ok` is scoped, not a claim about the repo.** `verdict: ok` means "this
  change introduced no formatting debt inside its scope". Hunks the clip
  dropped are reported (`out_of_scope_hunks`, `in_scope_debt_hunks`,
  `fmt_clean`) and a non-zero drop count prints an explicit
  "this verdict does NOT assert repo cleanliness" line; use
  `cargo fmt --all --check` (or `--verify-fmt-check=strict`) for repo-level
  cleanliness.
- **Rejected alternatives** (full list in [`docs/DESIGN.md`](docs/DESIGN.md)): direct
  `cargo fmt` (scope owned by the formatter), `--file-lines` as the only
  engine (nightly-only), a from-scratch tree-sitter patch formatter (diverges
  from rustfmt output), a formatting daemon (process-model cost for no
  benefit), prose-only CI rules (agents don't follow prose).

## Development

```sh
cargo test                 # unit tests
bash test/gates.sh         # end-to-end mechanical acceptance gates
```

## AI agents

If you are an AI coding agent (Codex, Claude Code, Cursor, DSH) driving this
tool, read **[docs/AGENT-GUIDE.md](docs/AGENT-GUIDE.md)** — it covers the
changeset protocol, report/exit-code semantics, budget tuning and the
fail-closed boundaries.

## License

MIT
