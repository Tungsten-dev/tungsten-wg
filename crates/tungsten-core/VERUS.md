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
backend and the interpreter disagree, the interpreter must be right. Testing gets you far, but
the long-term plan is to *prove* things about the checker and the interpreter:

- the interpreter refines a mathematical spec of the semantics;
- **type soundness**: a program the checker accepts never gets `Stuck`;
- fuel monotonicity, memo soundness, and the grid layout lemma.

Verus lets us write the checker and interpreter as ordinary Rust, put proofs next to them, and
have the SMT solver Z3 check those proofs. The same code is used for real (by `tungsten-check`
and the facade) and for the proofs, so there's no gap between "the thing we proved" and "the
thing we run".

### What is verified today

This is the **first phase**. There are no functional proofs yet. What Verus does prove, for
every function in the crate, is:

- **no out-of-bounds indexing** (every `v[i]` has `i < v.len()`),
- **no arithmetic overflow** (every `+`, `*`, … on machine integers stays in range),
- **termination** (every recursive function and every loop has a measure that decreases).

So the checker and interpreter can't panic or loop forever on *any* input. That's useful on its
own (the property tests throw random programs at them), and it means the proof phase only has
to *add* specs, `ensures`, invariants and lemmas. It should never need to restructure the code.

### What is not verified

- Anything about **what** the functions compute (no `ensures` beyond safety).
- `prim.rs`: the arithmetic primitives are trusted (§5.6).
- The `Externs` implementation the caller passes in.
- The hand-written `Clone` impls outside `verus!{}` (they just delegate to verified functions).
- Everything outside this crate: the lowering in `tungsten-check`, the backends.

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
| `spec fn` | `wf::wf_program` | No. A mathematical definition, only for proofs |
| exec `fn` | `check::check`, `exec::eval_program` | Yes. Verus checks its safety and termination |
| `external_body` exec fn | everything in `prim.rs` | Yes. Verus trusts it without looking inside |

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
   `external_body` and `#[verifier::external]`, and refuses to continue if any appear outside
   `prim.rs`. So nothing can quietly be "trusted without proof".
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

A green run reports `0 errors` (the last count was 313 verified). It also prints some warnings
that are expected:

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

## 4. The rules for this phase

Every rule below is there for a reason, given next to it. Breaking one usually means either
Verus rejects the code, or the proof phase gets much harder.

**1. `verify.sh` must pass.** Verus proves bounds, absence of overflow and termination. Exec
functions have no functional `ensures`.

> *Why no `ensures` yet?* Writing `ensures` means writing the spec they refer to, and that spec
> is the next phase's job. Partial `ensures` now would have to be rewritten later.

**2. No trusted gaps outside `prim.rs`.** No lemmas, no `ensures`, no `assume`, no `admit`, no
`external_body` (except in `prim.rs`). Loop invariants are allowed, but only for safety (bounds,
no overflow). A `requires` that only rules out overflow is fine (there is exactly one:
`check/mod.rs::list_class`).

> *Why?* An `assume` or `external_body` is a hole in the proof. This phase should be
> *completely* verified for the properties it claims. The grep in `verify.sh` enforces this
> mechanically.

**3. Each exec function is shaped like the spec it will get.** Same name and arguments as the
future `spec fn`, plus `memo: &mut Memo` where needed. The exception is loops (rule 7), which are
Rust loops even though the spec will be recursive.

> *Why?* The refinement proof then goes function by function: "`eval_expr` computes
> `spec_eval_expr`". If the exec code were organized differently from the spec, each proof would
> first have to bridge the structural difference.

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
> value instead of a panic, and Verus proves `get` itself is safe. Why *not* use `get` for
> children: see §5.3.

**6. Every program-level operation goes through `prim.rs`.** Every integer operation the program
does (`+ - * /` with wrapping, shifts with masking, `i8` promotion), every float operation and
comparison, and every cast.

> *Why?* These are the operations whose meaning has to be pinned down exactly (Java semantics).
> Putting them in one file of tiny functions means the trusted base is small and easy to audit,
> and the proof phase can give each one an `ensures` against a mathematical definition. Ordinary
> index arithmetic in the interpreter is *not* program-level and stays checked Rust arithmetic.

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

### 5.2 Well-formedness three times

Wf ("every id is in range") is defined:

1. as `spec fn wf_program` in `src/wf.rs`, the definition proofs will use;
2. as `check_wf` in `src/check/wf.rs`, executable, reporting errors with paths;
3. as a plain-Rust mirror in `tungsten-check/src/tests/wf_mirror.rs`, for tests.

A `spec fn` can't be run, so (2) exists to actually check programs, and the proof phase will
prove `check_wf(p).is_empty() ⟺ wf_program(p)`. Until then, a property test checks that (2)
and (3) agree on lowered and randomly broken programs. That test has passed, and deleting the
mirror is planned but waits for approval.

Spec details: each recursive `spec fn` has `decreases` on its structural argument. Recursion
through a `Vec` child under a guarded `forall` is accepted directly (vstd's
`axiom_vec_index_decreases`), so no `decreases_by` helpers were needed. `wf_stmts` isn't
recursive itself; it quantifies over the recursive `wf_stmt`, so no recursion mixes `Seq` and
datatype measures.

### 5.3 `get` for ids, direct indexing for children

`util::get` has no `ensures`. That's fine for ids: we only need "it returns something or
nothing". But for **termination**, Verus has to know that when we recurse into `args[i]`, that
element is part of `args`, which is part of the expression `e`, so it is smaller than `e`. If the
element came out of `get`, Verus knows nothing about it, and the termination proof fails.

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
  build_input 5  >  eval_unit_ctx 4  >  eval_cell 3  >  eval_call, eval_const 2
                 >  eval_stmts, eval_stmt 1  >  eval_expr, eval_args, read_density 0
  ```
- **Inline comparisons.** `if r < rank { eval_call(env, r, …) }` has to be written in the
  calling function. A helper `fn lower_rank(…) -> Option<usize>` would look nicer, but without
  an `ensures` (forbidden in this phase) Verus doesn't know its result is below `rank`.
  `rank_or(target, current)` only maps *none* to `current`; the `<` is still in the caller.
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
future spec will define them recursively (rule 3's exception). Recursion would put one stack
frame per iteration on the Rust stack, and at `DEFAULT_FUEL = 2^20` that overflows.

### 5.6 `prim.rs`: the trusted base

`prim.rs` is the one file with `#[verifier::external_body]`. Each function is a one-liner on
bits. In this phase they have no `ensures`, so Verus treats each result as an arbitrary value of
the right type, which is all the safety proofs need. The proof phase will add an `ensures`
equating each one to an uninterpreted `spec fn` (and the spec semantics will be written in terms
of those).

Keeping it small and flat matters: this is code we *trust*, so it should be easy to read and
compare with Java's rules, and it has its own edge-case unit tests.

### 5.7 Generic externs, no trait objects

The extern hook is `trait Externs` with a single `call`, and the interpreter is generic over
`E: Externs`. `&dyn Externs` would be more convenient, but Verus doesn't support trait objects.
The trait has no spec in this phase. Later it gets an uninterpreted spec, so the proofs hold for
*any* extern implementation (as long as it is pure).

### 5.8 The `Env` bundle and `InputBuild`

The read-only arguments (`p`, `ranks`, `case`, `ext`, `fuel`) are bundled in `Env<'_, E>`, so
signatures stay readable and match the future spec's arguments. `build_input` returns
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
| Termination fails when recursing into an element you got from `get` | `get` has no `ensures` | Index directly under `i < len` |
| Termination fails after `let r = helper(rank)?; eval_call(…, r, …)` | The helper's result isn't known to be `< rank` | Write `if r < rank { … }` inline |
| Termination fails for `eval_stmt` → `eval_expr` | Verus can't compare `Stmt` and `Expr` sizes | Give the callee a lower layer in the `decreases` tuple |
| "possible cycle" on `#[derive(Clone)]` | Derived `Clone` on a recursive type | Hand-written `impl Clone` outside `verus!{}` delegating to a `clone_*` fn |
| A call to `.clone()` / an iterator method is rejected | Verus doesn't know the function | Use the `clone_*` fn, or an explicit loop |
| Overflow error on `a + b` with lengths | Verus doesn't know the sum fits | Checked arithmetic, `add_or_max`, or (only for overflow) a `requires` |

### 6.3 Lint noise

- `inconsistent_fields` warnings are expected (§3).
- Clippy complains about the Verus style (`i = i + 1`, `&Vec`, explicit `match` on `Option`, …).
  The allowed lints are listed at the top of `lib.rs`. Don't "fix" them: the idiomatic forms
  usually don't verify.

### 6.4 Keeping the twins in sync

`wf.rs`, `check/wf.rs` and `wf_mirror.rs` describe the same thing. Changing one without the
others won't fail `verify.sh`; only the agreement property test catches it. When you add an IR
variant, update all three (the README's "When you change something" lists every place).

---

## 7. What the proof phase will add

The code is laid out so that the next phase only adds:

- `spec/`: a mathematical semantics (`spec_eval_expr`, `spec_eval_entry_cell`, …) with the same
  shape as the exec functions, and `well_typed`;
- `ensures` on every `prim.rs` function, equating it to an uninterpreted `spec fn`;
- `ensures` on the exec functions: each computes its spec counterpart (refinement);
- `proofs/`:
  - **type soundness**: `check` and `check_case` accept ⇒ no `Stuck`;
  - **fuel monotonicity**, stated per cell: if a cell isn't `OutOfFuel` at fuel `f`, it is the
    same at every `f2 ≥ f`. (Canonical error choice keeps this valid: a result that isn't
    `OutOfFuel` means no `OutOfFuel` came earlier in canonical order.);
  - **memo soundness**: memo on and memo off give the same result;
  - **wf agreement**: `check_wf(p)` is empty exactly when `wf_program(p)`;
  - **layout**: `decode(flat(c, d), d) == c` for every cell in the grid.

Until then, most of these are property tests in `tungsten-check/src/tests/` (type soundness by
mutation, memo on/off, fuel, wf agreement) or unit tests (layout round trip in `layout.rs`).
