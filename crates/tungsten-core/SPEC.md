# `tungsten-core`: proving the interpreter and the checker

Status: implemented. `scripts/verify.sh` passes with 531 verified, 0 errors, in about 20 seconds, with no
gaps. §12 lists where the implementation differs from this design.

This implements `TESTING.md` §7 and `VERUS.md` §7. The amendments in §9 change both.

## 1. Summary

Today Verus proves that every function in this crate is free of out-of-bounds indexing and overflow,
and that it terminates. It proves nothing about what the functions compute. This work adds a
mathematical semantics of the core IR and proves that the code that actually runs implements it, and
that programs the checker accepts never get `Stuck`.

### Goals

Four theorems, plus the wf agreement that `VERUS.md` §7 promises:

| # | Theorem | Holds for |
| --- | --- | --- |
| T1 | **Refinement with memo soundness.** `eval_program`'s cells equal `spec_eval_program`, memo on or off, at every fuel | Every program, checked or not |
| T2 | **Fuel monotonicity.** A cell that isn't `OutOfFuel` at fuel `f` has the same result at every `f2 ≥ f` | Every program |
| T3 | **Layout.** `decode(flat(c, d), d) == c` and `flat(decode(k, d), d) == k` | In-range cells of grids with dims ≥ 1 that fit `usize` |
| T4 | **Type soundness.** See §5.4 | Programs passing `check`, cases passing `check_case` |
| T5 | **Wf agreement.** `check_wf(p).is_empty() ⟺ wf_program(p)` | Every program |

The trusted base also shrinks: integer primitives become verified code, leaving 22 float primitives
(§4).

### Non-goals

- Proving `CaseResult.failures` exactly. Its entries are proven correct, but its discovery order and
  completeness stay tested (§3.5).
- **Checker completeness.** `check` ⇒ `well_typed` is one-way. Rejecting a good program is a usability
  bug, not a soundness bug, and stays covered by tests. T5 is the only exception.
- Proving `check/scope.rs` correct. The scoping rules exist for backends; the interpreter never reads
  those lists, so soundness doesn't need them (§5.2).
- IEEE-754 correctness. Floats stay uninterpreted in spec and exec, as `TESTING.md` already decides.
- The lowering, backends, parser, testsuite and `Externs` implementations.
- Stack overflow and allocation failure. Verus doesn't model them, so every theorem is about runs that
  aren't killed by either.
- Removing the interpreter's run-time `Stuck` checks. They stay, because T1 covers unchecked programs.

## 2. Layout

```
crates/tungsten-core/src/
  spec/                 # definitions only; this is what a reader has to audit
    value.rs            # SValue, SFlow, SFrame, SRanks and the views from exec types
    prim.rs             # integer definitions; uninterpreted float functions
    layout.rs           # spec_len, spec_flat, spec_decode
    eval.rs             # spec_eval_expr … spec_eval_program
    typing.rs           # spec_type_of, spec_flow_*, spec_ranks, well_typed, case_fits
  proofs/               # lemmas only, no definitions a theorem statement depends on
    layout.rs           # T3
    fuel.rs             # T2
    soundness.rs        # T4 at spec level
    ranks.rs            # acyclicity lemmas
  prim.rs               # integer primitives, now verified
  prim_float.rs         # the only file allowed external_body (§4)
scripts/proof-gaps.txt  # the hole allowlist (§7)
```

`spec/` and `proofs/` are separate so the audit surface can be measured and read on its own. `ensures`
clauses stay on the exec functions, next to the code.

## 3. Refinement (T1)

### 3.1 Spec-side data

Verus has no way to build a `Vec` in a `spec fn`: vstd exposes only its view as a `Seq`. So anything
the spec constructs needs a `Seq`-based twin. A `Value` can't be the result type of a spec function.

| Exec | Spec | Note |
| --- | --- | --- |
| `Value` | `SValue` | Same variants; `Array` and `Record` hold `Seq<SValue>`. Floats stay bits |
| `Flow` | `SFlow` | |
| `Frame` | `SFrame = Seq<Option<SValue>>` | |
| `Ranks` | `SRanks` | Four `Seq<Option<usize>>` |
| `Result<T, E>` | `Result<T@, E>` | |
| `Program`, `Expr`, `Stmt`, `Ctx`, `EvalError` | themselves | Specs read them and never need to construct a `Vec` |

`impl View for Value` gives `v@`. So clauses read `r@ == spec_eval_*(…)`, not `TESTING.md`'s
`result == spec_eval_*(…)`.

### 3.2 Spec shape

The spec is **one total mirror of the exec functions**: one `spec_eval_*` per `eval_*`, same arguments
minus `memo`, taking `ranks` as data and keeping every `Stuck` path, `Stuck(Rank)` included. It is
stateless and unmemoized. Each arm cites the `SEMANTICS.md` section it implements.

The closeness has a cost: review carries more of the assurance than it would for an independent
semantics. Two things offset that. T1 means the hand-written `@expect` corpus, which tests exec,
is evidence about the spec as well. And T4 and T2 are stated against this spec, so a wrong spec tends
to make them unprovable.

Departures from a pure mirror:

- **Loops are recursive.** `spec_while(…, frame, iters)` and `spec_repeat(…, frame, k)`, with the
  remaining count as a last `decreases` component. Exec loops stay Rust loops (`VERUS.md` §5.5). The
  exec invariant is that the initial statement's spec result equals the loop helper's result from the
  current frame and counter.
- **Statements return the frame inside `Ok`.** `Result<(SFlow, SFrame), EvalError>`. A failed
  statement's frame is unobservable, so the spec doesn't describe it.
- **`spec_eval_unit_ctx` needs no build order.** Without state, "the error of the lowest failing
  `InputId` in the list" is a direct definition. The exec loop's `best` proves equal to it.

### 3.3 The exec contracts

```rust
pub fn eval_expr<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &Frame, e: &Expr, memo: &mut Memo)
    -> (r: Result<Value, EvalError>)
    requires env_inv(env), memo_inv(env, old(memo)),
    ensures  r@ == spec_eval_expr(env@, rank, *ctx, frame@, *e), memo_inv(env, memo),
```

Every helper the proofs pass through gets an `ensures`: `util::get`/`set`/`get_rank`/`get_bool`,
`clone_*`, `copy_*`, `fill`, `layout::*`, `value_has_type`, `rank_or` (a result below `current` means
`target == Some(result)`), `binop`/`unop`/`field`/`cast`, `new_memo`.

### 3.4 Memo soundness

Memo soundness is part of T1, because the spec has no memo. The invariant is per id:

```
memo_inv(env, memo) :=
  lengths match the program
  ∧ memo.consts[c] == Some(r) ⟹ ranks.consts[c] == Some(rc) ∧ r@ == spec_eval_const(env@, rc, c)
  ∧ memo.inputs[i] == Some(r) ⟹ ranks.inputs[i] == Some(ri) ∧ r@ == spec_build_input(env@, ri, i)
  ∧ the diagnostics clauses of §3.5
```

`eval_const` and `build_input` get `requires ranks[id] == Some(rank)`. Every call site establishes it,
because the inline `r < rank` check only passes when the id had a rank (the `rank_or` `ensures`). This
is the point where "ranks are a function of the program, not of the caller" (`VERUS.md` §5.4) gets
used. A `build_input` that returns `Memoized` also ensures `memo.inputs[i]` is `Some`, which makes the
`Stuck(BadIndex)` fallback at its readers dead code in the proof. The fallback stays in the code.

The memo-off paths need no extra argument: they compute the spec directly.

### 3.5 Diagnostics

The spec is pure, so it can't describe discovery order without threading state through every function.
Instead:

- **Const failures are exact:** the list is the failing consts in id order, with their spec errors.
- **Input failures are sound:** each `Failure::Input { id, cell, error }` satisfies
  `spec_build_input(id) == Err((cell, error))`.
- **No duplicates:** `listed[i]` holds exactly when `i` has an entry, and entries have distinct ids.
  This relies on `build_input` being reached only with in-range ids, which its rank `requires` gives
  when `ranks.inputs.len() == p.inputs.len()` (part of `env_inv`). Without that, a `set` on `listed`
  would miss and the entry could repeat.

Not proven: that every failing input is listed, the order, and the observation that a listed failure
implies a failing cell on non-empty grids. `memo_off_is_identical` keeps covering order.

### 3.6 Top-level statement

```rust
pub fn eval_program<E: Externs>(p: &Program, case: &Case, ext: &E, fuel: u64, memo_enabled: bool)
    -> (r: CaseResult)
    ensures entries_view(r) == spec_eval_program(p, spec_ranks(p), case@, ext, fuel),
            diagnostics_sound(r, p, spec_ranks(p), case@, ext, fuel),
```

The eval functions' proofs never look inside `ranks`, so they hold for any ranks. Only this last step
needs `ranks(p)` to ensure `r@ == spec_ranks(p)`. `spec_ranks` mirrors `graph`, `node_rank` and the
bounded fixpoint.

### 3.7 Exec changes

`VERUS.md` said this phase only adds. That is relaxed to **behavior-preserving edits**: `cargo test
--workspace` and the corpus stay green, and each edit is noted in `VERUS.md`. Agreed so far:

1. `eval_expr`, `eval_args` and `read_density` take `frame: &Frame`. They never mutate it.
2. `eval_program` takes `memo_enabled: bool` and creates the memo itself. A "memo is fresh" `requires`
   can't be enforced on unverified callers, so this is what makes the theorem unconditional. Two call
   sites change: `tungsten-check/src/eval.rs` and `tests/stuck_mutation.rs`. `EvalOptions.memo` stays.
3. `eval_expr` arms become per-arm helpers (`eval_make_record`, `eval_extern_call`, `eval_binary`, …),
   at layer 0 with the same measure style as `eval_args`. The spec splits the same way, so rule 3 of
   `VERUS.md` §4 still holds. Split up front rather than after Z3 fails: each arm's obligations then
   verify in their own query, and the 10-minute budget (§7) makes this the likely outcome anyway.
4. The primitive changes in §4.

The result type and the semantics of cells don't change, apart from NaN bits (§4.3).

## 4. Primitives

### 4.1 Integers are defined and verified

This amends `TESTING.md` §7.3. With uninterpreted integers, the theorem would hold for any integer
semantics and say nothing about Java's rules, which are what backends keep getting wrong.

`spec/prim.rs` defines each operation mathematically (wrap, truncate toward zero, mask, `i8` through
`i32`), and the bodies in `prim.rs` drop `external_body`. vstd has specs for `wrapping_add`, `_sub`,
`_mul`, `_shl`, `_shr` and `checked_div` on signed types. It has none for two that `prim.rs` uses today,
so those bodies change:

| Op | Body |
| --- | --- |
| `*_div` | `checked_div`, with `None` mapped to `0` for `b == 0` and to `a` for `MIN / -1` |
| `*_neg` | `0.wrapping_sub(a)` |

The vstd `assume_specification`s these rest on join the trusted base as part of Verus. If an operation
can't be verified after real effort, it moves to `prim_float.rs` as a named trusted item with its
`ensures`, and §4.4 gets updated. The existing `prim.rs` edge-case unit tests stay, since they check
the same facts through real execution.

### 4.2 Floats are uninterpreted and trusted

`prim_float.rs` holds 22 `external_body` functions, each `ensures r == spec_*(args)` on an `uninterp
spec fn`: `f32`/`f64` `add sub mul div eq lt le` (14), `f32`/`f64` to `i32`/`i64` (4), `i64` to
`f32`/`f64` (2), and `f32_to_f64`, `f64_to_f32` (2).

Two groups leave the trusted set by being written in terms of verified code:

- `f32_neg` and `f64_neg` become an XOR of the sign bit. Rust defines float negation as exactly that,
  so behavior is unchanged.
- `f32_to_i8` and `f64_to_i8` become `i32_to_i8(f*_to_i32(a))`, so Java's `(byte)` rule is a proven
  composition.

`verify.sh`'s exemption narrows from `prim.rs` to `prim_float.rs`.

### 4.3 NaN canonicalization

The `ensures` above claims each float operation is a function of its input bits. For NaN results, Rust
doesn't promise that: their sign and payload are non-deterministic, and const-folding can differ from
run time. T1's memo soundness would rest on that false axiom.

So `add sub mul div` and `f32_to_f64`, `f64_to_f32` return a fixed quiet NaN whenever the result is
NaN: `0x7ff8_0000_0000_0000` and `0x7fc0_0000`, the values `float_to_bits` and `double_to_bits` already
produce. Literals, hosts and `neg` keep their bits.

Consequences:

- The interpreter's computed NaN bits change. The differential comparison isn't affected, since
  `core_eq` treats any NaN as equal to any NaN.
- **A NaN's sign is still observable** through `copysign(x, nan)`. The interpreter now gives `+|x|`,
  while a backend computing `0.0/0.0` on x86 at run time usually gets a negative NaN and `-|x|`. Such
  programs are declared outside the differential contract. No corpus file does this today (`copysign`
  is only called with `-1.0`). The random program generator, which biases toward NaN, could hit it; if
  it does, the generator must avoid that shape.

### 4.4 Externs and the complete trusted base

The trait gains a spec:

```rust
pub trait Externs {
    spec fn spec_call(&self, ext: ExternId, args: Seq<SValue>) -> Result<SValue, ExternFail>;
    fn call(&self, ext: ExternId, args: &[Value]) -> (r: Result<Value, ExternFail>)
        ensures r@ == self.spec_call(ext, args@.map_values(|v: Value| v@));
}
```

The three implementations are plain Rust outside `verus!{}`, so nothing checks them. The `ensures` is
a **contract**: externs must be pure, including the bits of any NaN they return. Extern results are not
canonicalized (`sqrt(-1.0)` and `fma` can produce non-deterministic NaNs in principle). A walk over
every extern result would cost time on the noise path to remove a risk the implementations can address.

Everything the theorems trust:

| Item | Trust |
| --- | --- |
| Verus, Z3, vstd's `assume_specification`s, rustc | Tools |
| `prim_float.rs` (22 functions) | Bodies match their deterministic spec functions; IEEE correctness is tested only |
| `Externs` implementations | Pure, bit for bit |
| Hand-written `impl Clone`s outside `verus!{}` | Delegate to `clone_*`; verified code doesn't call them |
| `spec/` | Says what `SEMANTICS.md` says. Reviewed, not proven |
| Stack and allocator | Not modeled |

## 5. Type soundness (T4)

### 5.1 Structure

```
check(p) == Ok ∧ program_fits(p)  ⟹  well_typed(p)       ensures on check
check_case(p, case) == Ok         ⟹  case_fits(p, case)  ensures on check_case
well_typed(p) ∧ case_fits(p, case) ⟹ results as in §5.4  proof fn over the spec
```

T4 is proven over the spec and doesn't depend on exec. T1 carries it to `eval_program`.

### 5.2 `well_typed`

```
well_typed(p) := ∃ ctys.
    wf_program(p)
  ∧ ∀ c. spec_type_of(p, no_locals, ctys, consts[c].init) == Some(ctys[c])
  ∧ ∀ f. function_typed(p, ctys, f) ∧ spec_flow_ok(f)
  ∧ ∀ u. functions[u.main].params == 0
  ∧ ∀ grid of an input or entry. dims ≥ 1 ∧ x*y*z ≤ i32::MAX
  ∧ ranks_valid(p, spec_ranks(p))
```

- **`ctys` is a witness**, taken from the exec `const_types` result. This avoids defining constant typing
  recursively in rank order inside the spec.
- **Left out because no-Stuck doesn't need them:** the scoping rules, `ConstForbidden`, and annotation
  checks (`ExternSignature`, `FieldAnnotation`), which the interpreter ignores at run time. `scope.rs` only
  needs an `ensures` that it never removes errors. If a proof turns out to need one of these, it joins
  `well_typed`.
- **`spec_flow_ok`** is the forward analysis of `CHECK.md` §5 without the reporting: reachability, the
  assigned set and loop depth.
- **`ranks_valid`:** every id has a rank, and every edge goes strictly down: function to callee, const
  and input; const to its references; input to source; unit to main, listed inputs and every const.

### 5.3 The checker's `ensures`

The checker collects errors and never returns evidence, so the contracts relate "no new error" to the
spec, for example:

```rust
fn type_expr(cx, e, path, errs) -> (r: Option<Type>)
    ensures errs@.len() >= old(errs)@.len(),
            errs@.len() == old(errs)@.len() && r is Some ==> spec_type_of(…, *e) == r,
```

The awkward points:

- **`None` without a new error** can happen only through `Expr::Const` on a const with no derived type.
  That needs the global fact that `const_types` assigns every const a type when there are no errors and no
  cycles, which comes from §5.5.
- **`cx.reported` in flow** suppresses repeat errors. The invariant is that a set flag means an error was
  already reported, so an empty final error list means no suppression happened.
- **`sort_errors`** only needs to preserve length.
- **`check_scope`** only needs to not shrink the list.

### 5.4 The theorem

Stated without any assumption about externs, because the interpreter checks their results at run time:

> **T4.** If `well_typed(p)` and `case_fits(p, case)`, then for any `ext` and `fuel`, every entry cell
> and every `failures` entry either has no `Stuck`, or is `Stuck(ExternResult)` or
> `Stuck(ExternDomainUndeclared)`. Every `Ok` cell value has the return type of its unit's `main`, and each
> entry has exactly `len(dims)` cells.

> **T4 corollary.** If `ext` also respects its declarations (on arguments of the declared types, it
> returns a value of the declared type, and `Domain` only when `domain_error` is set), then there is no
> `Stuck` at all.

So a `Stuck` on a checked program is provably the extern's fault. `failures` is covered through §3.5,
since `Prepared::eval` scans it for `Stuck` too.

The proof is a mutual induction on the spec's measure with these invariants: every `Some` frame slot has its
local's declared type; slots assigned per the flow state are `Some`; a statement's `Next`, `Break` and
`Return` agree with the flow result (unreachable means not `Next`; `Break` only at depth > 0); and every
`Ok` result has the static type. Density reads use "`Ok` buffers contain only values of the source main's
return type".

### 5.5 Ranks

`check` needs: no reported cycle ⟹ `ranks_valid`, in `proofs/ranks.rs`:

1. `refs_*` finds every call, const read and density read, so `graph` has each edge.
2. `node_ranks`: `Some(r)` at a node means all its successors are ranked lower, and ranks stay at most `n`,
   so `checked_add` never fails.
3. The loop ends at a fixpoint: each changing round ranks at least one node, and there are `n + 1` rounds.
4. At a fixpoint an unranked node has an unranked successor, so by pigeonhole some unranked node reaches
   itself. `reach` is sound, so `cycles` reports it, contradicting an empty cycle list.

**`program_fits(p)`:** `graph` numbers nodes with `add_or_max`, which saturates. Rank correctness needs
those sums not to saturate, and Verus only knows each length separately fits. So T4's checker link has the
hypothesis `nf + nc + ni + nu < usize::MAX`, strict so that `n + 1` rounds don't saturate either. It is
physically always true, since every element takes memory. It is documented as a named hypothesis rather
than enforced by an error kind that no test could reach. T1 doesn't need it.

## 6. The other theorems

**T2 (fuel).** Over the spec, for every `p`, ranks, case and extern. Each `spec_eval_*` gets the lemma:
result at `f` isn't `Err(OutOfFuel)` ⟹ same result, and same frame on `Ok`, at `f2 ≥ f`. Hand-checked for
the canonical-error points: when the chosen error isn't `OutOfFuel`, every candidate that would outrank it
(lower const id, lower input id, earlier cell) was `Ok` at `f` and stays `Ok`, so the same error wins. The
theorem is per cell and doesn't cover `failures`, which can legitimately change with fuel.

**T3 (layout).** Standalone nonlinear arithmetic, the first proof to land. Exec `layout::*` get `ensures`
against `spec/layout.rs`, which has to mirror the intermediate `checked_mul`s, because `x*y` can overflow
with `z == 0`. T4 uses it for "`k < len(d)` ⟹ `decode` is `Some`, and the `to_i32` conversions succeed".

**T5 (wf).** `check_wf(p).is_empty() ⟺ wf_program(p)`, per function: `w.errs` unchanged ⟺ the matching
`wf_*` spec. T4 only uses `⟹`. Once proven, `crates/tungsten-check/src/tests/wf_mirror.rs` and its agreement
test are deleted (approved in the interview), and `no_panic.rs:137` calls
`check_wf(&l.program).is_empty()` instead of `wf_mirror::is_wf`. Wf then has two definitions rather than
three, and `VERUS.md` §6.4 goes away.

## 7. Tooling

### `verify.sh` and the hole allowlist

The eval functions form one recursive group, and a function's proof relies on its callees' `ensures`. So
once one has a refinement `ensures`, the whole group needs them, and work can only land in pieces with
temporary holes. Those are tracked:

- Each hole sits on a line tagged `// GAP: <id>`.
- `scripts/proof-gaps.txt` lists `<id> <one-line reason>`.
- `verify.sh` fails on an untagged or unlisted hole, and on a listed id it can't find. It prints the count.
- **Ids may be added only by a milestone's opening commit**, the one that introduces the `ensures` needing
  them. After that the list only shrinks, and a milestone isn't done until its ids are gone.
- After M7 the file is deleted and `verify.sh` is strict again.
- **No doc may call a theorem proven while its dependencies have open gaps.**

The guard's grep also gains `assume_specification`, `axiom`, `#[verifier::external`,
`exec_allows_no_decreases_clause` and `assume_termination`. `uninterp` is allowed only in `prim_float.rs`.

### Budget

A full `verify.sh` stays **under 10 minutes** on the 16-core dev machine with the pinned Z3. Functions keep
the default rlimit. A raised rlimit needs a comment saying why, and the first response to a slow proof is to
split it (`spinoff_prover`, opaque definitions, smaller lemmas). Verus remains local and doesn't gate CI.

The Verus pin stays unless a proof actually needs a newer vstd. Bumping follows `VERUS.md` §3.

### Checking the proofs themselves

`scripts/verify-negative.sh`, run by hand at each milestone's end. It applies a short fixed list of deliberate
faults one at a time and requires verification to **fail**: `assert(false)` in a proof (vacuity), swapped
`And` short-circuit in exec only, `>` implemented as `lt(a, b)`, a memo store under the wrong id, `MIN / -1`
returning `0`, a spec arm changed without its exec. It uses `--verify-module` so a run stays affordable.

## 8. Milestones

Each one ends with `verify.sh` and `cargo test --workspace` green, and with an entry in `PAPER.md`'s findings
log (proof size, verification time, difficulties).

| # | Work | Done when |
| --- | --- | --- |
| M0 | **Spike and baseline.** Download the pinned Verus and record the current time and count. Try on a scratch branch: `spec fn` in `Externs` with the plain-Rust impls still compiling; recursive `impl View for Value` through `Vec`; `?` under refinement `ensures`; one arm of `eval_expr` end to end | Each works, or this spec is amended |
| M1 | **Prims and layout.** Split `prim.rs`, verify integers, canonicalize NaN, T3, the `verify.sh` guard and allowlist | Trusted set is the 22 float functions; corpus green |
| M2 | **Spec.** `spec/value.rs`, `prim`, `eval.rs`, with `SEMANTICS.md` citations; exec edits 1–3 | Definitions verify; behavior unchanged |
| M3 | **Leaf refinement.** Helpers, `binop`, `unop`, `field`, `cast`, `value_has_type`, `clone_*` | No gaps in these |
| M4 | **Group refinement.** Opens the gaps for the recursive group: expr, stmt (loops), call, const, input, unit, the memo and diagnostics invariants | The group's gaps are gone |
| M5 | **T1 and T2.** `spec_ranks`, `ranks` `ensures`, `eval_program` theorem; fuel lemmas | T1 and T2 stated with no gaps; docs may call the interpreter proven against its spec |
| M6 | **T5.** Wf iff, delete the mirror | |
| M7 | **T4.** `well_typed`, spec-level soundness, ranks lemmas, checker `ensures`, end-to-end theorem | Allowlist empty and deleted; `verify-negative.sh` passes; §9 applied |

Refinement goes first because it covers the code that actually runs, and writing it will expose spec
mistakes before soundness builds on the spec. If M7 lags, M5 is a complete, claimable result by itself.

### If a proof finds a bug

Expect this. An unprovable obligation that turns out to be a real interpreter or checker bug gets fixed in
the code with a regression test in the corpus or `tungsten-check`, and goes in the `PAPER.md` log as `bug`.
Don't weaken the spec to match. If the fix changes checked-program behavior, backends and `SEMANTICS.md`
change with it, which needs a decision from the maintainer.

## 9. Documentation changes

Applied as the corresponding milestone lands, then folded with `/fold-spec`.

| Document | Change |
| --- | --- |
| `TESTING.md` §7 | `r@ ==` contracts; integers defined and verified; 22 trusted float functions; NaN rule; T4's statement and hypothesis; wf agreement listed |
| `SEMANTICS.md` | **`spec/` is normative for cell results**, and this document is its readable explanation, so a disagreement is a doc bug. The interpreter defines only what the spec leaves open (`failures` order). §2 and §6: computed NaNs are canonical, literals and hosts keep payloads. §11: externs are pure including NaN bits. §12: "tested, proof planned" becomes proven, per milestone |
| `VERUS.md` | §1 what is verified; §4 phase rules (`ensures` and lemmas now allowed, behavior-preserving edits); §5.6, §5.7, §6.4, §7 |
| `CHECK.md` | §1 and §3: promise is proven, two wf definitions |
| `EXEC.md` | Refactored arms; `eval_program` signature; §3's note on inline rank checks (`rank_or` now has an `ensures`, though the inline comparison stays) |
| `README.md`, `tungsten-check/README.md` | Status, file table, "When you change something" gains spec and proof steps |
| `PAPER.md` | Status section; trust boundary; the copysign limitation |
| Property tests | Keep `memo_off_is_identical`, `fuel_is_monotone`, `stuck_mutation`. Reword from "stand-in for the proof" to "integration check of trusted parts": real prims, real externs and the unverified lowering together |

## 10. Decisions

Each gives the choice and what it replaced, so it doesn't get "simplified" back.

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Scope | All of `TESTING.md` §7.2 plus wf agreement | Interpreter theorems only, or refinement only: smaller, but leave the paper's headline result undone |
| Integers | Defined and verified | Uninterpreted: theorem holds for any integer semantics. Defined with trusted bodies: wrong bodies go uncaught |
| Float determinism | Canonicalize NaN results | Document the assumption: rests on something Rust doesn't promise. Relational spec: every theorem up to NaN equivalence |
| Extern NaNs | Contract only | Canonicalizing results: a walk on the noise path. Fixing `copysign` across runtimes: effort for a corner no program uses |
| Diagnostics | Cells exact, consts exact, inputs sound | Full equality: threads state through every spec function. Cells only: `Prepared::eval` and the oracle read `failures` |
| Spec shape | One total mirror with ranks as data | Added clean semantics: a second semantics and an equivalence proof. Rank-free: refinement would need the checker, against "never trust the checker" |
| Spec values | `SValue` over `Seq` | Uninterpreted `Seq → Vec` function: a needless axiom |
| Exec edits | Behavior-preserving allowed | Add-only: heavier proofs and an unenforceable `requires` on `eval_program` |
| Partial work | Tagged allowlist | Long-lived branch: drifts while other work lands. One big change: no checkpoints in the largest step |
| Wf | Iff, delete the mirror | One-way: leaves three definitions, or a broken `VERUS.md` promise |
| Budget | 10 minutes, default rlimit | Under 3: heavy engineering up front. None: unrun proofs, since CI doesn't gate them |
| Memo-off and tests | All kept | Removing memo-off: `failures` order loses its only test. Pruning tests: nothing catches a false trust assumption |
| Order | Refinement, then soundness | Soundness first: a spec bug found later redoes it. Parallel: spec changes invalidate work in flight |
| Extern assumption in T4 | Unconditional, plus corollary | Hypothesis only: says nothing about misbehaving externs |
| Graph sums | `program_fits` hypothesis | New error kind: untestable. Rewrite: about 490 lines of working code |
| Normative source | `spec/` | Interpreter stays the reference: undersells T1 |
| `well_typed` contents | Only what no-Stuck needs; `ctys` as a witness | Mirroring all of `check`: proves scope rules no theorem uses |
| `neg` and float-to-`i8` | Rewritten over verified code | Keeping them trusted: larger base for no gain |
| File split | `prim_float.rs` is the exemption | Exempting `prim.rs`: `external_body` could creep back onto integers unnoticed |
| Spec validation | Citations, corpus through T1, `verify-negative.sh` | Spec-level tests with `by(compute)`: programs contain `Vec`s, which a spec can't build |

## 11. Risks and open items

- **M0 assumptions, none tested yet.** The riskiest are recursion through `Vec` in `impl View for Value`, and
  whether a `spec fn` in `Externs` leaves the plain-Rust impls compiling.
- **The budget may not hold.** No baseline exists, because `target/verus` isn't present. M0 sets one. If
  it is already a large share of 10 minutes, the budget gets revisited rather than quietly missed.
- **M4 and M7 are large.** This spec gives no effort estimate, because one wouldn't be reliable before M3.
- **Unverified claims in this document.** The T2 canonical-error argument was checked by hand, and the claim
  that T4 needs no scoping rules comes from reading `exec/`. Both get settled by the proofs; if wrong, the
  statements change.
- **Random generator and `copysign`.** See §4.3.
- **Relationship to the `cse` branch.** The optimizer spec says it doesn't change `tungsten-core`, so no
  conflict is expected. `perform_cse` and `perform_dce` aren't covered by these theorems.

## 12. Implementation notes

All five theorems are proven as designed. T1 is the `ensures` on `exec::eval_program`, T2 is
`proofs/fuel.rs::theorem_fuel_monotone`, T3 is `proofs/layout.rs`, T5 is the `ensures` on `check_wf`, and T4 is
the `ensures` on `check` and `check_case` plus `proofs/soundness.rs::theorem_checked_run`.

Differences from the design:

- **No hole allowlist.** §7's `proof-gaps.txt` never existed, because no milestone needed a gap: the recursive
  group's proofs went through together. `verify.sh` went straight to its strict end state.
- **`uninterp` is allowed in `spec/prim.rs`,** not `prim_float.rs` as §7 says, because that's where §2 puts the
  uninterpreted functions. `verify.sh` also counts the 22 on both sides, which §7 didn't ask for.
- **`eval_expr` is split less than §3.7 expected:** only `eval_binary`, `eval_extern_call`, `eval_index`,
  `eval_make_vec3`, `eval_make_pos3` and the `MakeRecord` helpers. Making `memo_inv` opaque took care of the
  rest. Those arms take the whole expression as `Ghost` for their measure.
- **More behavior-preserving exec edits than §3.7 lists:** `eval_cells`, `buffer_of`, `pass`, `ref_edges`,
  `unit_edges`, `refs_exprs`, `refs_fields` and `cycle_members` are functions of their own. Two loops' `break`s
  became `return`s. Dead shift arms left `i32_op`. `ArrayLit` uses `eval_args`. `type_expr` is split like `eval_expr`.
- **`check_scope` has no contract.** §5.3 asked for one saying it doesn't shrink the list. Instead its errors go
  to a list of their own, appended before the sort, so scope.rs is untouched. The order of errors is unchanged.
- **`spec_best_input` is a fold,** not the direct definition §3.2 suggests. It is still stateless.
- **`listed` is proven one-way.** §3.5 says "exactly when", but a failure implying `listed` is all that
  no-duplicates needs. No-duplicates itself is proven.
- **`spec_ranks_valid` gained `rank < usize::MAX` for consts,** which `const_types`' round count needs, and T4's
  statement also covers `failures`, as §5.4 promised, by bounding each entry's error.
- **Citations are per function, not per arm** (§3.2). Most `spec_eval_*` functions and operator groups cite
  `SEMANTICS.md`; a reader auditing one arm follows that function's citation.
- **`verify-negative.sh` runs full verification** rather than `--verify-module` (§7), because a full run takes
  about 20 seconds. It has 8 faults rather than 6.

What the proofs found:

- **A real bug**, as §8 anticipated. `eval_entry` checked the rank before the layout, which broke fuel
  monotonicity on unchecked programs. Fixed in the code, with a regression test, not by weakening T2 (`PAPER.md`
  findings log, 2026-11-04). It can't change a checked program.
