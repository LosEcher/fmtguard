# fmtguard

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
   replayable projections of the log.

## Install

```sh
cargo install fmtguard
# or build from source:
cargo build --release        # binary at target/release/fmtguard
```

Requires `rustfmt` (stable) on PATH (`--rustfmt /path/to/rustfmt` to override).

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

# Tighter budgets for CI
fmtguard --scope-from-git --budget-max-added-lines 50 --budget-max-ratio 1.5
```

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

### Exit codes

| code | meaning |
|------|---------|
| 0 | ok (or nothing to do) |
| 1 | rejected by a mechanical gate — nothing was written |
| 2 | error (usage, VCS, engine, I/O) |

## Design notes

- **The harness owns the scope.** `rustfmt --file-lines` is unstable and
  `cargo fmt` is unscoped; fmtguard instead clips whole-file rustfmt output to
  the caller's ranges, which works on stable toolchains.
- **Event-sourced by default.** Append-only JSONL is the single source of
  truth; `--emit` outputs are derived projections (same shape as the mutation
  logs of event-sourced agent runtimes).
- **Fail-closed.** Any uncertainty (not a repo, engine error, budget overflow)
  refuses to write. `--apply` only runs after every gate passes.
- **Rejected alternatives** (full list in the design document): direct
  `cargo fmt` (scope owned by the formatter), `--file-lines` as the only
  engine (nightly-only), a from-scratch tree-sitter patch formatter (diverges
  from rustfmt output), a formatting daemon (process-model cost for no
  benefit), prose-only CI rules (agents don't follow prose).

## Development

```sh
cargo test                 # unit tests
bash test/gates.sh         # end-to-end mechanical acceptance gates
```

## License

MIT
