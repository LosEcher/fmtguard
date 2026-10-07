#!/usr/bin/env bash
# fmtguard mechanical acceptance gates (P0).
#
# Every gate is exit-non-zero: prose is not a guard, this script is.
# Run from the repo root:  bash test/gates.sh
#
# Fixtures are created in a temp dir; nothing outside it is touched.

set -u

# rustfmt must be reachable; add the default cargo bin dir when present
if [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
if [ -d "/opt/homebrew/bin" ]; then
  export PATH="/opt/homebrew/bin:$PATH"
fi

FG=${FG:-"$PWD/target/debug/fmtguard"}
RUSTFMT=${RUSTFMT:-rustfmt}
FAILS=0

note() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
pass() { printf '  \033[32mPASS\033[0m %s\n' "$*"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAILS=$((FAILS + 1)); }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

make_fixture() {
  # $1 = fixture dir
  local d="$1"
  mkdir -p "$d/src"
  cat > "$d/Cargo.toml" <<'EOF'
[package]
name = "fgtest"
version = "0.1.0"
edition = "2021"
EOF
  cat > "$d/src/main.rs" <<'EOF'
fn main() {
    println!("hello");
}
EOF
  # committed deliberately-misformatted file: fmtguard must never touch it
  cat > "$d/src/unrelated.rs" <<'EOF'
fn     unrelated(     x : i32     ) {
    if x > 0   { println!( "big" ); }
}
EOF
}

# agent-style edit: introduce formatting violations inside main()
misformat_main() {
  local d="$1"
  python3 - "$d/src/main.rs" <<'PYEOF'
import sys
p = sys.argv[1]
content = open(p).read()
content = content.replace('println!("hello");', 'let     x = 1;\n    println!( "hello" );')
open(p, 'w').write(content)
PYEOF
}

# git init + local identity + initial commit (identity is required on CI)
git_init_commit() {
  local d="$1"
  ( cd "$d" \
    && git init -q \
    && git config user.email test@fmtguard.local \
    && git config user.name "fmtguard test" \
    && git add -A \
    && git commit -qm init )
}


# ---------------------------------------------------------------- gate 1
note "G1: git scope — only the agent-changed file is formatted"
D="$WORK/g1"
make_fixture "$D"
git_init_commit "$D"
cp "$D/src/main.rs" "$WORK/g1-main.before"
cp "$D/src/unrelated.rs" "$WORK/g1-unrelated.before"
misformat_main "$D"
OUT=$( cd "$D" && "$FG" --scope-from-git --emit patch 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "expected exit 0, got $RC"
echo "$OUT" | grep -q -- "--- a/src/main.rs" || fail "patch does not contain src/main.rs"
echo "$OUT" | grep -q -- "--- a/src/unrelated.rs" && fail "patch touches unrelated.rs" || true
cmp -s "$WORK/g1-unrelated.before" "$D/src/unrelated.rs" || fail "unrelated.rs bytes changed"
grep -q 'let x = 1;' <<< "$OUT" || fail "patch does not contain the formatted line"
pass "git scope + containment"

# ---------------------------------------------------------------- gate 2
note "G2: budget rejection — formatter over the added-line budget"
D="$WORK/g2"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
OUT=$( cd "$D" && "$FG" --scope-from-git --emit json --budget-max-added-lines 1 2>/dev/null ); RC=$?
[ "$RC" = 1 ] || fail "expected exit 1 (rejected), got $RC"
echo "$OUT" | grep -q '"verdict": "rejected"' || fail "verdict is not rejected"
echo "$OUT" | grep -q 'budget.per_file_added' || fail "rejection lacks budget.per_file_added"
pass "budget rejection (exit 1 + rejected + gate name)"

# ---------------------------------------------------------------- gate 3
note "G3: changeset range clipping — outside the range stays untouched"
D="$WORK/g3"
make_fixture "$D"
# two misformatted spots in one file; the out-of-range one is >6 lines
# away so it forms a separate hunk that clipping must drop
cat > "$D/src/main.rs" <<'EOF'
fn main() {
let     a = 1;
    println!( "first" );
    let ok = 2;
    let ok = 3;
    let ok = 4;
    let ok = 5;
    let ok = 6;
    let ok = 7;
    let ok = 8;
    let     z = 9;
}
EOF
cat > "$WORK/g3.changeset.json" <<'EOF'
{
  "base_ref": "HEAD",
  "files": [
    { "path": "src/main.rs",
      "ranges": [{ "start": 2, "end": 3 }],
      "agent_added_lines": 2 }
  ]
}
EOF
( cd "$D" && "$FG" --changeset "$WORK/g3.changeset.json" --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "expected exit 0, got $RC"
grep -q 'let     z = 9;' "$D/src/main.rs" || fail "region outside the range was modified"
grep -q 'let a = 1;' "$D/src/main.rs" || fail "in-range region was not formatted"
grep -q 'println!("first");' "$D/src/main.rs" || fail "in-range region was not formatted (2)"
pass "changeset range clipping"

# ---------------------------------------------------------------- gate 4
note "G4: clean tree → nothing to do, exit 0"
D="$WORK/g4"
make_fixture "$D"
git_init_commit "$D"
OUT=$( cd "$D" && "$FG" --scope-from-git --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "expected exit 0, got $RC"
echo "$OUT" | grep -q '"verdict": "ok"' || fail "verdict is not ok"
pass "clean tree no-op"

# ---------------------------------------------------------------- gate 5
note "G5: not a repository → fail-closed, exit 2"
D="$WORK/g5"
mkdir -p "$D"
OUT=$( cd "$D" && "$FG" --scope-from-git 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2, got $RC"
pass "non-repo fail-closed"

# ---------------------------------------------------------------- gate 6
note "G6: jj scope (skipped when jj is unavailable)"
if command -v jj >/dev/null 2>&1; then
  D="$WORK/g6"
  make_fixture "$D"
  ( cd "$D" && jj git init 2>/dev/null && jj commit -m init 2>/dev/null )
  misformat_main "$D"
  OUT=$( cd "$D" && "$FG" --scope-from-jj --emit patch 2>/dev/null ); RC=$?
  [ "$RC" = 0 ] || fail "expected exit 0, got $RC"
  echo "$OUT" | grep -q -- "--- a/src/main.rs" || fail "jj patch does not contain src/main.rs"
  echo "$OUT" | grep -q -- "--- a/src/unrelated.rs" && fail "jj patch touches unrelated.rs" || true
  pass "jj scope + containment"
else
  pass "jj unavailable — skipped"
fi

# ---------------------------------------------------------------- gate 7
note "G7: idempotency — a second run after apply changes nothing"
D="$WORK/g7"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
( cd "$D" && "$FG" --scope-from-git --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "expected apply exit 0, got $RC"
OUT=$( cd "$D" && "$FG" --scope-from-git --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "expected second-run exit 0, got $RC"
echo "$OUT" | grep -q '"files_changed": 0' || fail "second run still wants to change files"
pass "idempotency (apply once, second run is a no-op)"

# ---------------------------------------------------------------- gate 8
note "G8: replay — rebuilds the emitted patch byte-for-byte"
D="$WORK/g8"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
( cd "$D" && "$FG" --scope-from-git --emit patch > "$WORK/g8.patch" 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "expected run exit 0, got $RC"
RUNID=$( cd "$D" && "$FG" --scope-from-git --emit json 2>/dev/null | python3 -c 'import sys,json;print(json.load(sys.stdin)["run_id"])' )
[ -n "$RUNID" ] || fail "no run_id in report"
( cd "$D" && "$FG" replay "$RUNID" --emit patch > "$WORK/g8.replay.patch" 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "expected replay exit 0, got $RC"
cmp -s "$WORK/g8.patch" "$WORK/g8.replay.patch" || fail "replay patch differs from original"
pass "replay byte-identical patch"
# replay with a bogus run id must fail closed (exit 2)
( cd "$D" && "$FG" replay no-such-run >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected replay(bad id) exit 2, got $RC"
pass "replay unknown run id fails closed"

# ---------------------------------------------------------------- gate 9
note "G9: sandbox — isolated worktree verification, no leftovers (git only)"
D="$WORK/g9"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
( cd "$D" && "$FG" --scope-from-git --apply --sandbox >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "expected sandbox apply exit 0, got $RC"
grep -q 'let x = 1;' "$D/src/main.rs" || fail "formatted result missing after sandbox apply"
WT=$( cd "$D" && git worktree list | wc -l | tr -d ' ' )
[ "$WT" = 1 ] || fail "sandbox worktree left registered (worktree list = $WT)"
LEFTOVERS=$(ls -d /tmp/fmtguard-sandbox-* 2>/dev/null | wc -l | tr -d ' ')
# only meaningful when no other sandbox run is concurrent; assert no NEW dirs
# for this fixture by checking the repo-level registration, done above.
[ "$LEFTOVERS" -ge 0 ]  # informational; repo-level check is the gate
pass "sandbox apply + worktree cleanup"
# --sandbox without --apply must fail closed
( cd "$D" && "$FG" --scope-from-git --sandbox >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected --sandbox without --apply exit 2, got $RC"
pass "sandbox requires --apply"
# jj sandbox coverage lives in G21 (supported since v0.4.1)

# ---------------------------------------------------------------- gate 10
note "G10: sandbox cargo check failure refuses apply"
D="$WORK/g10"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
printf '\nfn broken() { missing_symbol(); }\n' >> "$D/src/main.rs"
cp "$D/src/main.rs" "$WORK/g10-before"
( cd "$D" && "$FG" --scope-from-git --apply --sandbox >/dev/null 2>&1 ); RC=$?
[ "$RC" = 1 ] || fail "expected cargo-check rejection exit 1, got $RC"
cmp -s "$WORK/g10-before" "$D/src/main.rs" || fail "cargo-check rejection wrote to main worktree"
pass "sandbox cargo check rejection is fail-closed"

# ---------------------------------------------------------------- gate 11
note "G11: large file (rustfmt output > 64 KiB) must not deadlock on the pipe"
D="$WORK/g11"
make_fixture "$D"
python3 - "$D/src/big.rs" "$WORK/g11.line" <<'PYEOF'
import sys
out, linefile = sys.argv[1], sys.argv[2]
lines, target = [], None
for i in range(4000):
    if i == 2000:
        target = len(lines) + 1          # 1-based line of the misformatted fn
        lines.append("pub fn f2000( x :u32)->u32{let y=x+2000;y*2}")
    else:
        lines += [f"pub fn f{i}(x: u32) -> u32 {{", f"    let y = x + {i};", "    y * 2", "}", ""]
open(out, "w").write("\n".join(lines) + "\n")
open(linefile, "w").write(str(target))
PYEOF
LINE=$(cat "$WORK/g11.line")
cat > "$WORK/g11.changeset.json" <<EOF
{ "base_ref": "HEAD",
  "files": [ { "path": "src/big.rs",
               "ranges": [{ "start": $LINE, "end": $LINE }],
               "agent_added_lines": 4 } ] }
EOF
cp "$D/src/big.rs" "$WORK/g11-before"
START=$(python3 -c 'import time;print(time.time())')
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g11.changeset.json" --emit json 2>/dev/null ); RC=$?
ELAPSED=$(python3 -c "import time;print(round(time.time()-$START,1))")
[ "$RC" = 0 ] || fail "expected exit 0 on the large file, got $RC (deadlock regression?)"
echo "$OUT" | grep -q '"verdict": "ok"' || fail "large-file run is not ok"
python3 - "$ELAPSED" <<'PYEOF' || fail "large-file run was slower than 10s (pipe or diff cost)"
import sys
raise SystemExit(0 if float(sys.argv[1]) < 10 else 1)
PYEOF
FIRST=$(echo "$OUT" | python3 -c 'import sys,json;print(json.load(sys.stdin)["files"][0]["rustfmt_first_pass_ms"])')
[ "$FIRST" -gt 0 ] || fail "rustfmt did not actually run (first pass = ${FIRST}ms)"
( cd "$D" && "$FG" --changeset "$WORK/g11.changeset.json" --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "large-file apply exit $RC"
[ "$(grep -c 'pub fn f' "$D/src/big.rs")" = 4000 ] || fail "large-file apply touched unrelated functions"
grep -q 'pub fn f2000(x: u32) -> u32 {' "$D/src/big.rs" || fail "in-scope function was not formatted"
CHANGED=$(diff "$WORK/g11-before" "$D/src/big.rs" | grep -c '^[<>]')
[ "$CHANGED" -le 8 ] || fail "large-file apply changed $CHANGED lines (expected only the scoped function)"
pass "large file formats in ${ELAPSED}s (no pipe deadlock, scope preserved)"

# ---------------------------------------------------------------- gate 12
note "G12: engine timeout stays fail-closed (kill + exit 2 + no writes)"
D="$WORK/g12"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
cp "$D/src/main.rs" "$WORK/g12-before"
cat > "$WORK/slow-rustfmt" <<'EOF'
#!/bin/sh
sleep 5
exit 0
EOF
chmod +x "$WORK/slow-rustfmt"
OUT=$( cd "$D" && "$FG" --scope-from-git --rustfmt "$WORK/slow-rustfmt" \
        --engine-timeout-secs 2 --apply --emit json 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2 on engine timeout, got $RC"
echo "$OUT" | grep -q 'timed out' || fail "timeout message missing"
cmp -s "$WORK/g12-before" "$D/src/main.rs" || fail "timeout wrote to the work tree"
pass "engine timeout fails closed"

# ---------------------------------------------------------------- gate 13
note "G13: doctor probe + --require-version version gate"
D="$WORK/g13"
make_fixture "$D"
git_init_commit "$D"
OUT=$( cd "$D" && "$FG" doctor --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "expected doctor exit 0 with a working rustfmt, got $RC"
echo "$OUT" | grep -q '"probe_ok": true' || fail "doctor did not confirm the formatter behaves"
echo "$OUT" | grep -q '"version"' || fail "doctor report lacks the tool version"
# arm 2: broken formatter path → exit 2 and a named problem
OUT=$( cd "$D" && "$FG" doctor --rustfmt "$WORK/definitely-not-rustfmt" --emit json 2>/dev/null ); RC=$?
[ "$RC" = 2 ] || fail "expected doctor exit 2 with a broken formatter, got $RC"
echo "$OUT" | grep -q '"probe_ok": false' || fail "doctor did not flag the broken formatter"
# arm 3: version gate, both on doctor and on a normal run
( cd "$D" && "$FG" doctor --require-version 9.9.9 >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected doctor --require-version 9.9.9 exit 2, got $RC"
misformat_main "$D"
cp "$D/src/main.rs" "$WORK/g13-before"
( cd "$D" && "$FG" --scope-from-git --apply --require-version 9.9.9 >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected --require-version drift exit 2 on a normal run, got $RC"
cmp -s "$WORK/g13-before" "$D/src/main.rs" || fail "version drift still wrote to the work tree"
( cd "$D" && "$FG" doctor --require-version 0.0.1 >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "expected a satisfied --require-version to exit 0, got $RC"
pass "doctor probe (3 arms) + version gate fail-closed"

# ---------------------------------------------------------------- gate 14
note "G14: aborted runs leave evidence (ts + run_error) and replay reports them"
D="$WORK/g14"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
cat > "$WORK/slow-rustfmt2" <<'EOF'
#!/bin/sh
sleep 5
exit 0
EOF
chmod +x "$WORK/slow-rustfmt2"
( cd "$D" && "$FG" --scope-from-git --rustfmt "$WORK/slow-rustfmt2" --engine-timeout-secs 1 --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2 on the stubbed timeout, got $RC"
LOG="$D/.fmtguard/runs.jsonl"
grep -q '"t":"run_error"' "$LOG" || fail "aborted run wrote no run_error event"
grep -q '"ts":"' "$LOG" || fail "events carry no timestamp"
grep -q '"verdict":"error"' "$LOG" || fail "aborted run wrote no error report_emit"
RUNID=$(grep '"t":"run_start"' "$LOG" | tail -1 | python3 -c 'import sys,json;print(json.loads(sys.stdin.read())["run_id"])')
OUT=$( cd "$D" && "$FG" replay "$RUNID" --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "replay of the aborted run exited $RC"
echo "$OUT" | grep -q '"verdict": "error"' || fail "replay does not report the error verdict"
echo "$OUT" | grep -q 'timed out' || fail "replay lost the failure reason"
pass "aborted run is auditable from the event log"
# a dangling run_start (crash / kill) must not replay as ok
cat > "$WORK/g14-dangling.jsonl" <<'EOF'
{"t":"run_start","run_id":"run-crash-1","version":"0.0.0","cwd":"/tmp","base":"HEAD","vcs":"git","source":"git-diff","rustfmt":"rustfmt","toolchain":"x","dry_run":true,"ts":"2026-10-07T00:00:00.000Z"}
{"t":"scope_detect","source":"git-diff","files":["src/main.rs"],"excluded":[],"ts":"2026-10-07T00:00:00.001Z"}
EOF
OUT=$( cd "$D" && "$FG" replay run-crash-1 --log "$WORK/g14-dangling.jsonl" --emit json 2>/dev/null )
echo "$OUT" | grep -q '"verdict": "interrupted"' || fail "dangling run replays as ok instead of interrupted"
pass "dangling run replays as interrupted"

# ---------------------------------------------------------------- gate 15
note "G15: log prune — dry-run leaves bytes alone, --apply archives and keeps replay exact"
D="$WORK/g15"
make_fixture "$D"
git_init_commit "$D"
for i in 1 2 3; do
  # a committed clean file, then an agent edit → one run with a real patch
  printf 'fn f%s(x: i32) -> i32 {\n    x\n}\n' "$i" > "$D/src/r$i.rs"
  ( cd "$D" && git add -A && git commit -qm "r$i" )
  printf 'fn f%s( x :i32)->i32{ x }\n' "$i" > "$D/src/r$i.rs"
  ( cd "$D" && "$FG" --scope-from-git --emit patch > "$WORK/g15-run$i.patch" 2>/dev/null ); RC=$?
  [ "$RC" = 0 ] || fail "run $i exited $RC"
done
LOG="$D/.fmtguard/runs.jsonl"
RUNS=$(grep -c '"t":"run_start"' "$LOG")
[ "$RUNS" = 3 ] || fail "expected 3 runs in the log, found $RUNS"
cp "$LOG" "$WORK/g15-log.before"
OUT=$( cd "$D" && "$FG" log prune --keep-runs 1 --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "prune dry-run exited $RC"
echo "$OUT" | grep -q '"kept_runs": 1' || fail "dry-run kept_runs is not 1"
echo "$OUT" | grep -q '"mode": "dry-run"' || fail "dry-run did not report its mode"
cmp -s "$WORK/g15-log.before" "$LOG" || fail "dry-run modified the log"
KEPT_ID=$(grep '"t":"run_start"' "$LOG" | tail -1 | python3 -c 'import sys,json;print(json.loads(sys.stdin.read())["run_id"])')
FIRST_ID=$(grep '"t":"run_start"' "$LOG" | head -1 | python3 -c 'import sys,json;print(json.loads(sys.stdin.read())["run_id"])')
OUT=$( cd "$D" && "$FG" log prune --keep-runs 1 --apply --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "prune --apply exited $RC"
[ "$(grep -c '"t":"run_start"' "$LOG")" = 1 ] || fail "prune --apply did not reduce the log to one run"
ls "$D/.fmtguard/archive/"runs-*.jsonl >/dev/null 2>&1 || fail "pruned runs were not archived"
[ "$(cat "$D/.fmtguard/archive/"runs-*.jsonl | grep -c '"t":"run_start"')" = 2 ] || fail "archive does not hold the two pruned runs"
( cd "$D" && "$FG" replay "$KEPT_ID" --emit patch > "$WORK/g15.replay.patch" 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "replay of the kept run broke after prune ($RC)"
cmp -s "$WORK/g15-run3.patch" "$WORK/g15.replay.patch" || fail "replay after prune is not byte-identical"
( cd "$D" && "$FG" replay "$FIRST_ID" >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "a pruned run should no longer replay (expected exit 2, got $RC)"
pass "prune: dry-run free, archive written, kept replay exact"
# selector discipline
( cd "$D" && "$FG" log prune --emit json >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2 without a retention selector, got $RC"
( cd "$D" && "$FG" log prune --keep-runs 1 --older-than 7d >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2 with two selectors, got $RC"
# --older-than must refuse a log without timestamps rather than guess
cat > "$WORK/g15-legacy.jsonl" <<'EOF'
{"t":"run_start","run_id":"legacy-1","version":"0.2.0","cwd":"/tmp","base":"HEAD","vcs":"git","source":"git-diff","rustfmt":"rustfmt","toolchain":"x","dry_run":true}
{"t":"scope_detect","source":"git-diff","files":[],"excluded":[]}
{"t":"report_emit","verdict":"ok","files_changed":0,"total_added":0,"total_removed":0,"outputs":["json"]}
EOF
( cd "$D" && "$FG" log prune --older-than 7d --log "$WORK/g15-legacy.jsonl" >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2 for --older-than on a ts-less log, got $RC"
pass "retention selectors are fail-closed"

# ---------------------------------------------------------------- gate 16
note "G16: a torn log tail is quarantined, not appended to"
D="$WORK/g16"
make_fixture "$D"
git_init_commit "$D"
( cd "$D" && "$FG" --scope-from-git >/dev/null 2>&1 )
LOG="$D/.fmtguard/runs.jsonl"
[ -f "$LOG" ] || fail "first run wrote no log"
printf '{"t":"run_start","run_id":"torn","ver' >> "$LOG"
cp "$LOG" "$WORK/g16-torn"
misformat_main "$D"
( cd "$D" && "$FG" --scope-from-git --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "run after a torn tail exited $RC (should recover)"
CORRUPT=$(ls "$D/.fmtguard/"runs.jsonl.corrupt-* 2>/dev/null | head -1)
[ -n "$CORRUPT" ] || fail "torn log was not quarantined"
cmp -s "$WORK/g16-torn" "$CORRUPT" || fail "quarantined log bytes changed"
grep -q '"run_id":"torn"' "$LOG" && fail "torn line was carried into the new log" || true
python3 - "$LOG" <<'PYEOF' || fail "the new log is not entirely valid JSON lines"
import json, sys
with open(sys.argv[1]) as fh:
    for n, line in enumerate(fh, 1):
        if line.strip():
            json.loads(line)
raise SystemExit(0)
PYEOF
grep -q '"ts":"' "$LOG" || fail "new log events have no timestamps"
pass "torn tail quarantined, evidence preserved, fresh log healthy"

# ---------------------------------------------------------------- gate 17
note "G17: fmt-check — delta charges only new debt, strict charges the whole file"
D="$WORK/g17"
make_fixture "$D"
python3 - "$D/src/debt.rs" "$WORK/g17.line" <<'PYEOF'
import sys
out, linefile = sys.argv[1], sys.argv[2]
L = ["pub fn b( y : u32 ) -> u32 {   // baseline debt, line 1, far outside the edit",
     "    y + 1", "}"]
for i in range(7):
    L += [f"pub fn f{i}() -> u32 {{", f"    {i}", "}"]
L += ["pub fn target() -> u32 {"]
target = len(L) + 1          # 1-based line of the misformatted body below
L += ["    1+0", "}"]
open(out, "w").write("\n".join(L) + "\n")
open(linefile, "w").write(str(target))
PYEOF
LINE=$(cat "$WORK/g17.line")
cat > "$WORK/g17.changeset.json" <<EOF
{ "base_ref": "HEAD",
  "files": [ { "path": "src/debt.rs", "ranges": [{ "start": $LINE, "end": $LINE }],
               "agent_added_lines": 1 } ] }
EOF
# arm 1: no flag → untouched v0.2.x semantics, no debt fields at all
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g17.changeset.json" --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "arm 1 (no flag) exit $RC"
echo "$OUT" | grep -q 'in_scope_debt_hunks' && fail "arm 1 reported a fmt-check field without the flag" || true
# arm 2: delta → pre-existing debt is reported, not charged
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g17.changeset.json" --verify-fmt-check --emit json 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "arm 2 (delta) exit $RC — pre-existing debt must not reject the run"
echo "$OUT" | grep -q '"in_scope_debt_hunks": 0' || fail "arm 2 in_scope_debt_hunks is not 0"
echo "$OUT" | grep -q '"fmt_clean": false' || fail "arm 2 does not report the untouched debt"
echo "$OUT" | grep -q '"out_of_scope_hunks": 1' || fail "arm 2 stats lose the dropped hunk count"
echo "$OUT" | grep -q 'does NOT assert repo cleanliness' || fail "arm 2 lacks the honesty line"
# arm 3: strict → the same run is rejected because the file still carries debt
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g17.changeset.json" --verify-fmt-check=strict --emit json 2>/dev/null ); RC=$?
[ "$RC" = 1 ] || fail "arm 3 (strict) expected exit 1, got $RC"
echo "$OUT" | grep -q '"gate": "engine.fmt_check"' || fail "arm 3 lacks the fmt_check rejection"
echo "$OUT" | grep -q 'verify-fmt-check=delta' || fail "arm 3 detail does not point at the delta mode"
# arm 4: a formatter that leaves NEW work inside the scope fails delta
cat > "$WORK/g17-stub" <<'EOF'
#!/bin/sh
cat
echo "// stub-extra"
EOF
chmod +x "$WORK/g17-stub"
cat > "$WORK/g17-all.changeset.json" <<'EOF'
{ "base_ref": "HEAD", "files": [ { "path": "src/debt.rs", "agent_added_lines": 1 } ] }
EOF
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g17-all.changeset.json" --rustfmt "$WORK/g17-stub" \
        --verify-fmt-check --emit json 2>/dev/null ); RC=$?
[ "$RC" = 1 ] || fail "arm 4 (delta with a moving formatter) expected exit 1, got $RC"
echo "$OUT" | grep -q 'inside the scoped ranges' || fail "arm 4 rejection does not name the in-scope debt"
pass "fmt-check delta/strict semantics (4 arms)"

# ---------------------------------------------------------------- gate 18
note "G18: untracked *.rs are reported, and only formatted on request"
D="$WORK/g18"
make_fixture "$D"
git_init_commit "$D"
misformat_main "$D"
printf 'pub fn brand_new( x :u32)->u32{ x }\n' > "$D/src/new.rs"
ERR=$( cd "$D" && "$FG" --scope-from-git --emit json 2>&1 >"$WORK/g18.json" )
grep -q 'untracked .rs file(s) are NOT in the diff scope' <<< "$ERR" || fail "no warning for the untracked file"
grep -q 'src/new.rs' <<< "$ERR" || fail "warning does not name the untracked file"
grep -q '"untracked"' "$WORK/g18.json" || fail "report does not carry scope.untracked"
python3 - "$WORK/g18.json" <<'PYEOF' || fail "untracked file leaked into scope.files without the flag"
import json, sys
d = json.load(open(sys.argv[1]))
paths = [f["path"] for f in d["scope"]["files"]]
raise SystemExit(0 if "src/new.rs" not in paths else 1)
PYEOF
cp "$D/src/new.rs" "$WORK/g18-new.before"
( cd "$D" && "$FG" --scope-from-git --include-untracked --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "--include-untracked apply exit $RC"
grep -q 'pub fn brand_new(x: u32) -> u32 {' "$D/src/new.rs" || fail "untracked file was not formatted"
cmp -s "$WORK/g18-new.before" "$D/src/new.rs" && fail "untracked file bytes unchanged" || true
# negative control: no untracked file → no warning
( cd "$D" && git add -A >/dev/null 2>&1 && git commit -qm add >/dev/null 2>&1 )
ERR=$( cd "$D" && "$FG" --scope-from-git --emit json 2>&1 >/dev/null )
grep -q 'untracked' <<< "$ERR" && fail "spurious untracked warning with a clean tree" || true
pass "untracked warning + opt-in formatting (3 arms)"

# ---------------------------------------------------------------- gate 19
note "G19: --hunk-context isolates the edit; budget rejections explain hunk merging"
D="$WORK/g19"
make_fixture "$D"
python3 - "$D/src/merge.rs" <<'PYEOF'
import sys
L = ["pub fn debt( x :u32)->u32{ x }",   # out-of-scope formatting debt
     "",                                  # one unchanged line of separation
     "pub fn edited( y :u32)->u32{ y }",  # the caller's edit
     "pub fn tail() -> u32 {", "    0", "}"]
open(sys.argv[1], "w").write("\n".join(L) + "\n")
PYEOF
cat > "$WORK/g19.changeset.json" <<'EOF'
{ "base_ref": "HEAD",
  "files": [ { "path": "src/merge.rs", "ranges": [{ "start": 3, "end": 3 }],
               "agent_added_lines": 1 } ] }
EOF
# arm 1: default context merges the debt into the caller's hunk → rejection explains why
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g19.changeset.json" --emit json 2>/dev/null ); RC=$?
[ "$RC" = 1 ] || fail "arm 1 expected exit 1 (merged hunk over budget), got $RC"
echo "$OUT" | grep -q 'the kept hunk spans' || fail "arm 1 rejection does not explain the merged span"
echo "$OUT" | grep -q -- '--hunk-context 0' || fail "arm 1 rejection does not offer the isolation knob"
# arm 2: context 0 separates the dropped hunk from the kept one
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g19.changeset.json" --hunk-context 0 --emit json 2>/dev/null ); RC=$?
[ "$RC" = 0 ] || fail "arm 2 (--hunk-context 0) expected exit 0, got $RC"
echo "$OUT" | grep -q '"hunks_kept": 1' || fail "arm 2 still merges hunks"
echo "$OUT" | grep -q '"hunks_total": 2' || fail "arm 2 does not see two separate hunks"
echo "$OUT" | grep -q '"min_cross_gap": 1' || fail "arm 2 does not report the gap to the dropped hunk"
# arm 3: with a tight budget the same run reports the distance to the dropped hunk
OUT=$( cd "$D" && "$FG" --changeset "$WORK/g19.changeset.json" --hunk-context 0 \
        --budget-max-ratio 0.5 --emit json 2>/dev/null ); RC=$?
[ "$RC" = 1 ] || fail "arm 3 expected exit 1 under a tight ratio, got $RC"
echo "$OUT" | grep -q 'nearest out-of-scope formatting hunk is 1 line(s) away' || fail "arm 3 lacks the gap diagnosis"
# arm 4: a nonsensical context is rejected instead of silently accepted
( cd "$D" && "$FG" --changeset "$WORK/g19.changeset.json" --hunk-context 9 >/dev/null 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "arm 4 expected exit 2 for --hunk-context 9, got $RC"
pass "hunk-context isolation + merge diagnosis (4 arms)"

# ---------------------------------------------------------------- gate 20
note "G20: config files — repo policy drives behaviour, CLI wins, typos rejected"
D="$WORK/g20"
make_fixture "$D"
git_init_commit "$D"
export XDG_CONFIG_HOME="$WORK/xdg"   # keep the user config out of the fixture
cat > "$D/.fmtguard.toml" <<'EOF'
# repo policy for this fixture
verify_fmt_check = "strict"
engine_timeout_secs = 90

[budget]
max_files = 20
max_ratio = 1.5
EOF
OUT=$( cd "$D" && "$FG" --dump-config 2>/dev/null )
echo "$OUT" | grep -q '"max_files": 20' || fail "repo config did not set budget.max_files"
echo "$OUT" | grep -q '"max_ratio": 1.5' || fail "repo config did not set budget.max_ratio"
echo "$OUT" | grep -q '"verify_fmt_check": "strict"' || fail "repo config did not set verify_fmt_check"
echo "$OUT" | grep -q '"engine_timeout_secs": 90' || fail "repo config did not set the timeout"
echo "$OUT" | grep -q 'file:.*\.fmtguard\.toml' || fail "dump-config does not report the config file as source"
OUT=$( cd "$D" && "$FG" --dump-config --budget-max-files 3 2>/dev/null )
echo "$OUT" | grep -q '"max_files": 3' || fail "CLI did not override the repo config"
python3 - "$OUT" <<'PYEOF' || fail "CLI override is not attributed to `cli`"
import json, sys
d = json.loads(sys.argv[1])
raise SystemExit(0 if d["sources"]["budget.max_files"] == "cli" else 1)
PYEOF
OUT=$( cd "$D" && "$FG" --dump-config --no-config 2>/dev/null )
echo "$OUT" | grep -q '"max_files": 5' || fail "--no-config did not fall back to defaults"
# behaviour, not just the dump: the repo's strict mode must reject untouched debt
python3 - "$D/src/cfg.rs" "$WORK/g20.line" <<'PYEOF'
import sys
# baseline debt at line 1, the edited line far enough away to stay a separate hunk
L = ["pub fn debt( x :u32)->u32{ x }", ""]
for i in range(7):
    L += [f"pub fn f{i}() -> u32 {{", f"    {i}", "}"]
L += ["pub fn edited() -> u32 {"]
target = len(L) + 1
L += ["    1+0", "}"]
open(sys.argv[1], "w").write("\n".join(L) + "\n")
open(sys.argv[2], "w").write(str(target))
PYEOF
# agent_added_lines is generous on purpose: this fixture isolates the *fmt-check*
# mode (the repo file says strict, the CLI says delta), not the ratio budget.
LINE=$(cat "$WORK/g20.line")
cat > "$WORK/g20.changeset.json" <<EOF
{ "base_ref": "HEAD",
  "files": [ { "path": "src/cfg.rs", "ranges": [{ "start": $LINE, "end": $LINE }],
               "agent_added_lines": 6 } ] }
EOF
( cd "$D" && "$FG" --changeset "$WORK/g20.changeset.json" --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 1 ] || fail "repo strict mode did not reject the run (exit $RC)"
( cd "$D" && "$FG" --changeset "$WORK/g20.changeset.json" --verify-fmt-check=delta --apply >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "CLI --verify-fmt-check=delta did not override the repo's strict mode (exit $RC)"
printf 'nope = 1\n' > "$D/.fmtguard.toml"
OUT=$( cd "$D" && "$FG" --dump-config 2>&1 ); RC=$?
[ "$RC" = 2 ] || fail "a typo in the config file must exit 2, got $RC"
grep -q 'unknown config key' <<< "$OUT" || fail "config typo error is not actionable"
pass "config precedence + typo rejection (5 arms)"

# ---------------------------------------------------------------- gate 21
note "G21: jj sandbox — isolated workspace verifies, then cleans up"
if command -v jj >/dev/null 2>&1; then
  D="$WORK/g21"
  make_fixture "$D"
  ( cd "$D" && jj git init >/dev/null 2>&1 && jj commit -m init >/dev/null 2>&1 )
  misformat_main "$D"
  BEFORE=$( cd "$D" && jj log -r 'all()' --no-graph -T 'change_id ++ "\n"' | wc -l | tr -d ' ' )
  ( cd "$D" && "$FG" --scope-from-jj --apply --sandbox >/dev/null 2>&1 ); RC=$?
  [ "$RC" = 0 ] || fail "jj sandbox apply exit $RC"
  grep -q 'let x = 1;' "$D/src/main.rs" || fail "jj sandbox did not write the formatted result"
  WS=$( cd "$D" && jj workspace list | wc -l | tr -d ' ' )
  [ "$WS" = 1 ] || fail "jj sandbox left a registered workspace (jj workspace list = $WS)"
  AFTER=$( cd "$D" && jj log -r 'all()' --no-graph -T 'change_id ++ "\n"' | wc -l | tr -d ' ' )
  [ "$BEFORE" = "$AFTER" ] || fail "jj sandbox left an orphan change ($BEFORE -> $AFTER)"
  pass "jj sandbox apply + workspace/change cleanup"
  # negative arm: a compile error in the candidate must not touch the main tree
  printf '\nfn broken() { missing_symbol(); }\n' >> "$D/src/main.rs"
  cp "$D/src/main.rs" "$WORK/g21-before"
  ( cd "$D" && "$FG" --scope-from-jj --apply --sandbox >/dev/null 2>&1 ); RC=$?
  [ "$RC" = 1 ] || fail "jj sandbox cargo-check rejection expected exit 1, got $RC"
  cmp -s "$WORK/g21-before" "$D/src/main.rs" || fail "jj sandbox rejection wrote to the main tree"
  WS=$( cd "$D" && jj workspace list | wc -l | tr -d ' ' )
  [ "$WS" = 1 ] || fail "jj sandbox rejection left a registered workspace"
  pass "jj sandbox rejection is fail-closed + clean"
else
  pass "jj unavailable — jj sandbox gates skipped"
fi

# ---------------------------------------------------------------- gate 22
note "G22: the clip diff has its own budget and fails closed (attributable error)"
D="$WORK/g22"
make_fixture "$D"
# a file rustfmt rewrites wholesale: the clip (not rustfmt) is the expensive part
python3 - "$D/src/big.rs" <<'PYEOF'
import sys
lines = [f"pub fn f{i}(x:u32)->u32{{let y=x+{i};y*2}}" for i in range(4000)]
open(sys.argv[1], "w").write("\n".join(lines) + "\n")
PYEOF
cat > "$WORK/g22-all.changeset.json" <<'EOF'
{ "base_ref": "HEAD", "files": [ { "path": "src/big.rs", "agent_added_lines": 4000 } ] }
EOF
cp "$D/src/big.rs" "$WORK/g22-before"
( cd "$D" && "$FG" --changeset "$WORK/g22-all.changeset.json" --diff-timeout-secs 1 --apply >/dev/null 2>"$WORK/g22.err" ); RC=$?
[ "$RC" = 2 ] || fail "expected exit 2 when the clip budget expires, got $RC"
grep -q 'scope clipping hit its 1s budget' "$WORK/g22.err" || fail "error does not name the clip budget"
grep -q 'rustfmt timed out' "$WORK/g22.err" && fail "clip budget is still misattributed to rustfmt" || true
cmp -s "$WORK/g22-before" "$D/src/big.rs" || fail "clip-budget failure wrote to the work tree"
# the limit must not fire on an honest large-file edit
python3 - "$D/src/big.rs" <<'PYEOF'
import sys
lines = [f"pub fn f{i}(x: u32) -> u32 {{\n    let y = x + {i};\n    y * 2\n}}" for i in range(4000)]
lines[10] = "pub fn f10( x :u32)->u32{let y=x+10;y*2}"
open(sys.argv[1], "w").write("\n".join(lines) + "\n")
PYEOF
cat > "$WORK/g22-one.changeset.json" <<'EOF'
{ "base_ref": "HEAD",
  "files": [ { "path": "src/big.rs", "ranges": [{ "start": 11, "end": 11 }],
               "agent_added_lines": 6 } ] }
EOF
( cd "$D" && "$FG" --changeset "$WORK/g22-one.changeset.json" --diff-timeout-secs 1 --emit json >/dev/null 2>&1 ); RC=$?
[ "$RC" = 0 ] || fail "a 254 KB file with a one-line edit must fit a 1s clip budget (got $RC)"
pass "clip budget is bounded, attributable and fail-closed (3 arms)"

# ---------------------------------------------------------------- summary
echo
if [ "$FAILS" -gt 0 ]; then
  printf '\033[31m%d gate(s) FAILED\033[0m\n' "$FAILS"
  exit 1
else
  printf '\033[32mall gates passed\033[0m\n'
  exit 0
fi
