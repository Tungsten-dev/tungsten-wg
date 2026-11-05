# tungsten-check: a guide

This file explains how the code is organized, how to follow one program through it, and which
rules it follows (errors, id order, design decisions).

## What this crate does

It turns a `tungsten-ir` `OptimizedProgram` into the core IR defined in `tungsten-core`. Read
that crate's `README.md` first if you don't know what the core IR looks like.

```rust
let registry = RegistryBuilder::new()
    .extern_fn("sin", &[VariableType::F64], VariableType::F64)
    .record("Pair", &[("a", VariableType::F64), ("b", VariableType::I32)])
    .build()?;

match tungsten_check::lower(&program, &registry) {
    Ok(Lowered { program, map }) => { /* core_ir::Program + the name side table */ }
    Err(errors) => eprintln!("{errors}"),   // every problem, one per line, with its location
}
```

The core IR is re-exported as `tungsten_check::core_ir`.

### What it does *not* do

This is the most important thing to understand about the crate. **The lowering does not
type-check.** It only:

1. **resolves names** (refs → dense ids, extern/record/field names → ids via the registry), and
2. **normalizes shapes** (e.g. `Construct` → `MakeVec3`, `"x"` → `FieldSel::Vec3(X)`).

It reports an error only when the program *can't be expressed* in the core IR at all: a ref
pointing nowhere, an unknown name, a `Dynamic` host, and so on. A type mismatch, a missing
record field or an empty array literal is perfectly expressible, so it passes through
untouched for the checker (`tungsten-core`'s `check`, run by `prepare`) to reject.

The reason: this crate is unverified code. Keeping it "dumb" keeps the amount of trusted code
small.

### What is a lowering error, and what isn't

| `LowerErrorKind` | When |
| --- | --- |
| `DanglingRef { kind }` | A ref that is stale, removed or from another arena. Includes a `Var` from another function, a `DensityInput` ref found in no unit's arena, and `EntryPoint.density_function`. |
| `UnknownExtern` / `UnknownRecord` | The name isn't in the registry. |
| `UnknownField { base, field }` | Not `x`/`y`/`z` on Vec3/Pos3, or not in the registry's record. |
| `FieldOnNonStruct { base }` | The `Field`'s `base_type` isn't Vec3, Pos3 or a record. |
| `DynamicHost` | `HostInput::Dynamic`. |
| `DensityInputType` | `VariableType::DensityInput` as a parameter, return type, host, annotation, cast target or array element, or a read or assign of a `DensityInput` var. |
| `BadConstruct { t }` | `Construct` on a type other than Vec3/Pos3/Extern, or `ConstructExtern` on a non-`Extern` type. |
| `ConstructArity` | Vec3/Pos3 with ≠ 3 args, or a positional record construct with more args than fields. |
| `Parameter { problem }` | A parameter listed twice, or not in the function's arena. |

**Not** lowering errors (kept for the checker): every type mismatch, missing or duplicate
record fields, a positional record construct with too few args, empty array literals, dims
< 1, cyclic constants or calls, `Break` placement, definite assignment, scoping-list violations
and cross-unit density reads.

Dropped during lowering (kept in `LowerMap` where it matters): every name, `canonical_name`,
`source_hash`, `DensityInput.var` and table idents. A `Field`'s `base_type` is used only to pick
the selector.

## Files at a glance

```
src/
  lib.rs          lower() — the public entry point. Small; read it first.
  registry.rs     Registry + RegistryBuilder: extern signatures and record layouts, by name
  lower/
    mod.rs        Ctx (the state carried through the walk) and its helpers
    program.rs    run(): id assignment, then hosts → consts → functions → units → inputs → entries
    stmt.rs       statements, and how a Var ref becomes a LocalId
    expr.rs       expressions: the big match, field selection, constructs
    types.rs      VariableType → core Type
    consts.rs     ConstExpr → Expression, so constants go through the expression walker
  map.rs          LowerMap: core id ↔ tungsten-ir Ref, plus names
  error.rs        LowerError, LowerErrorKind, Location (Item + path of Steps)
  tests/          see "Tests" below
```

## Following a program through `lower`

### 1. `lib.rs::lower`

```
lower(p, registry)
  └─ lower::run(p, registry)   → (Program without externs/records, LowerMap, errors)
  if any errors → Err(LowerErrors)
  else copy the registry's extern/record tables and names into the program and map → Ok
```

The program's `externs` and `records` are always **exactly the registry's tables**, whether or
not the program uses them.

### 2. `lower/program.rs::run`: two passes

**Pass 1: `build_ids`** walks every arena and gives each item its core id, in arena
iteration order. It fills the `LowerMap`: units, functions, constants, hosts, tables, inputs
(unit by unit), each with a display name.

The order is **arena iteration order** (slot order, skipping holes), the same order `compact`
and `isomorphic` use. Inputs are numbered unit by unit, then in each unit's `density_inputs`
order. Entries keep their `entry_points` index. Extern and record ids are registry order.
Consequences:

- `lower(p) == lower(p.clone()) == lower(compact(p))` exactly, but `lower(permute(p, s))` may
  assign different ids.
- Holes vanish (ids are dense); a ref to a removed slot is a `DanglingRef`.
- Every function and unit is lowered and must be wf, reachable or not. A function shared
  between units is lowered once; its locals and body don't depend on the unit.
- An empty program lowers to an empty `Program`.

Doing this first means that by the time any expression is lowered, *every* id already exists.
A call to a function defined later just looks up `map.functions.id(r)`; there's no ordering
problem. If that lookup returns `None`, the ref is stale or from another program. That's a
`DanglingRef` error.

**Pass 2: lower each table**, in this fixed order (which is also the order errors come out in):

```
tables (no errors possible) → hosts → consts → functions → units → inputs → entries
```

Each step loops over the arena, calls `cx.enter(Item::…)` to say "errors from now on belong to
this item", and builds the core struct.

### 3. Lowering a function (`program.rs::function`)

This is where locals get their numbers:

1. Parameters first, in order → `LocalId(0)`, `LocalId(1)`, …  
   A parameter that is missing from the arena, listed twice, or has type `DensityInput` is an
   error.
2. Then every other var in the function's arena, in arena order, **skipping vars of type
   `DensityInput`**. Those vars are only placeholders for `DensityInput.var`; backends skip them
   too.
3. The `Ref<Var> → LocalId` table goes into `cx.locals` for the body, and into
   `map.locals[fn_id]` for later.
4. Then the return type, the `hosts` list, and the body (`cx.block`).

### 4. Statements (`stmt.rs`)

A one-to-one translation. The only interesting part is `local()`, which turns a `Ref<Var>`
into a `LocalId` using `cx.locals`. If the ref isn't there, it works out why: a
`DensityInput` var (error), a var from another function (`DanglingRef`), or a broken
parameter that was already reported (no second error).

### 5. Expressions (`expr.rs::expr`)

One big `match`, one arm per `tungsten-ir` `Expression` variant. Most arms are mechanical:
look up an id, lower the children, build the core variant. A few to know about:

- **`UnaryOp { Ref(_), e }`** returns `lower(e)` directly. The `Ref` disappears.
- **`ExternCall`** looks the name up in the registry, then lowers the *annotated* parameter and
  return types as written. It doesn't compare them with the registry's signature; that's for
  the checker.
- **`DensityVariable(r, _)`** finds `r` in `map.inputs`. That gives the one global input whose
  arena contains it, even when a helper function shared between units does the read.
- **`Field`** calls `field_sel` (below).
- **`Construct` / `ConstructExtern`** → `construct` / `construct_extern`. Vec3/Pos3 need exactly
  3 args. A record by position becomes fields `0..n`; a record by name looks up each field
  index. Written order and duplicates are kept.

When something fails, the arm records an error and returns a **placeholder** (a dummy literal
or id 0) so the walk can continue and find further errors. Placeholders never escape, because
`lower` returns `Err` if any error was recorded.

### 6. Field selection

To turn `foo.x` into a `FieldSel`, the lowering has to know whether `foo` is a `vec3`, a
`pos3` or a record. `tungsten-ir` stores that on every `Field` as `base_type`, filled by whoever
built the IR (legacy conversion, tungsten-parse, frontends). `field_sel` reads it:

- `vec3` / `pos3` → `FieldSel::Vec3(axis)` / `FieldSel::Pos3(axis)`, or `UnknownField` if the
  name isn't `x`/`y`/`z`
- an extern record → `FieldSel::Record(id, index)` via the registry, or `UnknownRecord` /
  `UnknownField`
- `density` → a quiet placeholder (the read of the `density` value is reported on its own)
- anything else → `FieldOnNonStruct`

A `density`-typed parameter is reported once at the parameter, and again at every read or
assign, since reading a `DensityInput` var is its own error.

**`base_type` is trusted for nothing else**, and lowering doesn't compare it with the base. If
it is wrong, the result is a wrong `FieldSel`, and the checker will notice that the base doesn't
have that type. So a bad annotation can produce a false rejection, never a silently miscompiled
program.

## Errors and locations (`error.rs`, `lower/mod.rs`)

Every error carries a `Location`: the **item** (which function, constant, unit, …) and a
**path** of `Step`s from the item down to the bad node. For example:

```
fn `base_main` › stmt 2 › then 0 › value › arg 1: unknown extern function `sinn`
```

The path is maintained by `Ctx`:

- `cx.enter(item)` starts a new item and clears the path.
- `cx.at(step, |cx| …)` pushes a step, runs the closure and pops the step. Every recursive
  call in `stmt.rs` and `expr.rs` goes through `at`, which is why the code is full of
  `self.at(Step::Arg(i), |cx| cx.expr(a))`.
- `cx.err(kind)` records an error at the current location.

Names in a location come from `canonical_name` or the `Name`, falling back to `#<core id>`
(`Ref`'s slot index isn't public). A unit's name is its main function's name, a Perlin table's
is its `ident`, and an input's is its var's name.

Errors come out in walk order (pass 2's order, then pre-order within an item: a node's own
error before its children's). `lower` never stops early; you get all of them.

### It never panics

`tungsten-ir`'s `Arena::get` panics on a ref from another arena. The lowering never calls it
directly. It uses `lower::get`, which checks `Arena::contains` first and returns `None`
otherwise. A proptest (`tests/no_panic.rs`) feeds deliberately broken programs to make sure.

## The registry (`registry.rs`)

The registry is how the lowering knows what `sin` or `Pair` means. The frontend that built the
program also knows its externs and records, so it builds the registry with `RegistryBuilder`:

- `extern_fn(name, params, ret)` / `extern_fn_with_domain_error(…)`
- `record(name, &[(field, type), …])`. Field types can name other records.
- `build()` resolves record names, assigns ids **in declaration order**, and rejects duplicate
  names, duplicate fields, unknown record names, `DensityInput` used as a type, and records
  that contain themselves by value (`find_record_cycle`).

The registry is **strict**: an extern, record or field the program uses but the registry
doesn't know is a lowering error.

## The side table (`map.rs`)

The core IR has no names, so `LowerMap` remembers everything the lowering threw away that
someone might still want:

- `IdMap<T, Id>` works both ways: `get(id) → Ref<T>`, `name(id) → &str`, `id(ref) → Id`. There
  is one for units, functions, consts, hosts, tables and inputs, plus one per function for
  locals (`map.locals[fn_id]`, or `map.local(fn_id, var_ref)`).
- Extras: `host_names` (the `Named` string, for binding host values by name), `table_idents`
  (the original table, with `ident`/`subident`), `input_vars`, extern names, record and field
  names.

Use it to turn a core id back into something a human recognizes, as `check.rs` does for checker
errors, or to resolve `ExternId → name` and `TableId → ident` in an `Externs` implementation.

## Checking and evaluating (`check.rs`, `eval.rs`)

The checker and the interpreter live in `tungsten-core` (verified). This crate wraps them:

```rust
let prep = tungsten_check::prepare(&program, &registry)?;   // lower + check, once per program
let case = prep.case(&NamedCase { origin: [0.0; 3], hosts: vec![("seed".into(), v)] })?;
let result = prep.eval(&case, &my_externs, EvalOptions::default())?;  // once per case
// or, one shot: tungsten_check::evaluate(&program, &registry, &named_case, &ext, opts)
```

- `prepare` returns `Invalid::Lower(LowerErrors)` or `Invalid::Check(CheckErrors)`. Checker
  errors are rendered with names: a `Location` (core path → `Item` + `Step`s; cycles are under
  `Item::Program`, a transitive missing const under `Step::WholeList`) and a message. The core
  `CheckErrorKind` is kept on each error.
- `Prepared::case` binds hosts by `Named` string (unknown, missing and duplicate names are
  `CaseBindError`s) and runs `check_case`.
- `Prepared::eval` runs `check_case`, then `eval_program`. Any `Stuck` in a cell or a failure
  diagnostic is `EvalFailure::Stuck`: an interpreter or checker bug, never a program error.
- `EvalOptions { fuel, memo }`: fuel per `While` execution (`DEFAULT_FUEL = 1 << 20`), and
  `memo: false` for the test-only recompute-everything mode (results are identical).
- `CaseResult { entries, failures }`: per entry, per cell in flat order, plus every failing const
  and input once. Backends are compared against it per case: no error anywhere → values must be
  bit-equal; any error → the backend must fail the case (TESTING.md §9.4).
- `check_lowered(&Lowered)` runs only the checker (for hand-built or mutated programs).

## Tests (`src/tests/`)

Most tests write a small `.tw` program, parse it with `tungsten-parse`, lower it, and compare
the result. `tests/mod.rs` has the helpers:

- `TestEnv` / `registry()`: a matching parser environment and registry (`sin`, `sample`,
  record `Pair`).
- `snippet(decls, body)`: wraps a function body in a small program with a host, tables, a
  constant and a density input, so a test only has to write the interesting line.
- `snippet(decls, body)`: the body goes in `fn top(p: f64) -> f64`, a helper of unit `u`
  whose 0-param main `top_main` returns `top(1.0)`; every fn in `decls` is listed as a helper.
- `ok(…)`: lowers, runs `check_wf`, and runs the checker. A program the checker rejects fails
  the test unless the test is listed in `check_rejects.rs` with a reason (deliberately
  ill-typed normalization tests, and the parse corpus files whose mains have params).

| File | What it checks |
| --- | --- |
| `normalization.rs` | One test per expression rule: input expression → expected core `Expr` |
| `errors.rs` | Every `LowerErrorKind` at least once, including how its location prints |
| `ids.rs` | Local numbering, input ids across units, shared helpers reading inputs |
| `determinism.rs` | `lower(p) == lower(p.clone()) == lower(compact(p))` |
| `map.rs` | Every id round-trips through the `LowerMap` |
| `registry.rs` | `RegistryBuilder` errors and id order |
| `no_panic.rs` | proptest: break refs at random, `lower` must return `Err`, never panic |
| `legacy.rs` | Programs from `tungsten-legacy`'s builders lower cleanly (and pass the checker) |
| `check_errors.rs` | One reject test per `CheckErrorKind` (both hints), with its rendered location; hand-built core programs for wf and forbidden const inits |
| `check_rejects.rs` | The lowering tests whose programs the checker rejects on purpose, with reasons |
| `eval.rs` | Interpreter results computed by hand (operators × types, overflow, shifts, casts, `-0.0`, short-circuit, loops, fuel, consts, density inputs, eager failures, canonical errors, externs) and the `prepare`/`Prepared`/`evaluate` API |
| `stuck_mutation.rs` | proptest: checker-accepted programs mutated on the core IR; those still accepted never get `Stuck`. Proven in `tungsten-core`; this runs the real primitives and externs |
| `memo.rs` | proptest: memo on == memo off, including the order of diagnostics, which isn't proven; results are monotone in fuel per cell |

Run them with `cargo test -p tungsten-check`.

## How it's used from the facade

With feature `check`, the root crate exposes `compile_checked(program, &registry, backends)`.
It calls `prepare` (lower + the core checker) first. If that fails it returns
`CompileError::Invalid(check::Invalid)` and runs no backend.

## Design decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| What fails lowering | Only what the core IR can't represent. All errors are collected, each with a location. | Type-checking here (more trusted code); stopping at the first error. |
| Field resolution | The IR stores the base type (`Field::base_type`); lowering trusts it only to pick the selector, and the checker validates it. A bad annotation becomes a checker reject, not a silent change. | A field-name string in the core IR; re-deriving the base type bottom-up in lowering (the old `synth.rs`, a second copy of the frontends' type rules). |
| Record construction | `MakeRecord` fields in **written** order, duplicates kept; evaluation follows source order. | Reordering to registry order: changes which error wins. |
| Empty `ArrayLiteral` | Lowered to `ArrayLit([])`; the checker rejects it. | A lowering error. |
| Registry | Always required and strict. Frontends build it with the public `RegistryBuilder` (the testsuite's `registry.toml` loader too). | An Infer mode that builds the tables from the program's own annotations. It existed briefly and was removed: it only checks the program against itself, never against the runtime, and needs unverified evidence-merging rules. |
| `LowerMap` | Both directions: core id → `Ref` + display name, and `Ref` → id. | Names only. |
| Crate | Lowering, `prepare` and `evaluate` live here; the checker and interpreter themselves are in `tungsten-core`. There is no `tungsten-interpreter` crate. | A separate `tungsten-interpreter` facade crate (TESTING.md's earlier plan): it would only wrap these entry points. |
| API | `prepare` once per program, `Prepared::eval` per case, plus a one-shot `evaluate`. | One-shot with core ids (callers don't know them before lowering); re-lowering per case. |
| Errors | `Invalid = Lower(LowerErrors) \| Check(CheckErrors)`. | One merged list: blurs which stage failed. |
| Facade | Opt-in feature `check` with `compile_checked` (lower + check); `compile` unchanged. | Always on (every user would need vstd and a registry). |
| Tests | `.tw` snippets via `tungsten-parse`, plus hand-built programs for what text can't express. | Hand-built only. |

Open questions:

- A `RegistryBuilder` that takes core `Type`s directly, if a non-tungsten-ir caller appears.

## When you change something

- **New `tungsten-ir` `Expression` variant:** add an arm in `lower/expr.rs::expr`, plus a
  normalization test.
- **New core IR variant:** see the checklist in `tungsten-core/README.md`. On this side: the
  lowering and the rendering in `check.rs`.
- **New `CheckErrorKind`:** its message in `check.rs::Names::message` and a test in
  `tests/check_errors.rs`.
- **New error kind:** add it to `LowerErrorKind` in `error.rs` with a `Display` message, and
  add a test in `tests/errors.rs`.
- **New arena in `OptimizedProgram`:** give it ids in `build_ids`, lower it in `run` at the
  right point in the walk order, and add it to `LowerMap`.
