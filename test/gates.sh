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

# ---------------------------------------------------------------- summary
echo
if [ "$FAILS" -gt 0 ]; then
  printf '\033[31m%d gate(s) FAILED\033[0m\n' "$FAILS"
  exit 1
else
  printf '\033[32mall gates passed\033[0m\n'
  exit 0
fi
