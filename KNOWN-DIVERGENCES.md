# Known divergences from `rustfmt`

fmtguard is not a formatter. It is a *scoped, gated executor* for `rustfmt`, so on
purpose it does not always produce the same bytes as running `rustfmt` over a
whole file. Those intentional differences are listed here as data, and
`test/gates.sh` (G24) asserts that **every entry still reproduces**. When an entry
stops reproducing, the gate fails and this file must be changed deliberately —
otherwise the list rots into a description of a tool that no longer exists.

The differential gate G23 pins the other direction: for a scope that covers a
whole file, our bytes must be *identical* to `rustfmt` on the same input. This
file covers only the cases where they must legitimately differ.

| entry | subject | reproduces in |
|---|---|---|
| KD-1 | bytes outside the declared scope are never written | G24 arm KD-1 |
| KD-2 | a rejected run writes nothing, where `rustfmt` would have written | G24 arm KD-2 |

### KD-1 — out-of-scope bytes are never written

- subject: a run whose declared scope covers only part of a file
- input: a file with two misformatted sites more than 6 lines apart, of which only
  the first is inside `ranges` (closer sites are fused into one hunk by the hunk
  merger, which would hide the divergence)
- reference: `rustfmt` formats both sites
- ours: the in-scope site is formatted, the out-of-scope site keeps its original bytes
- reason: scope containment is the tool's entire reason to exist. Reformatting a
  line the caller did not declare would move unrelated bytes into the change set,
  which is exactly the failure fmtguard was built to prevent. Callers who want
  whole-file behaviour declare the whole file (no `ranges`), and G23 pins that
  path to byte-equality with `rustfmt`.
- revive: if fmtguard ever reformats outside the declared scope, this entry is void
  and G23's whole-file arms are no longer sufficient to describe the contract.

### KD-2 — a rejected run writes nothing

- subject: a run that a budget gate rejects (here `--budget-max-added-lines 1`)
- input: a file with a small misformatted edit
- reference: `rustfmt` has no budget and would rewrite the file
- ours: exit 1, no file written, `rejections[]` names the gate, metric and limit
- reason: fail-closed. A budget is a statement about how much unrelated churn the
  caller will accept; applying the formatting anyway would convert a refusal into
  the very damage the budget was set to avoid. The same rule covers every other
  rejection path (scope containment, whitespace, engine idempotency, timeouts).
- revive: if a rejected run ever leaves bytes on disk, this entry is void and the
  exit-code contract in `README.md` needs rewriting.
