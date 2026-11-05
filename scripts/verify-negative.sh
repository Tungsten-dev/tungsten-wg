#!/usr/bin/env bash
# Checks that the proofs can fail: applies deliberate faults to tungsten-core one at a time and
# requires scripts/verify.sh to reject each. Run by hand after changing specs or proofs.
#
#   VERUS_Z3_PATH=/path/to/z3 scripts/verify-negative.sh
#
# The working tree is restored after every fault, including on Ctrl-C.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
SRC=crates/tungsten-core/src

BACKUP=""
TARGET=""
restore() {
    if [[ -n "$BACKUP" ]]; then
        cp "$BACKUP" "$TARGET"
        rm -f "$BACKUP"
        BACKUP=""
    fi
}
trap restore EXIT INT TERM

fail=0
# fault <name> <file> <python-style old text> <new text>
fault() {
    local name="$1" file="$2" old="$3" new="$4"
    TARGET="$file"
    BACKUP="$(mktemp)"
    cp "$file" "$BACKUP"
    if ! OLD="$old" NEW="$new" python3 -I - "$file" <<'PY'
import os, sys
path = sys.argv[1]
s = open(path).read()
old, new = os.environ["OLD"], os.environ["NEW"]
if old not in s:
    print("fault text not found: " + old, file=sys.stderr)
    sys.exit(1)
open(path, "w").write(s.replace(old, new, 1))
PY
    then
        echo "STALE    $name (the code it targets changed; update this script)"
        fail=1
        restore
        return
    fi
    if scripts/verify.sh >/dev/null 2>&1; then
        echo "MISSED   $name: verification still passes"
        fail=1
    else
        echo "caught   $name"
    fi
    restore
}

fault "vacuity: a proof asserts false" \
    "$SRC/proofs/layout.rs" \
    "    reveal(spec_decode);
    let (dx, dy) = (d.x as int, d.y as int);" \
    "    reveal(spec_decode);
    assert(false);
    let (dx, dy) = (d.x as int, d.y as int);"

fault "exec: \`&&\` doesn't short-circuit" \
    "$SRC/exec/expr.rs" \
    "            if !l {
                return Ok(Value::Bool(false));
            }" \
    ""

fault "exec: \`>\` on f64 compares the wrong way round" \
    "$SRC/exec/expr.rs" \
    "BinOp::Greater => Ok(Value::Bool(prim::f64_lt(b, a)))," \
    "BinOp::Greater => Ok(Value::Bool(prim::f64_lt(a, b))),"

fault "exec: the memo stores a const under the wrong id" \
    "$SRC/exec/unit.rs" \
    "set(&mut memo.consts, c.0, Some(clone_result(&r)));" \
    "set(&mut memo.consts, 0, Some(clone_result(&r)));"

fault "prim: MIN / -1 returns 0" \
    "$SRC/prim.rs" \
    "None => if b == 0 { 0 } else { a }," \
    "None => 0,"

fault "spec: an arm changes without its exec" \
    "$SRC/spec/eval.rs" \
    "Ok(false) => Ok((SFlow::Next, frame))," \
    "Ok(false) => Ok((SFlow::Break, frame)),"

fault "checker: a missing return is accepted" \
    "$SRC/check/flow.rs" \
    "        report(errs, CheckErrorKind::MissingReturn { hint }, item, &path);" \
    "        let _ = hint;"

fault "spec: well_typed forgets break placement (soundness must break)" \
    "$SRC/spec/typing.rs" \
    "Stmt::Break => if st.reach && !st.in_loop { None } else { Some(SFlowState { reach: false, ..st }) }," \
    "Stmt::Break => Some(SFlowState { reach: false, ..st }),"

if [[ "$fail" != 0 ]]; then
    echo "verify-negative.sh: some faults were not caught" >&2
    exit 1
fi
echo "verify-negative.sh: every fault was caught"
