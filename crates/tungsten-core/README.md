# tungsten-core: a guide

This file explains what the crate is for, how its parts fit together, where to start reading,
and why it is built the way it is ([Design decisions](#design-decisions)). The lowering that
produces the core IR is described in [`../tungsten-check/README.md`](../tungsten-check/README.md).

## What this crate is for

Tungsten programs have a rich representation (`tungsten-ir`'s `OptimizedProgram`). It is
built for compiling: generational `Ref`s, arenas, names everywhere, convenience variants such
as `ConstructExtern`, and annotations the backends rely on.

That representation is awkward to *reason about*. The long-term plan is to write a checker
(and later an interpreter) and **prove** things about them with
[Verus](https://github.com/verus-lang/verus). Proofs get harder with every enum variant, every
string and every float. So this crate defines a second, much plainer form of the same program:
the **core IR**.

You never write the core IR by hand. `tungsten-check` produces it from an `OptimizedProgram`
(see its `README.md`). This crate holds:

1. the data types (`ir.rs`, `registry.rs`, `value.rs`),
2. a definition of what "well-formed" means for them (`wf.rs`),
3. the executable **checker** (`check/`) and **interpreter** (`exec/`), and
4. the trusted primitives they call (`prim.rs`).

In this phase there are no functional proofs: Verus proves bounds, absence of overflow and
termination of every exec fn. The code is shaped so the proof phase only *adds* `spec fn`s,
`ensures`, invariants and lemmas.

```
OptimizedProgram ──(tungsten-check::lower)──▶ core_ir::Program ──▶ check::check ──▶ exec::eval_program
   (tungsten-ir)                               (this crate)          (this crate)     (this crate)
```

## Further reading

| Document | Covers |
| --- | --- |
| [SEMANTICS.md](SEMANTICS.md) | What a core program means: values, operators, statements, constants, density inputs, errors |
| [CHECK.md](CHECK.md) | Every checker rule, what it accepts and why, with every error kind |
| [EXEC.md](EXEC.md) | The interpreter function by function: data, termination, memo, costs |
| [VERUS.md](VERUS.md) | What Verus verifies, how to run it, the rules the code follows and why, known pitfalls |

## The files

| File | What's in it | Read it when |
| --- | --- | --- |
| `src/ir.rs` | Every data type: ids, `Type`, `Lit`, `Expr`, `Stmt`, `Function`, `Unit`, `Program`, … | Always. Start here. |
| `src/registry.rs` | `ExternDecl` and `RecordDecl`: the signatures of extern functions and the field types of records | You want to know what `Program.externs` / `Program.records` hold |
| `src/wf.rs` | `spec fn wf_program` and its helpers: "every id points at something that exists" | You want to know what a lowered program is guaranteed to satisfy |
| `src/value.rs` | `Value`, `EvalError`, `StuckReason`, `Case`, `trait Externs`, `value_has_type` | You evaluate programs or write an `Externs` |
| `src/util.rs` | `get`/`set` (bounds-checked `Vec` access), small copies, `clone_*` for the recursive types, `type_eq` | You write exec code here |
| `src/prim.rs` | Trusted int/float/cast primitives on bits (Java semantics). The only `external_body` file | You want the exact semantics of an operator or cast |
| `src/layout.rs` | `len`, `flat`, `decode` (x fastest), overflow-checked | Grid indexing |
| `src/check/` | `check(p)`, `check_case(p, case)`: `wf.rs` (exec twin of `crate::wf`), `types.rs`, `flow.rs`, `scope.rs`, `acyclic.rs` (ranks, cycles), `error.rs` | You want to know what the checker accepts |
| `src/exec/` | `eval_program`, `Memo`, `CaseResult`: `unit.rs` (entries, inputs, cells, consts), `stmt.rs` (calls, loops), `expr.rs` (expressions, density reads, operators) | You want to know what a program computes |

`wf.rs` (the spec) stays at the crate root; `check/wf.rs` is its exec twin.

### The checker in one paragraph

`check` runs `check_wf` first and stops if it reports anything. Otherwise it collects every
error, each with a `CorePath` (item + steps), in walk order: consts, functions, units, inputs,
entries, then cycles, pre-order within an item. Typing computes `Option<Type>` per expression
and never cascades on `None`. Flow is conservative (loops never count). Scoping enforces the
unit/function lists. `acyclic::ranks` gives every function, const, input and unit a rank (or
`None` on/reaching a cycle) with a bounded fixpoint; the interpreter uses the same fn.

### The interpreter in one paragraph

`eval_program` evaluates every const in id order, then every entry cell in flat order. Before a
unit runs in a context, every input it declares is built over all its cells (eager). Results
are per cell; `Stuck` is a per-cell error that a checked program never hits. The error choice
is canonical (lowest const, lowest input, first cell, left to right), so backends may schedule
freely. Every call/read checks that the target's rank is below the caller's (`Stuck(Rank)`
otherwise), and the termination measure is `(rank, layer, structure)`: the layer orders the
fns at one rank (expr 0 < stmt 1 < call/const 2 < cell 3 < unit 4 < input 5), because Verus
can't compare `Stmt` and `Expr` heights in one recursion group. Fuel is per `While` execution;
`Repeat` is free.

## Reading `ir.rs`

Read it **bottom-up**: look at `Program` (near the end) first, then follow its fields.

### `Program` is a set of tables

```rust
pub struct Program {
    pub units: Vec<Unit>,           // compute units (density functions)
    pub functions: Vec<Function>,   // every function, main or helper
    pub consts: Vec<ConstDecl>,     // constant initializers
    pub hosts: Vec<HostDecl>,       // host inputs (values the caller passes in)
    pub tables: Vec<TableDecl>,     // permutation tables / noise samplers
    pub inputs: Vec<InputDecl>,     // density inputs (one unit reading another)
    pub entries: Vec<Entry>,        // what the caller actually evaluates
    pub externs: Vec<ExternDecl>,   // extern function signatures
    pub records: Vec<RecordDecl>,   // record (struct) layouts
}
```

Everything else refers into these tables by **index**. `FnId(3)` means `functions[3]`,
`HostId(0)` means `hosts[0]`, and so on. Every id is a newtype around `usize`, so the compiler
won't let you mix a `FnId` up with a `ConstId`.

`LocalId` is the odd one out: it indexes the *enclosing function's* `locals`, not a global
table.

### How this differs from `tungsten-ir`

If you know `tungsten-ir`, these are the differences that will trip you up:

| In `tungsten-ir` | In the core IR | Why |
| --- | --- | --- |
| `Ref<T>` into an `Arena<T>` (can be stale) | plain `usize` index into a `Vec` | Easy to state "in bounds" and reason about |
| Names on everything | **No names at all** | Verus shouldn't deal with `String`. Names live in `tungsten-check`'s `LowerMap` |
| `f32` / `f64` values | `u32` / `u64` bit patterns (`Lit::F64(bits)`, `F64x3`) | Equality is exact, NaN included; no float axioms needed |
| `Field { base, field: "x", … }` | `Field { base, sel: FieldSel::Vec3(Axis::X), ty }` | The field is resolved to a position, no string lookup |
| `Construct` / `ConstructExtern` | `MakeVec3`, `MakePos3`, `MakeRecord` | One variant per shape |
| `ConstExpr` (a separate type) | the ordinary `Expr` | One expression type to reason about |
| `UnaryOperator::Ref(_)` | gone | It's the identity |
| `Function.parameters` (a list of var refs) | `Function.params: usize` | Parameters are simply `locals[0..params]` |
| A unit's own `density_inputs` arena | a global `inputs` table, each with an `owner` unit | One id space for all inputs |
| `VariableType::Extern("Name")` | `Type::Record(RecordId)` | Resolved against the registry |

### Things that look redundant but are on purpose

Some fields seem unnecessary because they could be derived. They are kept so that the
checker can **verify** them rather than trust them:

- `ExternCall { params, ret, .. }`: the types the call was *annotated* with. The extern's real
  signature is in `externs[ext]`. The checker (`check/types.rs`) compares the two.
- `Field { ty, .. }`: the annotated field type.
- `Unit { helpers, tables, hosts, consts, inputs }` and `Function { hosts }`: scoping lists.
  They say which things a unit or function may use. The checker enforces them (`check/scope.rs`).
- `MakeRecord { fields }` stays in the order the fields were *written*, and may have
  duplicates or gaps. "Every field exactly once" is a checker rule.

The general rule: **if something can be wrong, it stays in the IR so the checker can catch
it.** The lowering doesn't quietly fix or drop it.

### Small things worth knowing

- `Dims` holds `i32`s and is not validated. "At least 1" is a checker rule.
- `ConstDecl` has no declared type. The checker derives it from `init`, in rank order.
- A constant's `init` is an ordinary `Expr`, but must not contain `Local`, `Call`, `Density`,
  and so on. `wf` already catches `Local` there (see below). The rest is for the checker.
- `Stmt::Repeat { count }` is a fixed iteration count. `Break` can appear anywhere
  syntactically; whether it is inside a loop is a checker rule.
- There's no type for "density input": you can only *read* one with `Expr::Density`. It is
  never a value you can store.
- Floats are bits, so the derived `Eq` is exact: NaN literals keep their payload, and `-0.0`
  and `0.0` are different literals.
- `BinOp` has the same 18 variants as `tungsten_ir::BinaryOperator`, in the same order.

### The `Clone` impls at the bottom

`Type`, `Expr`, `FieldInit` and `Stmt` (and `Value`, `Case`) are recursive. Verus rejects a
derived `Clone` on them, so their `Clone` impls are written **after** the `verus! { … }` block
closes and delegate to the `clone_*` exec fns in `util.rs` / `value.rs`, which have
`decreases`. Verified code calls the `clone_*` fns directly, never `.clone()`.

## Reading `wf.rs`

`wf` stands for *well-formed*. It answers one question: **does every index point at something
that exists?** It says nothing about types.

The functions are `spec fn`s. They exist only for Verus and are erased in a normal build, so
you can't call them from regular Rust. Read them as precise documentation:

- `wf_program(p)` is the top. It says every unit, function, constant, host, input, entry,
  extern and record is well-formed, plus `wf_input_links`.
- `wf_function(f)`: `params <= locals.len()`, all types wf, the body wf with
  `nlocals = locals.len()`.
- `wf_expr(p, nlocals, e)`: a big `match`, one arm per `Expr` variant, checking ids against
  their tables and recursing into children.
  Constants are checked with `nlocals = 0`, so any `Local` in a constant fails.
- `wf_type`: `Record(r)` needs `r < records.len()`, recursively through `Array`.
- `wf_sel`: `FieldSel::Record(r, i)` needs record `r` to have a field `i`.
- `wf_input_links`: input `i` says its owner is unit `u` **if and only if** unit `u` lists
  input `i`.

What `wf` deliberately does **not** check, because these are checker rules: anything
type-related, exactly-once record fields, scoping lists, acyclicity of calls and constants,
dims, definite assignment, `Break` placement and density-read legality.

Termination: each recursive `spec fn` has `decreases` on its structural argument. Recursion
through `Vec` children under a guarded `forall` is accepted as is (vstd's
`axiom_vec_index_decreases`), so no `decreases_by` helpers were needed. `wf_stmts` isn't
recursive itself; it quantifies over the recursive `wf_stmt`, so no recursion mixes `Seq` and
datatype measures.

`tungsten-check` promises that every program it returns satisfies `wf_program`. Since you can't
run a `spec fn`, its tests use a hand-written plain-Rust copy instead
(`crates/tungsten-check/src/tests/wf_mirror.rs`). **If you change `wf.rs`, change the mirror
too.**

### Reading Verus syntax

Quick decoder for the bits that aren't ordinary Rust:

| Syntax | Meaning |
| --- | --- |
| `verus! { … }` | Verus code. Normal `cargo build` strips the ghost parts and keeps the data types |
| `pub open spec fn` | A mathematical definition, not executable code. `open` means other modules can see its body |
| `&&& a &&& b` | Just `a && b`, written as a bulleted list |
| `forall\|k: int\| #![trigger xs[k]] 0 <= k < xs.len() ==> P(xs[k])` | "P holds for every element of `xs`". The trigger is a hint for the solver; ignore it when reading |
| `decreases e` | Proof that the recursion terminates: each call is on a smaller `e` |
| `xs@` | The `Vec` viewed as a mathematical sequence (`Seq`) |
| `nat`, `int` | Mathematical naturals and integers (no overflow) |

## Building and verifying

- `cargo build` / `cargo test`: normal Rust. Verus isn't needed; the ghost code is erased.
- `scripts/verify.sh` (from the repo root): actually runs Verus on this crate. It downloads a
  pinned Verus into `target/`, checks the `vstd` pin matches, and **refuses** `admit(`,
  `assume(`, `external_body` or `#[verifier::external]` in this crate, so nothing can be
  "trusted without proof". If the bundled z3 won't start (it needs glibc 2.38), set
  `VERUS_Z3_PATH` to a Z3 4.16.0 binary. Verus rejects any other Z3 version unless you pass
  `-- -V no-solver-version-check`.
- Pins: `VERUS_TAG = 0.2026.10.26.d17fbd0` goes with `vstd =0.0.0-2026-10-25-0158` and Rust
  1.99.1 (installed by the script, selected through `RUSTUP_TOOLCHAIN`). The pairing comes from
  Verus's own `source/cargo-verus/toolchain-manifests/<tag>.toml`. Bump them together; the
  script's pin check enforces the `vstd` part. `cargo verus` needs no direct `verus_builtin*`
  pins.
- Expected Verus output: `inconsistent_fields` lint warnings on `Expr` (`index`, `op`),
  `Failure` (`id`) and `CheckErrorKind` (`op`, `expected`, `found`, …): a field name with
  different types in different variants. They only disable `->field` syntax.
  `autoderive_clone_without_spec` is silenced in `lib.rs`.

### Verus rules for this phase

1. `scripts/verify.sh` passes: Verus proves bounds, absence of overflow and termination. Exec
   fns have no functional `ensures`.
2. Exec fns are allowed inside `verus!{}`. Loop invariants are for safety only (bounds, no
   overflow). Lemmas, `ensures`, `assume`, `admit`, and `external_body` outside `prim.rs` are
   forbidden. `verify.sh`'s grep guard enforces the last part (it exempts `prim.rs`).
   A `requires` that only rules out overflow is fine.
3. Every exec fn that will get a spec has the spec's name and arguments, plus `memo: &mut Memo`
   where it needs one. Loops (`While`, `Repeat`) are Rust loops, documented as such.
4. Functional style: no closures, iterators, `HashMap`, fn pointers or generics apart from
   `E: Externs`.
5. Every id-based `Vec` read goes through `util::get` / `get_mut`; a miss is `Stuck(BadIndex)`
   in exec and a wf error in the checker. Loops over a node's own children index directly
   under `i < len`, because `get` (no `ensures`) would hide from Verus that the element is part
   of the node. Index arithmetic is checked.
6. Every program-level integer op, float op, float comparison and cast goes through `prim.rs`.
7. Every recursive fn and every loop has a `decreases`. A loop whose body recurses into a child
   needs `#[verifier::loop_isolation(false)]`, so the termination check sees that the child is
   part of the fn's argument. A rank comparison must be inline (`if r < rank`), not hidden in a
   helper fn: Verus only sees the decrease if the comparison is in the caller.
8. Recursive types get `clone_*` exec fns with `decreases`; the `impl Clone`s outside
   `verus!{}` delegate to them.

## When you change something

- **Adding an `Expr` / `Stmt` / `Type` variant:** add arms to `util::clone_*` (and `type_eq`
  for a type), `wf.rs` and its exec twin `check/wf.rs`, typing (`check/types.rs`), flow
  (`check/flow.rs`), scoping (`check/scope.rs`), the rank edges (`check/acyclic.rs::refs_*`),
  exec (`exec/expr.rs` or `exec/stmt.rs`), the step order in `check/mod.rs::step_key` if it has
  new children, the rendering in `tungsten-check/src/check.rs`, the mirror in `tungsten-check`'s
  `wf_mirror.rs`, and the lowering in `tungsten-check/src/lower/`.
- **Adding a `Value` variant:** `clone_value`, `value_has_type`, and the operator/cast dispatch
  in `exec/expr.rs`.
- **Adding a `CheckErrorKind`:** its `Clone` arm in `check/error.rs` and its message in
  `tungsten-check/src/check.rs`, plus a reject test in `tungsten-check/src/tests/check_errors.rs`.
- **Adding a table to `Program`:** add a new id type, a `wf_*` clause in `wf_program` and
  `check/wf.rs`, and the lowering in `tungsten-check/src/lower/program.rs`.
- Run `scripts/verify.sh` afterwards. It's the acceptance test for this crate.

## Design decisions

Each line gives the choice and the alternative that was rejected, so it doesn't get
"simplified" back.

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Scope | Data types, declaration tables and index-bounds `spec fn`s here; the lowering in tungsten-check. | Only `ir.rs`: the IR wouldn't be self-describing and nothing would exercise it. |
| Float literals | Raw bits (`F32(u32)`, `F64(u64)`), scales too. | Native floats: derived `PartialEq` breaks on NaN, and spec reasoning needs float axioms. |
| Normalization | Aggressive: resolved `FieldSel`, `MakeVec3`/`MakePos3`/`MakeRecord`, `ConstExpr` folded into `Expr`, `Ref` dropped, params are locals `0..k`. | A 1:1 mirror of tungsten-ir: more cases in every later proof. |
| Annotations | Kept (`ExternCall` params/ret, `Field.ty`, construct record ids); the checker verifies them. | Lowering checks and drops them: more trusted code. |
| Scoping | Units and functions keep their scoping lists with global ids; the checker enforces them. | Flattening: scoping bugs would only show up as backend failures. |
| Density inputs | One global `InputId` space with `owner: UnitId`. A function reading an input owned by `U` may only be listed in `U`. | A per-unit index resolved against the evaluating unit: silently reinterprets cross-unit reads. |
| Declaration tables | `externs`/`records` live in `Program`, so the checker needs only `&Program`. | A separate registry argument to the checker. |
| Names | Only in plain-Rust side tables (`Registry`, `LowerMap`). | Names inside core types: Verus would see `String`. |
| Derives | Derived `Clone`/`Debug`/`Eq`/… inside `verus!{}`, except a hand-written `Clone` for the recursive types (Verus rejects the derive as a possible cycle). | — |
| No proofs yet | Bounds, overflow and termination only. `prim.rs` is the only trusted file. | `external_body` gaps elsewhere, or reshaping the IR around tooling. |
| Termination across calls | Per-item ranks from `acyclic::ranks`, checked at every call/read; measure `(rank, layer, structure)`. | A global depth counter: a memoized cell computed deep and reused shallow would disagree with the unmemoized spec. |
| Fuel | Per `While` execution; `Repeat` is free. | A shared budget: the memo could change results. |
| Evaluation | Eager consts (global) and eager input buffers (per unit context), memoized per id; per-cell results; canonical error choice. | Lazy pointwise reads: a backend computes every scheduled input, so an unread failing cell would be a backend failure with an interpreter `Ok`. |
| Externs | Generic `E: Externs` with one `call`. | `&dyn Externs`: Verus has no trait objects. |
| Verus source | crates.io `vstd`, `=`-pinned, run with `cargo verus`. | A git revision and calling `verus` directly. |
| Verus binary | `verify.sh` downloads the pinned release (sha256-checked) to `target/verus/<tag>/`. | Requiring it on `PATH`. |
| Re-export | Reachable as `tungsten_check::core_ir`. | As `core`: shadows `::core` in derive output. |
| CI | Verus runs locally only. | Gating CI on it. |

## What's coming later

Later stages (see `TESTING.md`) add spec semantics (`spec/`), `well_typed`, the `ensures` on
`prim.rs` and the exec fns, and proofs (`proofs/`): refinement, type soundness (the checker's
`Ok` implies the interpreter never gets stuck; tested now by mutation), fuel monotonicity per
cell and memo soundness (both tested now), and the layout lemma.

Open questions:

- Whether `CompileError` should become `#[non_exhaustive]`, since `Invalid` is feature-gated.
- Core IR snapshots (TESTING.md L2), deferred to the testsuite.
