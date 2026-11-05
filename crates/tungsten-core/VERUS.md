# Verus in tungsten-core: choices, rules and pitfalls

`tungsten-core` is written to be verified with [Verus](https://github.com/verus-lang/verus).
That shapes a lot of the code in ways that look odd if you don't know why. This document
collects every Verus-related decision in one place: what is verified today, how to run it, the
rules the code follows, why each rule exists, and the problems we ran into.

What the code *means* is in [SEMANTICS.md](SEMANTICS.md). The checker and interpreter themselves
are in [CHECK.md](CHECK.md) and [EXEC.md](EXEC.md).

---

## 1. Why Verus at all

The goal of the testing work (TESTING.md) is a **trustworthy reference interpreter**: when a
backend and the interpreter disagree, the interpreter must be right. Testing gets you far, and
proving gets further, so this crate does both.

Verus lets us write the checker and interpreter as ordinary Rust, put proofs next to them, and
have the SMT solver Z3 check those proofs. The same code is used for real (by `tungsten-check`
and the facade) and for the proofs, so there's no gap between "the thing we proved" and "the
thing we run".

### What is verified

Everything exec is free of out-of-bounds indexing and overflow, and terminates. On top of that:

| # | Theorem | Where | Holds for |
| --- | --- | --- | --- |
| T1 | **Refinement with memo soundness.** `eval_program`'s cells equal `spec_eval_program`, memo on or off, at every fuel. Const failures are exact, each input failure is correct and listed once | `ensures` on `exec::eval_program` | Every program |
| T2 | **Fuel monotonicity.** A cell that isn't `OutOfFuel` at `f` is the same at every `f2 ≥ f` | `proofs/fuel.rs`, `theorem_fuel_monotone` | Every program |
| T3 | **Layout.** `decode` and `flat` are inverses on in-range cells | `proofs/layout.rs` | Grids with dims ≥ 1 that fit `usize` |
| T4 | **Type soundness.** No `Stuck` except the two only an extern breaking its declaration can cause; every `Ok` has its static type; each entry has `len(dims)` cells. With an extern keeping its declaration, no `Stuck` at all | `ensures` on `check` and `check_case`, `proofs/soundness.rs`, `theorem_checked_run` | Programs passing `check`, cases passing `check_case` |
| T5 | **Wf agreement.** `check_wf(p)` is empty exactly when `wf_program(p)` | `ensures` on `check_wf` | Every program |

`scripts/verify-negative.sh` applies deliberate faults and requires verification to fail, as a check
that the proofs aren't vacuous.

### What is trusted, and what is not verified

- Verus, Z3, rustc, and vstd's specifications of the standard library.
- `prim_float.rs`: 22 float primitives. Their `ensures` say each is a function of its input bits,
  which canonicalizing `NaN` results makes true. IEEE-754 correctness is tested, not proven.
- `Externs` implementations: the trait's `ensures` is a contract nothing checks. They must be pure,
  including the bits of any `NaN` they return.
- `spec/`: reviewed, not proven. It should say what SEMANTICS.md says.
- Stack overflow and allocation failure aren't modeled.
- T4's checker half has the hypothesis `spec_program_fits`: the node table fits `usize`. It is
  physically always true, since every element takes memory.

Not verified at all: the order in which failing inputs are discovered; that every failing input is
listed; that the checker doesn't reject good programs (other than T5); `check/scope.rs`, which no
theorem depends on; the hand-written `Clone` impls outside `verus!{}`, which verified code doesn't
call; and everything outside this crate, namely the lowering in `tungsten-check` and the backends.

---

## 2. How the code is built

Everything lives inside `verus! { … }` blocks. The only dependency is `vstd`, Verus's standard
library.

- A **normal `cargo build`** doesn't need Verus. The `verus!` macro (from `vstd`) erases the
  ghost parts (`spec fn`s, `requires`, `decreases`, invariants) and leaves plain Rust. That's how
  `tungsten-check` and the facade use the crate.
- **`scripts/verify.sh`** runs actual verification (§3).

Three kinds of functions appear:

| Kind | Example | Exists at run time? |
| --- | --- | --- |
| `spec fn` | `wf::wf_program`, `spec::eval::spec_eval_expr` | No. A mathematical definition, only for proofs |
| `proof fn` | `proofs::fuel::theorem_fuel_monotone` | No. A lemma Verus checks |
| exec `fn` | `check::check`, `exec::eval_program` | Yes. Verus checks it against its `ensures` |
| `external_body` exec fn | everything in `prim_float.rs` | Yes. Verus trusts it without looking inside |

### Reading Verus syntax

| Syntax | Meaning |
| --- | --- |
| `pub open spec fn` | A mathematical definition. `open` means other modules can see its body |
| `&&& a &&& b` | `a && b`, written as a bulleted list |
| `forall\|k: int\| #![trigger xs[k]] 0 <= k < xs.len() ==> P(xs[k])` | "P holds for every element of `xs`". The trigger is a hint for the solver; skip it when reading |
| `decreases e` | Termination: each recursive call (or loop iteration) makes `e` smaller |
| `decreases rank, 2int, args` | A lexicographic measure: compared first by `rank`, then `2`, then `args` |
| `invariant i <= v.len()` | A loop invariant: true before and after every iteration |
| `requires P` | The caller must make sure `P` holds |
| `xs@` | A `Vec` viewed as a mathematical sequence (`Seq`) |
| `nat`, `int` | Mathematical integers, which can't overflow |

---

## 3. Running verification

```bash
scripts/verify.sh                         # from the repo root
VERUS_Z3_PATH=/path/to/z3 scripts/verify.sh
scripts/verify.sh -- <verus args>          # arguments after -- go to Verus
```

The script, step by step:

1. **Pin check.** The `vstd` version in the root `Cargo.toml` must be the one this Verus release
   was published with. Otherwise it stops.
2. **Trusted-gap guard.** It greps `crates/tungsten-core/src` for `admit(`, `assume(`,
   `external_body`, `verifier::external`, `assume_specification`, `axiom fn`,
   `exec_allows_no_decreases_clause` and `assume_termination`, and refuses to continue if any appear
   outside `prim_float.rs`. `uninterp` is allowed only in `spec/prim.rs`, and it counts exactly 22
   of each. So nothing can quietly be "trusted without proof".
3. **Download.** The first time, it downloads the pinned Verus release into
   `target/verus/<tag>/` and checks its sha256.
4. **Toolchain.** It installs the Rust toolchain that release needs with `rustup` and selects it
   with `RUSTUP_TOOLCHAIN`.
5. **Z3 check.** If the bundled `z3` doesn't run (§6.1), it stops and tells you to set
   `VERUS_Z3_PATH`.
6. **Verify:** `cargo verus verify -p tungsten-core`.

### Pins

| What | Value |
| --- | --- |
| Verus release (`VERUS_TAG`) | `0.2026.10.26.d17fbd0` |
| `vstd` (root `Cargo.toml`, `=`-pinned) | `0.0.0-2026-10-25-0158` |
| Rust toolchain | `1.99.1` |
| Z3 | `4.16.0` |

The pairing of Verus, `vstd` and the toolchain comes from Verus's own
`source/cargo-verus/toolchain-manifests/<tag>.toml` (and `version.json` in the release zip).
**Bump them together**: `VERUS_TAG`, `VSTD_VERSION`, `VERUS_TOOLCHAIN_VERSION`, the four
checksums in `verify.sh`, and the `vstd` pin. The script's pin check enforces the `vstd` part.

### Expected output

A green run reports `0 errors` (the last count was 531 verified, in about 20 seconds on a 16-core
machine, against a budget of 10 minutes). It also prints some warnings that are expected:

- **`inconsistent_fields`** on `Expr` (`index`, `op`), `Failure` (`id`) and `CheckErrorKind`
  (`op`, `expected`, `found`, …). Verus warns when the same field name has different types in
  different enum variants. The only effect is that the `->field` shorthand syntax isn't
  available for those fields, which we don't use.
- `autoderive_clone_without_spec` is silenced in `lib.rs` (§5.9).

### Why it isn't in CI

Verus runs locally only; gating CI on it was decided against. It needs a large download, a
specific Rust toolchain and a Z3 that runs on the machine. `cargo test` is what CI runs;
`verify.sh` is the acceptance test you run yourself after changing this crate.

---

## 4. The rules

Every rule below is there for a reason, given next to it. Breaking one usually means either
Verus rejects the code, or the proofs get much harder.

**1. `verify.sh` must pass.** It proves bounds, absence of overflow, termination and the theorems of
§1, so it is the acceptance test for this crate.

**2. No trusted gaps outside `prim_float.rs`.** No `assume`, `admit` or `external_body` anywhere else.
The trusted base is a decision: `verify.sh` counts it, so growing it means changing that check on purpose.

> *Why?* An `assume` or `external_body` is a hole in the proof. The grep in `verify.sh` enforces
> this mechanically.

**Keep the time budget.** A full run stays under 10 minutes with the pinned Z3. Functions keep the
default rlimit: when a proof gets slow, split it, or make a definition opaque (`memo_inv`,
`spec_decode`, `env_ok`, `const_stage`) so that its quantifiers stay out of proofs that don't need
them. A raised rlimit needs a comment saying why. There are none today.

**Edits to exec code must preserve behavior.** `cargo test --workspace` and the corpus stay green,
and a real bug found by a proof gets fixed in the code with a regression test, never by weakening a
spec.

**3. Each exec function is shaped like its spec.** Same name and arguments as its `spec fn`, plus
`memo: &mut Memo` where needed. The exception is loops (rule 7), which are Rust loops even though
the spec is recursive.

> *Why?* The refinement proof goes function by function: "`eval_expr` computes `spec_eval_expr`".
> If the exec code were organized differently from the spec, each proof would first have to bridge
> the structural difference. Exec loops build lists, so the spec's list functions take an
> accumulator: the invariant is then one equality, `spec(…, 0, empty) == spec(…, i, so_far)`.

**4. Functional style.** No closures, no iterators, no `HashMap`, no function pointers, no
generics except `E: Externs`. Plain `while` loops with an index, and `match` instead of
`Option` combinators like `map` or `and_then`.

> *Why?* Verus supports closures and iterators only partly, and every one of them needs its own
> specification to reason about. An explicit loop with an invariant is easy for Verus and for the
> later proofs. This is also why `lib.rs` allows a long list of Clippy lints (`i = i + 1`,
> explicit matches, `&Vec`, …): the "idiomatic" fix would undo this rule.

**5. Ids go through `util::get`.** Every read of a `Vec` at an index that comes from the program
(an id) goes through `util::get(&v, i) -> Option<&T>` (writes: `util::set`). A miss becomes
`Stuck(BadIndex)` in the interpreter and a wf error in the checker. Loops over a node's own
children index directly (`&v[i]` under `i < v.len()`). Index arithmetic is checked.

> *Why?* The checker might not have run, so an id can point anywhere. `get` turns that into a
> value instead of a panic. Why *not* use `get` for children: see §5.3.

**6. Every program-level operation goes through `prim.rs`.** Every integer operation the program
does (`+ - * /` with wrapping, shifts with masking, `i8` promotion), every float operation and
comparison, and every cast.

> *Why?* These are the operations whose meaning has to be pinned down exactly (Java semantics).
> Putting them in tiny functions means each has an `ensures` against a definition, and the trusted
> base (`prim_float.rs`) is small and easy to audit. Ordinary index arithmetic in the interpreter is
> *not* program-level and stays checked Rust arithmetic.

**7. Everything recursive has a `decreases`.** Every recursive function and every loop.
A loop whose body recurses into a child needs `#[verifier::loop_isolation(false)]`. A rank
comparison has to be written inline in the caller (`if r < rank`), not hidden in a helper.

> *Why?* See §5.3 and §5.4.

**8. Recursive types get `clone_*` functions.** `Type`, `Expr`, `FieldInit`, `Stmt`, `Value`
and `Case` each have a `clone_*` exec function with a `decreases`. The `impl Clone` for them is
written *outside* `verus!{}` and delegates to it. Verified code calls `clone_*` directly, never
`.clone()`.

> *Why?* See §5.9.

---

## 5. Design decisions, and why

### 5.1 The core IR exists because of Verus

`tungsten-ir`'s `OptimizedProgram` is built for compiling: generational `Ref`s into arenas,
names everywhere, convenience variants like `ConstructExtern`, `f32`/`f64` values. Each of those
makes proofs harder. So the core IR is a second, much plainer form of the same program:

| Choice | Why |
| --- | --- |
| **Dense `usize` ids** into `Vec`s instead of arena `Ref`s | "In bounds" is just `id < v.len()`. A stale generational ref has no simple statement |
| **No names**, no `String` anywhere | Strings are awkward in Verus and irrelevant to meaning. Names live in plain-Rust side tables (`tungsten-check`'s `Registry`, `LowerMap`) |
| **Floats as bits** (`u32`/`u64`) | Equality is exact, including `NaN`, so the derived `PartialEq` is lawful. Reasoning about floats needs no float axioms in the IR itself; only `prim.rs` touches real floats |
| **Aggressive normalization**: field selectors resolved to positions, one `Make*` per shape, constant expressions folded into `Expr`, `Ref` (identity) dropped, parameters are simply locals `0..k` | Every variant adds a case to every future proof. Fewer variants, fewer cases |
| **Annotations kept** (extern signatures at call sites, field types) and checked by the checker | Otherwise the lowering would have to check and drop them, and the lowering is unverified |
| **Declaration tables inside `Program`** (`externs`, `records`) | The checker and interpreter need only `&Program`, so specs range over one object |

### 5.2 Well-formedness twice

Wf ("every id is in range") is defined:

1. as `spec fn wf_program` in `src/wf.rs`, the definition proofs use;
2. as `check_wf` in `src/check/wf.rs`, executable, reporting errors with paths.

A `spec fn` can't be run, so (2) exists to actually check programs. T5 proves
`check_wf(p).is_empty() ⟺ wf_program(p)`, which replaced a plain-Rust mirror and its agreement test.

Spec details: each recursive `spec fn` has `decreases` on its structural argument. Recursion
through a `Vec` child under a guarded `forall` is accepted directly (vstd's
`axiom_vec_index_decreases`), so no `decreases_by` helpers were needed. `wf_stmts` isn't
recursive itself; it quantifies over the recursive `wf_stmt`, so no recursion mixes `Seq` and
datatype measures.

### 5.3 `get` for ids, direct indexing for children

For **termination**, Verus has to know that when we recurse into `args[i]`, that element is part of
`args`, which is part of the expression `e`, so it is smaller than `e`. Going through `get` hides
that from the termination check.

So loops over a node's own children index directly, under `i < len`:

```rust
while i < args.len() invariant i <= args.len(), decreases args.len() - i {
    eval_expr(…, &args[i], …);   // Verus knows args[i] is a part of e
    i = i + 1;
}
```

and such loops need **`#[verifier::loop_isolation(false)]`** on the function. By default Verus
verifies a loop body in isolation, knowing only the loop invariant, and then it can't see that
`args` is a field of the function's argument `e`. Turning isolation off lets the termination check
see the connection.

### 5.4 The termination measure: `(rank, layer, structure)`

Termination of the interpreter is the hardest part. See EXEC.md §3 for the full picture. The
Verus-specific parts:

- **Ranks.** Every function, constant, input and unit has a rank from `check::acyclic::ranks`
  (CHECK.md §7). Calls into another item check `target < current` and pass the target's rank
  down. The `decreases` clause starts with `rank`.
- **Layers.** Verus requires every position of a `decreases` tuple to have a compatible type
  across all the functions in one recursion group, and it **can't compare the height of a `Stmt`
  with the height of an `Expr`**. The original plan was `(rank, structural size, loop counter)`,
  but `eval_stmt` calling `eval_expr` at the same rank then needs `Expr < Stmt`, which Verus
  can't decide. The fix is a constant second component, the layer: `eval_stmt` is layer 1 and
  `eval_expr` layer 0, so that step decreases on the layer and the structures are never compared:

  ```
  build_input 6  >  eval_unit_ctx 5  >  eval_cells 4  >  eval_cell 3  >  eval_call, eval_const 2
                 >  eval_stmts, eval_stmt 1  >  eval_expr, eval_args, read_density 0
  ```
- **Inline comparisons.** `if r < rank { eval_call(env, r, …) }` is written in the calling function,
  like the spec. `rank_or(target, current)` only maps *none* to `current`; the `<` is still in the
  caller. So a call that gets through had a rank, which `eval_const` and `build_input` require: the
  memo's entry for an id is the spec's result at that id's own rank.
- **The spec's measure** is `(rank, layer, structure, loop count, list index)`, with one layer per
  spec function above statements. Recursion over a loop's iterations and a list's index needs the two
  extra places. Every position must have one type across the group, so the layers above expressions
  use `env.p` as a constant third place.
- **Loops are their own measure.** `While` is `decreases fuel - iters`, `Repeat` is
  `decreases count - k`, child loops `decreases len - i`.

*Why ranks rather than a global depth counter?* A depth counter (start high, decrease on every
call) was the first plan. But the memo stores a constant or buffer computed at one depth and
reuses it at another. The unmemoized spec would compute it again at the new depth, where it might
run out, so the memo would change results, and proving memo soundness would need facts from the
checker. Ranks depend only on the program, so a cached result is always the same.

*Why not prove acyclicity and drop the run-time check?* That would make the interpreter depend
on the checker. Keeping the check means the interpreter terminates on **any** program, checked or
not, which is what the property tests and the "never trust the checker" principle need.

### 5.5 Fuel per `While` execution, and loops as loops

Each execution of a `While` may run at most `fuel` iterations (SEMANTICS.md §7.3). A shared fuel
budget would make a result depend on how much work happened before, including work the memo
skipped. Per-loop fuel keeps results a function of `(program, case, fuel)`.

The interpreter implements `While` and `Repeat` as Rust loops, not recursion, even though the
spec defines them recursively (rule 3's exception). Recursion would put one stack
frame per iteration on the Rust stack, and at `DEFAULT_FUEL = 2^20` that overflows.

### 5.6 Integers verified, floats trusted

`prim.rs` is verified against definitions in `spec/prim.rs`. With uninterpreted integers the theorems
would hold for any integer semantics, and say nothing about Java's wrapping, shift-mask and `i8`
rules, which are what backends keep getting wrong. vstd has no spec for `wrapping_div` or
`wrapping_neg`, so those bodies use `checked_div` and `wrapping_sub`.

`prim_float.rs` is the one file with `#[verifier::external_body]`: 22 functions whose `ensures`
equates each to an `uninterp spec fn` in `spec/prim.rs`. Each is a one-liner on bits.

That `ensures` claims a float operation is a *function* of its input bits. For a `NaN` result, Rust
doesn't promise that: sign and payload are non-deterministic, and const-folding can differ from run
time. Memo soundness would rest on the false claim. So arithmetic and float-to-float conversions
return a canonical `NaN`, which makes it true. The cost is a corner that can differ from a backend,
because a `NaN`'s sign is observable through `copysign` (SEMANTICS.md §6.2).

Keeping this file small and flat matters: this is code we *trust*, so it should be easy to read and
compare with Java's rules, and it has its own edge-case unit tests.

### 5.7 Generic externs, no trait objects

The extern hook is `trait Externs` with a single `call`, and the interpreter is generic over
`E: Externs`. `&dyn Externs` would be more convenient, but Verus doesn't support trait objects.
The trait has a `spec fn spec_call`, and `call` ensures its result is that function's: so the proofs
hold for *any* implementation, as long as it is pure. Implementations are plain Rust outside
`verus!{}`, where ghost items are erased, so they don't define `spec_call` and nothing checks them.

### 5.7a Values in the spec

A `spec fn` can't build a `Vec`: vstd only has its view. So what the semantics constructs is a `Seq`
twin in `spec/value.rs`, mainly `SValue`, with views such as `view_value`, and contracts read
`view_result(r) == spec_eval_expr(…)`.

### 5.8 The `Env` bundle and `InputBuild`

The read-only arguments (`p`, `ranks`, `case`, `ext`, `fuel`) are bundled in `Env<'_, E>`, so
signatures stay readable and match the spec's arguments, whose `SEnv` is its view. `build_input` returns
`InputBuild::{Memoized, Fresh(result)}`: with the memo on, the buffer stays in `memo.inputs` and
the reader looks it up there, so a cached buffer is read in place instead of being cloned on
every read.

### 5.9 `Clone` outside `verus!{}`

The pinned Verus rejects a **derived** `Clone` on a type that contains itself (through `Box` or
`Vec`), as a possibly non-terminating cycle. So `Type`, `Expr`, `FieldInit`, `Stmt`, `Value`,
`Case`, `CheckErrorKind`, `CorePath` and `CheckError` have hand-written `impl Clone` blocks
*after* the `verus!{}` block closes. The IR and value ones delegate to `clone_*` exec functions
inside `verus!{}`, which have `decreases` and are verified to terminate.

Verified code never calls `.clone()` on these types, because `.clone()` is a function Verus
doesn't know (the impl is outside `verus!{}`). It calls `clone_expr`, `clone_value`, and so on.

All the other types derive `Clone`, `Debug`, `PartialEq`, `Eq` normally inside `verus!{}`. Those
derives have no spec, which triggers `autoderive_clone_without_spec`. That lint is silenced in
`lib.rs`, since no spec uses them.

### 5.10 Algorithms chosen for easy termination

Some algorithms in the checker are simpler than they could be, on purpose:

- **Ranks and cycles** (`check/acyclic.rs`) use bounded iterative fixpoints: "repeat at most
  `n + 1` rounds, stop early if nothing changed". No recursion over the graph, no work list, so
  termination is just the round counter.
- **Transitive constants** in scoping use the same bounded fixpoint.
- **Sorting errors** into walk order (`check/mod.rs::sort_errors`) is a stable selection sort
  (repeatedly remove the first minimal element). Quadratic, but error lists are short, and it
  needs no comparator closure.
- **Overflow-free counters**: where a sum of lengths could in theory overflow `usize`,
  `util::add_or_max` saturates instead. With real programs it never saturates; it just saves an
  overflow proof.
- **`&Vec<T>` instead of `&[T]`** in signatures: the termination measures use vstd's `Vec` index
  axioms (hence Clippy's `ptr_arg` is allowed).

### 5.11 `vstd` from crates.io, run with `cargo verus`

`vstd` comes from crates.io, `=`-pinned, and verification runs through `cargo verus` from the
downloaded release, rather than depending on a git revision of Verus and calling the `verus`
binary directly. `cargo verus` needs no direct `verus_builtin*` pins. The release binary is downloaded and sha256-checked by `verify.sh`
instead of being required on `PATH`, so everyone runs the same version.

### 5.12 Re-export name

The crate is reachable as `tungsten_check::core_ir`, not `core`. Re-exporting it as `core` would
shadow `::core`, which derive macros expand to, and break unrelated code.

---

## 6. Known issues and pitfalls

### 6.1 The bundled Z3 needs a recent glibc

The Z3 shipped in the x86-linux Verus release is built against glibc 2.38. On older systems it
doesn't start, and `verify.sh` stops with a message. Build or download Z3 **4.16.0** for your
machine and point `VERUS_Z3_PATH` at it. Verus refuses any other Z3 version unless you pass
`-- -V no-solver-version-check`, which you shouldn't rely on: a different solver version can
make proofs pass or fail differently.

### 6.2 Things that make Verus reject code that "obviously" terminates

| Symptom | Cause | Fix |
| --- | --- | --- |
| Termination fails when recursing into `args[i]` inside a loop | Loop isolation hides that `args` belongs to the argument | `#[verifier::loop_isolation(false)]` on the function |
| Termination fails when recursing into an element you got from `get` | `get`'s result isn't a known part of the node | Index directly under `i < len` |
| Termination fails after `let r = helper(rank)?; eval_call(…, r, …)` | The helper's result isn't known to be `< rank` | Write `if r < rank { … }` inline |
| Termination fails on a field of a list element in a helper taking `Ghost<Expr>` | The chain back to the expression is too long to find | Make the helper's measure the list itself (`type_make_record`) |
| rlimit exceeded on a long `match` | Every arm's obligations are in one query | One function per arm, taking the whole node as `Ghost` with a lower last place in `decreases` |
| rlimit exceeded with a quantified invariant | Its quantifiers are visible everywhere | Make it `#[verifier::opaque]`, and `reveal` where it's used |
| `*memo` in `ensures` is rejected | This Verus wants the state named | `*final(memo)`, `*old(memo)` |
| Triggers miss with `A { args, .. } \| B { args, .. }` in a `spec fn` | `args` is a conditional over the two variants | One arm per variant |
| Termination fails for `eval_stmt` → `eval_expr` | Verus can't compare `Stmt` and `Expr` sizes | Give the callee a lower layer in the `decreases` tuple |
| "possible cycle" on `#[derive(Clone)]` | Derived `Clone` on a recursive type | Hand-written `impl Clone` outside `verus!{}` delegating to a `clone_*` fn |
| A call to `.clone()` / an iterator method is rejected | Verus doesn't know the function | Use the `clone_*` fn, or an explicit loop |
| Overflow error on `a + b` with lengths | Verus doesn't know the sum fits | Checked arithmetic, `add_or_max`, or (only for overflow) a `requires` |

### 6.3 Lint noise

- `inconsistent_fields` warnings are expected (§3).
- Clippy complains about the Verus style (`i = i + 1`, `&Vec`, explicit `match` on `Option`, …).
  The allowed lints are listed at the top of `lib.rs`. Don't "fix" them: the idiomatic forms
  usually don't verify.

---

## 7. How the proofs are laid out

- `spec/`: definitions only, which is what a reader has to audit. `value.rs` (`SValue` and views),
  `prim.rs`, `layout.rs`, `ops.rs` (operators), `eval.rs` (the semantics), `ranks.rs` (the graph and
  ranks), `typing.rs` (`well_typed`, flow).
- `proofs/`: lemmas. `value.rs`, `layout.rs` (T3), `fuel.rs` (T2), `typing.rs`, `ranks.rs` (no cycle
  reported means valid ranks), `soundness.rs` (T4).
- `ensures` stay on the exec functions, next to the code (T1, T5 and the checker's half of T4).

Properties also run as tests in `tungsten-check/src/tests/` (type soundness by mutation, memo on/off,
fuel) and `layout.rs`. They aren't redundant: they run the real primitives and externs, which the
proofs trust, on the output of the lowering, which the proofs don't cover.
