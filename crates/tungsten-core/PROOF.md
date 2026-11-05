# The specs and proofs, explained

`tungsten-core` is the reference that the backends are tested against: when a backend and the
interpreter disagree, the interpreter should be right. This document explains why that claim is more
than a hope. It covers what the semantics in `src/spec/` says, what each theorem states, how each
proof works, and what the theorems still take on trust.

It is written for someone who wants to understand or change the proofs. You don't need to know Verus
beyond the syntax table in [VERUS.md](VERUS.md) §2.

| To learn | Read |
| --- | --- |
| What a program computes, in prose | [SEMANTICS.md](SEMANTICS.md) |
| Which programs the checker accepts | [CHECK.md](CHECK.md) |
| How the interpreter is built | [EXEC.md](EXEC.md) |
| How to run Verus, the code rules, the pitfalls | [VERUS.md](VERUS.md) |

---

## 1. The argument in one page

There are three layers, and each is related to the next by a theorem:

```
 check, check_case          exec code that accepts a program and a case
        │  ensures: accepted ⟹ well_typed, case_fits
        ▼
 well_typed, case_fits      spec: what "accepted" means
        │  T4: well typed ⟹ no Stuck, every Ok has its type
        ▼
 spec_eval_program          spec: what a program computes
        ▲
        │  T1: the interpreter's cells equal the spec's
 eval_program               exec code that actually runs
```

Each result rests on a spec function rather than directly on the code:

- **T1** says the code that runs computes `spec_eval_program`. It holds for every program, checked
  or not, with the memo on or off, at every fuel.
- **T4** is a statement about `spec_eval_program` alone. T1 is what carries it over to real runs.
- **T2** (more fuel doesn't change a finished result) is also about the spec, and T1 carries it over
  the same way.
- **T3** (`flat` and `decode` are inverses) is plain arithmetic, used by T4.
- **T5** (`check_wf` agrees with `wf_program`) is the checker's first gate, used by `check`'s contract.

The theorems are not about a model of the code. The `ensures` clauses sit on the same Rust functions
that `tungsten-check` calls, and `cargo build` erases the ghost parts and leaves those functions.

| # | Statement | Where |
| --- | --- | --- |
| T1 | `eval_program` equals `spec_eval_program`; diagnostics are sound | `ensures` on `exec::eval_program` |
| T2 | A cell that isn't `OutOfFuel` keeps its result at any higher fuel | `proofs/fuel.rs`, `theorem_fuel_monotone` |
| T3 | `decode` and `flat` are inverses on in-range cells | `proofs/layout.rs` |
| T4 | Accepted programs get no `Stuck`, and every `Ok` has its static type | `ensures` on `check` and `check_case`, `proofs/soundness.rs` |
| T5 | `check_wf(p)` is empty exactly when `wf_program(p)` | `ensures` on `check_wf` |

On top of these, Verus proves for all exec code that no index is out of bounds, no arithmetic
overflows, and everything terminates, on any input.

---

## 2. Where everything is

| File | Contains | Who should read it |
| --- | --- | --- |
| `src/spec/eval.rs` | `spec_eval_expr` … `spec_eval_program`: the semantics | Anyone auditing the theorems. Start here |
| `src/spec/ops.rs` | Operators, casts, field access, `spec_value_has_type` | Auditors |
| `src/spec/prim.rs` | Integer definitions; the 22 uninterpreted float functions | Auditors |
| `src/spec/layout.rs` | `spec_len`, `spec_flat`, `spec_decode` | Auditors |
| `src/spec/value.rs` | `SValue`, `SFlow`, `SFrame`, `SRanks`, and the views from exec types | Auditors |
| `src/spec/ranks.rs` | The dependency graph and `spec_ranks` | Auditors of T4 |
| `src/spec/typing.rs` | `spec_type_of`, flow, `spec_ranks_valid`, `spec_well_typed`, `spec_case_fits` | Auditors of T4 |
| `src/wf.rs` | `wf_program` and its helpers | Auditors of T4 and T5 |
| `src/proofs/fuel.rs` | T2 | People changing the semantics |
| `src/proofs/layout.rs` | T3 | People changing the layout |
| `src/proofs/soundness.rs` | T4, the spec half | People changing the semantics or typing |
| `src/proofs/typing.rs` | Lemmas behind the checker's contracts | People changing the checker |
| `src/proofs/ranks.rs` | "No cycle reported" ⟹ valid ranks | People changing `acyclic` |
| `src/proofs/value.rs` | Small facts about the views | Rarely anyone |
| `src/prim_float.rs` | The trusted float primitives | Anyone auditing the trusted base |

The split is deliberate. To trust the theorems you must read their statements and everything those
statements mention. That is `spec/` and `wf.rs`, plus a few short definitions kept next to their
theorems:

- in `proofs/soundness.rs`: `allowed`, `result_ok`, `failure_allowed`, `extern_respects`, `args_typed`;
- in `proofs/fuel.rs`: `with_fuel`;
- in `exec/mod.rs`: `diagnostics_sound`, `input_failure_ok`, `failure_input`, `program_env`, `view_entries`.

You don't need to read the lemmas or the proof bodies: Z3 checked them, and if one were wrong,
verification would fail. T1, T5 and the checker half of T4 have no file of their own, because their
proofs are the loop invariants and `proof` blocks inside `exec/` and `check/`.

---

## 3. The semantics

### 3.1 Shape

`spec/eval.rs` has one `spec_eval_*` function per `eval_*` function in `exec/`, with the same
arguments minus the memo. Three properties matter:

- **Total.** It gives a result for every program, including ones the checker would reject. A bad id
  gives `Stuck(BadIndex)`, a wrong operand gives `Stuck(Shape)`, and so on. This is why T1 needs no
  assumptions about the program.
- **Stateless.** There is no memo. A constant is simply recomputed wherever it is read, so the fact
  that the memo changes nothing has to be proven (§4.3).
- **A mirror of the code.** It even keeps the same checks in the same order, so that T1 also pins
  down *which* `Stuck` an ill-formed program gets. See §9 for what this closeness costs.

The read-only inputs travel together in `SEnv`, which is the spec's view of the interpreter's `Env`:

```rust
pub struct SEnv<E> {
    pub p: Program,
    pub ranks: SRanks,
    pub origin: F64x3,
    pub hosts: Seq<SValue>,
    pub ext: E,
    pub fuel: u64,
}
```

### 3.2 Values that a spec can build

A `spec fn` can't construct a `Vec`, since vstd only exposes a `Vec`'s view. So `Value` has a twin,
`SValue`, with `Seq` where `Value` has `Vec`, and `view_value` maps one to the other. Floats stay bits
in both, so equality is exact. A contract then reads `view_result(r) == spec_eval_expr(…)`. The same
goes for `Flow` and `SFlow`, `Frame` and `SFrame`, and `Ranks` and `SRanks`.

`Program`, `Expr` and `Stmt` need no twin, because a spec only reads them. That is why the spec uses
`env.p.consts@[c]` and similar on the real IR types.

### 3.3 Ranks are data

Recursion in a program could go on forever, so the semantics needs something to terminate on. Every
function, const, input and unit has a **rank**, computed from the program alone. A call or read goes
ahead only if the target's rank is below the current one:

```rust
Expr::Const(c) => {
    let rc = spec_rank_or(spec_rank(env.ranks.consts, c.0), rank);
    if rc < rank {
        spec_eval_const(env, rc, c)
    } else {
        Err(stuck(StuckReason::Rank))
    }
},
```

`spec_rank_or` maps "no rank" to `rank` itself, which then fails the test, so an id on or reaching a
cycle is `Stuck(Rank)`. Note what is passed down: the *target's own* rank `rc`, never `rank - 1`. A constant is
therefore evaluated at the same rank whoever reads it, and §4.3 depends on this.

The termination measure is lexicographic: `(rank, layer, structure, loop count, list index)`. The layer
orders the functions that run at one rank, because Verus can't compare the size of a `Stmt` with the
size of an `Expr`:

```
spec_build_input 8 > spec_eval_unit_ctx 7 > spec_best_input 6 > spec_input_error 5 > spec_unit_cell 4
  > spec_eval_cell 3 > spec_eval_call, spec_eval_const 2 > spec_stmts, spec_eval_stmt 1
  > spec_eval_expr, spec_args, spec_fields, spec_read_density 0
```

### 3.4 Where the spec differs from the code

The spec is a mirror, but three things can't be mirrored:

- **Loops are recursive.** `spec_while` and `spec_repeat` take the iterations done so far, where the
  interpreter has Rust loops (recursion would overflow the Rust stack).
- **List functions take an accumulator.** `spec_args(env, …, args, i, acc)` continues from position
  `i`. A spec written this way gives an exec loop a one-line invariant (§4.2).
- **Statements return their frame.** `spec_eval_stmt` returns `Result<(SFlow, SFrame), EvalError>`, where
  exec mutates a `&mut Frame`. A failed statement's frame can't be observed, so it isn't described.

### 3.5 The top of the semantics

```rust
pub open spec fn spec_entry_cells<E: Externs>(env: SEnv<E>, e: int) -> Seq<Result<SValue, EvalError>> {
    match spec_first_const_error(env, env.p.consts@.len() as int) {
        Some(err) => match spec_len(env.p.entries@[e].dims) {
            Some(n) => spec_fill(n as nat, err),
            None => seq![Err(stuck(StuckReason::Layout))],
        },
        None => spec_eval_entry(env, e),
    }
}
```

Reading down from here gives the whole evaluation order: the lowest failing const fails every cell;
otherwise each entry evaluates its unit. Within a unit, `spec_eval_unit_ctx` builds every listed input
first. `spec_best_input` picks the failing input with the lowest id, whose error fills every cell.
Otherwise each cell runs on its own.

`spec_best_input` is written as a fold over the unit's list, because the error it picks must not depend
on the order inputs happen to be built in.

### 3.6 Primitives

**Integers are defined.** `spec/prim.rs` states Java's rules mathematically, and `prim.rs` is verified
against them:

```rust
pub open spec fn spec_i32_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        0
    } else if a == i32::MIN && b == -1 {
        a
    } else {
        rust_div(a as int, b as int) as i32
    }
}
```

With uninterpreted integers the theorems would hold for any integer semantics, and would say nothing
about wrapping, shift masks and `i8` promotion, which are what the backends keep getting wrong.

**Floats are uninterpreted.** `spec_f64_add` and the 21 others are `uninterp spec fn`s. Verus knows
nothing about them except that each is a function: same input bits, same output bits. So the theorems
hold for *any* float semantics, and the proofs can't conclude that `a + b == b + a`. IEEE-754
correctness is tested, not proven.

"Same bits in, same bits out" isn't automatically true of real floats: Rust doesn't promise the sign
or payload of a `NaN` result, and const-folding can differ from run time. If it were false, the memo
could change a result, so T1 would rest on a false axiom. `prim_float.rs` makes it true by returning
one canonical `NaN` from arithmetic and float-to-float conversions.

**Externs** get the same treatment through the trait. `spec_call` is an abstract function of the
extern id and arguments, and `call` promises to compute it:

```rust
pub trait Externs {
    spec fn spec_call(&self, ext: ExternId, args: Seq<SValue>) -> Result<SValue, ExternFail>;

    fn call(&self, ext: ExternId, args: &[Value]) -> (r: Result<Value, ExternFail>)
        ensures view_extern_result(r) == self.spec_call(ext, view_values(args@));
}
```

Implementations are plain Rust outside `verus!{}`, so nothing checks that promise. It is a contract:
an extern must be pure, including any `NaN` bits it returns.

---

## 4. T1: the interpreter computes the semantics

```rust
pub fn eval_program<E: Externs>(p: &Program, case: &Case, ext: &E, fuel: u64, memo_enabled: bool) -> (out: CaseResult)
    ensures
        view_entries(out.entries@) == spec_eval_program(program_env(*p, *case, *ext, fuel)),
        diagnostics_sound(out.failures@, program_env(*p, *case, *ext, fuel)),
```

There is no `requires`, so nothing is assumed about the program, the case, the fuel or the memo flag.
`eval_program` creates its own memo, because a "the memo is fresh" precondition couldn't be enforced
on unverified callers.

### 4.1 Function by function

The proof has no separate induction: each exec function's `ensures` says it computes its spec, and
Verus checks the body assuming the contracts of its callees, with recursion justified by the
`decreases` clause. A typical contract:

```rust
pub fn eval_expr<E: Externs>(env: &Env<E>, rank: usize, ctx: &Ctx, frame: &Frame, e: &Expr, memo: &mut Memo) -> (r: Result<Value, EvalError>)
    requires
        env_inv(env),
        memo_inv(senv(env), *old(memo)),
    ensures
        memo_inv(senv(env), *final(memo)),
        view_result(r) == spec_eval_expr(senv(env), rank, *ctx, view_frame(*frame), *e),
    decreases rank, 0int, e, 1int,
```

Because exec and spec have the same shape, most arms need little or no proof text. The checks in
`eval_*` line up with the branches of `spec_eval_*`, and the two sides meet at each call.

The ranks are arguments too, and these proofs don't look inside them. Only the last step of
`eval_program` needs to know they are the real ones, which `ranks`'s contract gives:
`view_ranks(r) == spec_ranks(*p)`.

### 4.2 Loops

Exec loops match the recursive spec through one equality: the spec's result from the start equals its
result from where the loop has got to.

```rust
loop
    invariant
        spec_while(senv(env), rank, *ctx, start, *cond, *body, 0)
            == spec_while(senv(env), rank, *ctx, view_frame(*frame), *cond, *body, iters as int),
    decreases fuel - iters,
```

One step of the loop is one unfolding of `spec_while`, so the invariant carries through each iteration
with almost no help. When the loop returns, the equality gives the result the contract asks for. The
list loops work the same way, with `spec_args(…, 0, empty) == spec_args(…, i, so_far)`.

### 4.3 Memo soundness

The interpreter reuses results and the spec has no memo, so this is where the two really differ. The
argument is a single invariant, kept by every function in the recursive group:

```rust
pub open spec fn memo_inv<E: Externs>(env: SEnv<E>, memo: Memo) -> bool {
    &&& memo.consts@.len() == env.p.consts@.len()
    &&& memo.inputs@.len() == env.p.inputs@.len()
    &&& memo.listed@.len() == env.p.inputs@.len()
    &&& forall|c: int| 0 <= c < memo.consts@.len() && (#[trigger] memo.consts@[c]) is Some ==> match spec_rank(env.ranks.consts, c as usize) {
        Some(rc) => view_result(memo.consts@[c]->0) == spec_eval_const(env, rc, ConstId(c as usize)),
        None => false,
    }
    // … the same for inputs, and the diagnostics clauses of §4.4
}
```

In words: whatever the memo holds for an id is exactly what the spec computes for that id at that id's
own rank. Then:

- **Reading** the memo returns a clone, and `clone_result` is proven to preserve the view. So a hit
  gives the spec's result.
- **Writing** stores a result just computed, and the contracts of the functions that computed it say
  it is the spec's result.
- **Caller independence.** `eval_const` and `build_input` have a precondition that `rank` *is* the
  id's own rank, for example `spec_rank(env.ranks.consts@, c.0) == Some(rank)`. Every call site can
  establish it, because the inline `rc < rank` test can only pass if the id had a rank (§3.3).

This is why ranks replaced an earlier plan of a global depth counter. With a counter, a constant
computed deep in a call and reused near the top would have been computed with a different budget than
the spec would use, so the memo could really change a result. With ranks, the result of an id can't
depend on who asks.

The invariant never mentions `memo.enabled`. With the memo off nothing is ever stored for a hit to find,
so the same proof covers both modes.

`memo_inv` is `#[verifier::opaque]`: its quantifiers stay out of the solver's context except in the
few functions that `reveal` it. Without that, the recursive group runs out of rlimit.

### 4.4 Diagnostics

The `failures` list isn't fully determined by the spec, because a pure spec has no discovery order. So
the statement is weaker than for cells:

```rust
pub open spec fn diagnostics_sound<E: Externs>(fs: Seq<Failure>, env: SEnv<E>) -> bool {
    let cs = spec_const_failures(env, env.p.consts@.len() as int);
    &&& fs.len() >= cs.len()
    &&& forall|j: int| 0 <= j < cs.len() ==> #[trigger] fs[j] == cs[j]
    &&& forall|j: int| cs.len() <= j < fs.len() ==> input_failure_ok(env, #[trigger] fs[j])
    &&& forall|x: int, y: int| cs.len() <= x < y < fs.len() ==> #[trigger] failure_input(fs[x]) != #[trigger] failure_input(fs[y])
}
```

So const failures are exact and come first. After them, each input failure is true
(`spec_build_input` really fails with that cell and error), and no input appears twice. **Not** proven:
the order of input failures, and that every failing input is listed. `memo_off_is_identical` in
`tungsten-check` tests the order.

---

## 5. T2: more fuel doesn't change a finished result

```rust
pub proof fn theorem_fuel_monotone<E: Externs>(env: SEnv<E>, f2: u64, e: int, k: int)
    requires
        env.fuel <= f2,
        0 <= e < env.p.entries@.len(),
        0 <= k < spec_eval_program(env)[e].len(),
        spec_eval_program(env)[e][k] != Err::<SValue, EvalError>(EvalError::OutOfFuel),
    ensures
        spec_eval_program(with_fuel(env, f2))[e].len() == spec_eval_program(env)[e].len(),
        spec_eval_program(with_fuel(env, f2))[e][k] == spec_eval_program(env)[e][k],
```

This holds for every program and extern, and any ranks. Every lemma in `proofs/fuel.rs` has one shape,
following one spec function: run it at `env` and at `with_fuel(env, f2)`; if the first isn't
`OutOfFuel`, the second is identical.

```rust
pub open spec fn mono<T>(r1: Result<T, EvalError>, r2: Result<T, EvalError>) -> bool {
    r1 != Err::<T, EvalError>(EvalError::OutOfFuel) ==> r2 == r1
}
```

Most cases are immediate, because `OutOfFuel` from a sub-step propagates: a result that isn't
`OutOfFuel` means no sub-step was. In `While`, a loop that finished at fuel `f` never hit the
`iters >= fuel` test, so it never hits it at a higher fuel either.

The one place an `OutOfFuel` can be *swallowed* is the canonical error choice. Suppose a unit lists
inputs 1 and 2. At low fuel, input 1 runs out of fuel and input 2 fails with `DivByZero`, so the unit
reports `OutOfFuel` from input 1, since it has the lower id. At higher fuel input 1 may succeed, and now
the unit reports `DivByZero`. That is allowed, because the low-fuel result was `OutOfFuel`. What must
*not* happen is a real error at low fuel changing at high fuel. The fold is related across the two runs
by `best_rel`:

```rust
pub open spec fn best_rel(b1: Option<(usize, EvalError)>, b2: Option<(usize, EvalError)>) -> bool {
    b1 == b2 || match b1 {
        Some((m1, EvalError::OutOfFuel)) => match b2 {
            None => true,
            Some((m2, _)) => m2 >= m1,
        },
        _ => false,
    }
}
```

The bests are equal, or the low-fuel best was `OutOfFuel` and the high-fuel one is no lower. Consts work
the same way (`lemma_first_const`).

T2 is per cell. It doesn't cover `failures`, which can legitimately change with fuel. Because of T1, it
holds for real runs of `eval_program` too.

---

## 6. T3: layout

```rust
pub proof fn lemma_decode_flat(c: I32x3, d: Dims)   // decode(flat(c, d), d) == c
pub proof fn lemma_flat_decode(k: usize, d: Dims)   // k < len(d) decodes in range, and flat takes it back
```

Both assume dims ≥ 1, and that the arithmetic fits `usize` (`spec_flat` or `spec_len` is `Some`). They
are standalone arithmetic: write
`k == x + y*dx + z*(dx*dy)` as `dx*rest + x`, then apply vstd's uniqueness of division
(`lemma_fundamental_div_mod_converse`) twice. The exec functions in `layout.rs` have
`ensures r == spec_*(…)`, so the result applies to them directly.

`spec_len` and `spec_flat` mirror the intermediate overflow checks of the exec code, because `x*y` can
overflow even when `z == 0`. T4 uses `lemma_flat_decode` for "a cell index below `len` always decodes".

---

## 7. T4: accepted programs don't get stuck

### 7.1 Statement

Stated over the spec:

```rust
pub proof fn theorem_soundness<E: Externs>(env: SEnv<E>, ctys: Seq<Option<Type>>, ext_ok: bool)
    requires
        spec_well_typed_with(env.p, ctys),
        env.ranks == spec_ranks(env.p),
        spec_case_fits(env.p, env.hosts),
        ext_ok ==> extern_respects(env.p, env.ext),
```

It concludes that each entry has `len(dims)` cells, every `Ok` cell has the return type of its unit's
`main`, and every error, in cells and in `failures`, is `allowed`:

```rust
pub open spec fn allowed(ext_ok: bool, e: EvalError) -> bool {
    match e {
        EvalError::Stuck(r) => !ext_ok && (r is ExternResult || r is ExternDomainUndeclared),
        _ => true,
    }
}
```

The ghost flag `ext_ok` lets one proof give two theorems:

- `ext_ok == false` assumes nothing about the extern, and leaves it two ways to get `Stuck`. Since the
  interpreter checks extern results at run time, **a `Stuck` on a checked program is provably the
  extern's fault.**
- `ext_ok == true` assumes `extern_respects` (on arguments of the declared types, it returns the
  declared type, and `Domain` only if declared). Then there is no `Stuck` at all.

T4 doesn't say that evaluation succeeds. `DivByZero`, `OutOfBounds`, `OutOfFuel` and `ExternDomain` are
real outcomes that backends must reproduce.

### 7.2 What `well_typed` contains

```rust
pub open spec fn spec_well_typed_with(p: Program, ctys: Seq<Option<Type>>) -> bool {
    &&& wf_program(&p)
    &&& spec_ranks_valid(p, spec_ranks(p))
    &&& spec_consts_typed(p, ctys)
    &&& forall|k: int| 0 <= k < p.functions@.len() ==> spec_function_typed(p, ctys, #[trigger] p.functions@[k]) && spec_flow_ok(p.functions@[k])
    &&& forall|u: int| 0 <= u < p.units@.len() ==> p.functions@[(#[trigger] p.units@[u]).main.0 as int].params == 0
    &&& forall|i: int| 0 <= i < p.inputs@.len() ==> spec_dims_ok((#[trigger] p.inputs@[i]).dims)
    &&& forall|e: int| 0 <= e < p.entries@.len() ==> spec_dims_ok((#[trigger] p.entries@[e]).dims)
}
```

`spec_well_typed(p)` says some `ctys` exists. `ctys` is each const's derived type, and it is an
existential so the spec doesn't have to define constant typing recursively in rank order; `check`
supplies its own `const_types` result as the witness.

This has only what no-`Stuck` needs, which is less than the checker enforces. Scoping lists,
`ConstForbidden` and annotation checks aren't there, because the interpreter doesn't depend on them.
They exist for the backends, and no theorem covers them.

Each reason for `Stuck` is ruled out by one clause:

| `Stuck` reason | Ruled out by |
| --- | --- |
| `BadIndex` | `wf_program` |
| `Rank` | `spec_ranks_valid`: every id has a rank and every edge goes strictly down |
| `Shape` | `spec_type_of` and `spec_value_has_type`, through the induction below |
| `Unassigned` | `spec_flow_ok`: definite assignment |
| `NoReturn` | `spec_flow_ok`: the end of the body is unreachable |
| `BreakOutsideLoop` | `spec_flow_ok`: `break` only inside a loop |
| `MainParams` | The `params == 0` clause |
| `Layout` | `spec_dims_ok`, and T3 |
| `Case` | `spec_case_fits` |
| `ExternResult`, `ExternDomainUndeclared` | Only `extern_respects`, which is what `ext_ok` selects |

### 7.3 The spec half: an induction on the semantics

`proofs/soundness.rs` follows the spec's own recursion with the same `decreases` measure, one lemma per
spec function.

For **expressions**, the hypothesis `expr_pre` bundles what is needed:

```rust
pub open spec fn expr_pre(p: Program, ranks: SRanks, ctys: Seq<Option<Type>>, locals: Seq<Type>, rank: usize, frame: SFrame, assigned: Seq<bool>, e: Expr, t: Type) -> bool {
    &&& frame_typed(p, frame, locals)         // every Some slot has its local's type
    &&& assigned_some(frame, assigned)        // flow's assigned locals really are Some
    &&& reads_ok(locals, assigned, e)         // e reads only assigned locals, or none at all (a const)
    &&& spec_expr_ranked(ranks, rank, e)      // every call and read goes below the current rank
    &&& spec_type_of(p, locals, ctys, e) == Some(t)
}
```

and the conclusion is `result_ok(ext_ok, p, spec_eval_expr(…), t)`: an `Ok` has type `t`, and an error
is allowed. Operators, casts and field access are separate lemmas (`lemma_binop`, `lemma_cast`, and so
on) saying a well-typed operand can't reach a `Stuck(Shape)` arm. `MakeRecord` relies on "every field
exactly once" in `spec_type_of`, so the slots are filled with no repeat and none left empty. A density
read relies on `lemma_input`: an `Ok` buffer holds only values of the source main's return type.

For **statements**, `stmt_pre` gives the same facts plus the flow state, and the conclusion ties the
run-time result to the flow analysis:

```rust
pub open spec fn stmt_post(ext_ok: bool, p: Program, r: Result<(SFlow, SFrame), EvalError>, frame: SFrame, locals: Seq<Type>, ret: Type, in_loop: bool, after: SFlowState) -> bool {
    match r {
        Err(e) => allowed(ext_ok, e),
        Ok((fl, f2)) => {
            &&& frame_typed(p, f2, locals)
            &&& preserves(frame, f2)
            &&& match fl {
                SFlow::Next => after.reach && assigned_some(f2, after.assigned),
                SFlow::Break => in_loop,
                SFlow::Return(v) => spec_value_has_type(p, v, ret),
            }
        },
    }
}
```

The three arms give the flow results directly:

- **`Next`** implies `after.reach`. A function body whose end flow marks unreachable (`spec_flow_ok`)
  therefore can't finish with `Next`, so there is no `NoReturn`.
- **`Break`** implies `in_loop`, and a body starts with `in_loop` false, so there is no
  `BreakOutsideLoop`.
- **`Return(v)`** has the function's return type, so a call's result has its static type.

`preserves` (an assigned slot stays assigned) is what makes the conservative treatment of loops sound:
flow says the state after a loop is the state before it, and the frame after the loop still has
everything that was assigned before.

**Ranks** let the induction cross calls. `spec_expr_ranked` says every target is below the current
rank, so the `rc < rank` test always passes, and the hypothesis applies to the callee because the
measure dropped.

`env_ok` holds the global assumptions. It is opaque with one accessor lemma per use (`env_const`,
`env_fn`, `env_input`, `env_unit`, …), to keep `well_typed`'s quantifiers out of the big proofs.

### 7.4 The checker half: "no new error" means "the rule holds"

The checker collects errors and returns no evidence, so its contracts relate the length of the error
list to the spec:

```rust
pub fn type_expr(cx: &TCx, e: &Expr, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: Option<Type>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_hyp(*cx, *e) ==> r is Some && r == cx_type_of(*cx, *e),
```

Each stage has this shape. `check` then only has to observe that an empty final list means no stage
added an error, so every stage's conclusion holds:

```rust
pub fn check(p: &Program) -> (r: Result<(), CheckErrors>)
    ensures
        r is Ok && spec_program_fits(*p) ==> spec_well_typed(*p),
```

| Stage | Conclusion when it adds no error | Note |
| --- | --- | --- |
| `check_wf` | `wf_program` | T5 |
| `const_types` | `spec_consts_typed` | See below |
| `check_types` | `spec_function_typed`, `params == 0` | Needs every const typed, which `const_types` gives |
| `check_flow` | `spec_flow_ok` | `clean` handles `reported`: with no error and nothing reported yet, nothing was suppressed |
| `check_scope` | Nothing | Writes to its own list, then appended, so it can't remove an error from the others |
| `check_dims` | `spec_dims_ok` | |
| `cycles` | Every unranked node has `no_self_reach` | See §7.5 |

**Constants** are typed in rank order, so the invariant is staged: `const_stage` says everything before
the current position has a type, and nothing after it does. A const's init reads only lower-ranked
consts (`lemma_known_from_ranked`), whose types are already known. `lemma_type_mono` says a type once
derived stays valid as more consts get types.

`check_case`'s contract is direct: `r is Ok ==> spec_case_fits(*p, view_values(case.hosts@))`.

### 7.5 From "no cycle reported" to valid ranks

This is the part of T4 about graphs, in `proofs/ranks.rs`. `spec_ranks_valid` needs every id to have a
rank, and `cycles` only reports that it found no cycle.

1. **Ranks that get assigned are correct** (`lemma_node_ranks`). A rank is given only when all successors
   already have lower ones (`valid_ranks`), and it stays below the number of ranked nodes
   (`rank_bounded`), so it can't overflow.
2. **The loop reaches a fixpoint** (`rank_fixpoint`). Each round that changes something ranks at least one
   more node, and there are `n + 1` rounds, so the last one changes nothing. At a fixpoint an unranked node
   has an unranked successor (`lemma_unranked_successor`).
3. **An unranked node gives a cycle** (`lemma_path`, `lemma_all_ranked`). Keep following unranked successors.
   There are finitely many nodes, so a walk that keeps going must repeat one, and that node reaches itself.
4. **`cycles` would have reported it.** Its contract says: with an empty result, every unranked node `x`
   has `no_self_reach`, witnessed by a closed set of nodes that contains everything `x` reaches and not `x`
   itself (`reach_closed`). This contradicts step 3, so every node has a rank.
5. **Every edge in the program is in the graph** (`lemma_expr_ranked`, `lemma_stmt_ranked`, `lemma_refs_below_node`).
   `spec_refs_expr` finds each call, const read and density read. With wf, none is dropped as out of range,
   so each is a graph edge and the target is ranked lower: this is `spec_ranks_valid` (`lemma_ranks_valid`).

The hypothesis `spec_program_fits` is needed here. `graph` numbers nodes with `add_or_max`, which
saturates, and Verus knows only that each table's length fits `usize`, not that their sum does. It is
physically always true, since every element takes memory, so it stays a named hypothesis rather than an
error kind no test could reach. T1 doesn't need it.

### 7.6 Putting it together

`theorem_checked_run` joins everything for one run. Its `requires` are exactly what the exec contracts
ensure:

```rust
pub proof fn theorem_checked_run<E: Externs>(p: Program, case: Case, ext: E, fuel: u64, out: CaseResult, ext_ok: bool)
    requires
        spec_well_typed(p),                                                    // check
        spec_case_fits(p, view_values(case.hosts@)),                           // check_case
        ext_ok ==> extern_respects(p, ext),
        view_entries(out.entries@) == spec_eval_program(program_env(p, case, ext, fuel)),   // eval_program
        diagnostics_sound(out.failures@, program_env(p, case, ext, fuel)),                  // eval_program
```

Its conclusions are about `out`, the interpreter's actual result.

---

## 8. T5: two definitions of well-formed

Wf has a `spec fn` (`wf_program`, used by proofs) and an exec function (`check_wf`, used to reject programs
with a path). They could drift apart, so each function of `check/wf.rs` proves an *iff* with its spec twin:

```rust
fn wf_expr(p: &Program, nlocals: usize, e: &Expr, w: &mut W)
    ensures
        final(w).errs@.len() >= old(w).errs@.len(),
        (final(w).errs@.len() == old(w).errs@.len()) == swf::wf_expr(p, nlocals as nat, e),
```

At the top this becomes `(errs@.len() == 0) == swf::wf_program(p)`. T4 only uses one direction. The other
is what lets `tungsten-check`'s tests call `check_wf` as a stand-in for "`wf_program` holds", which replaced
a third, plain-Rust copy of the definition.

---

## 9. What the theorems still take on trust

The list of trusted items is in [VERUS.md](VERUS.md) §1. This table says what breaks if each is wrong:

| If this is wrong | Then |
| --- | --- |
| **`spec/` doesn't say what we meant** | Every theorem still holds, but about the wrong semantics. See below |
| **A float primitive isn't a function of its input bits** | T1's memo soundness can fail for real runs, along with what T1 carries over |
| **A float primitive is the wrong operation** (say, `add` subtracts) | Every theorem still holds, and every float result is wrong. Only tests catch this |
| **An extern isn't pure** | T1 can fail for real runs: the spec has one `spec_call` result, and the memo reuses it |
| **Verus, Z3, rustc or vstd has a bug** | Anything can fail. The versions are pinned |
| **The stack or allocator runs out** | The process dies. The theorems describe runs that finish |

**The closeness of spec and code is a real limitation.** The spec was written to mirror the interpreter,
so a mistake in understanding could be in both, and T1 wouldn't notice. Some things offset that, none
completely:

- T1 makes the hand-written `@expect` corpus evidence about the spec, since those tests run the
  interpreter, which is proven equal to it.
- T2 and T4 are stated against the spec, so a wrong spec tends to make them unprovable. T2 did this (§10).
- `SEMANTICS.md` states the same rules in prose, which gives a reviewer something to compare against.
- The differential tests compare against backends written separately, from the same rules.

**The theorems stop at this crate's boundary.** The lowering in `tungsten-check`, the optimizer and the
backends aren't covered. That is why the tests that seem redundant with the proofs stay:
`memo_off_is_identical`, `fuel_is_monotone` and `stuck_mutation` run the real primitives and externs on the
output of the real lowering, which the proofs either trust or don't cover.

### Checking the proofs can fail

A proof can pass for a bad reason, such as a contradictory hypothesis that makes everything true.
`scripts/verify-negative.sh` applies eight deliberate faults one at a time and requires verification to reject
each:

- a proof that asserts `false` (vacuity);
- `&&` that doesn't short-circuit in exec;
- `>` on `f64` compared the wrong way round;
- a memo that stores a const under the wrong id;
- `MIN / -1` returning `0`;
- a spec arm changed without its exec;
- a checker that accepts a missing return;
- `well_typed` forgetting `break` placement.

Run it by hand after changing specs or proofs.

---

## 10. What proving found

Working out T2 found a real bug. `eval_entry` checked the rank before the layout. Take a program, unchecked,
with an entry that has no computable layout, whose unit has no rank, and a const that runs out of fuel at
low fuel:

- At low fuel, the failing const takes the first branch of `spec_entry_cells`. With no layout, the entry is
  one `Stuck(Layout)` cell.
- At high fuel, the const succeeds, so the entry gets evaluated. The rank check came first, so the entry was
  `Stuck(Rank)`.

That is a finished, non-`OutOfFuel` result changing with fuel, so T2 was false. The fix was in the code and
the spec, not a weaker T2: now layout wins in both paths. It can't affect a checked program, because
`spec_dims_ok` means there is always a layout. The regression test is
`no_layout_is_stuck_layout_at_every_fuel` in `tungsten-check/src/tests/memo.rs`.

If a proof won't go through, this is the case to consider first. The rule is to fix a real bug in the code with
a regression test, and never to weaken a spec to match the code.

---

## 11. Working on the proofs

**Running them.** `scripts/verify.sh`, from the repo root ([VERUS.md](VERUS.md) §3). There are no `assume`s or
`admit`s, and the script refuses to run if any appear, or if the trusted count stops being 22.

**Reading order.** To audit the theorems: `spec_eval_program` at the bottom of `spec/eval.rs`, reading upward;
then `spec/ops.rs`; then `spec_well_typed_with` at the bottom of `spec/typing.rs`. To learn how a proof works:
`proofs/layout.rs` is short and self-contained; `proofs/fuel.rs` then shows the shape every induction here has;
`proofs/soundness.rs` is the same shape with more to carry.

**Changing the semantics.** Change `spec/` and `exec/` together and let Verus point at what broke, which is
usually one arm each in `proofs/fuel.rs` and `proofs/soundness.rs`. `README.md`'s "When you change something"
lists every place a new `Expr` or `Stmt` variant touches.

**When a proof gets slow.** Three techniques keep this one fast, and the default rlimit is enough everywhere:

- **Opaque definitions.** `memo_inv`, `env_ok`, `const_stage` and `spec_decode` are `#[verifier::opaque]`, so
  their quantifiers or arithmetic reach only the proofs that `reveal` them.
- **One function per arm.** When a `match` puts too many obligations in one query, each big arm becomes its own
  function, taking the whole node as `Ghost` for its measure (`eval_binary`, `lemma_expr_call`).
- **One spec arm per variant.** `A { args, .. } | B { args, .. }` in a `spec fn` defeats trigger selection.

[VERUS.md](VERUS.md) §6.2 lists the symptoms and fixes.
