# `tungsten-builder`: an IR builder for frontends

Status: design agreed, not implemented. Lands after milestone 3 of `specs/program-level-inputs.md` and before its
milestone 5, which deletes `tungsten-legacy`. `specs/density-mappings.md` and `specs/refinement.md` land later and
extend it.

## 1. Summary

A frontend currently has three ways to produce a program: legacy SPMT plus `conversion`, `.tw` text, and hand-written
`OptimizedProgram` literals. Once legacy is gone, only the last two remain, and neither suits a compiler that lowers a
recursive source language. `tungsten-builder` is the replacement: a new crate for constructing valid-by-construction
`OptimizedProgram`s from Rust.

```rust
use tungsten_wg::builder::prelude::*;

let mut pb = ProgramBuilder::new();

// A unit, finished before anything reads it.
let mut f = FnBuilder::new("base", F64);
f.ret(pos3().x().cast(F64) * 10.0);
let base = pb.finish_unit(f);

// A helper. It reads an input, which is legal now that inputs are program-level.
let grid = Grid::new((4, 1, 1));
let d = pb.input(base, grid);
let mut h = FnBuilder::new("shared", F64);
h.ret(d.read());
let shared = pb.finish_helper(h);

// `body!`/`build!` and plain methods build the same statements.
let mut f = FnBuilder::new("main", F64);
let i = f.local("i", I32);
build!(f, {
    i = 0;
    let total: F64 = shared.call([]);
    while (i.lt(3)) {
        total += d.at(i);
        i += 1;
    }
    return total;
});
let main = pb.finish_unit(f);
pb.entry(main, grid);

// Every error from the whole build, or a program whose unit lists match its bodies.
let program: OptimizedProgram = pb.build()?;
```

### Goals

- A readable, ergonomic API: the existing `body!` style for hand-written code, and plain methods for generated code.
- Recursive lowering works without `Option::take`, `RefCell` or a lifetime on the builder.
- Lists on units and functions always match the bodies, including transitive helpers.
- Type mistakes are reported where they were written, all at once, without panics.
- New IR fields (`mapping`, `previous`, refinements) reach frontends through builder methods, so they don't break
  frontend code.
- Depends only on `tungsten-ir`, and nothing in the backends depends on it.

### Non-goals

- **Porting `tungsten-mc-compile`.** It stays on a pinned older version (§8). §9 records what was agreed for a later port.
- Being a second checker. Flow, grids, cycles, and agreement with the registry stay in core (§4.3).
- Editing an existing `OptimizedProgram`. The builder only constructs.
- Writing invalid programs on purpose. `reject` tests stay in `.tw`.
- Compile-time typing (`Expr<F64>`), and a proc macro with real `<` and `>>` (§11, §12).
- Replacing `spmt::pretty` and `spmt::dag`. `tungsten_parse::print` and `tungsten-inspect` already print IR.

## 2. Crate

```text
crates/tungsten-builder/
  src/lib.rs        re-exports and prelude
  src/expr.rs       Expr, operators, typing rules
  src/function.rs   FnBuilder
  src/program.rs    ProgramBuilder, handles, Grid, InputSpec
  src/derive.rs     unit lists from bodies
  src/decls.rs      ExternDecl, RecordDecl, Decls
  src/macros.rs     body!, build!, extern_fns!, record_decls!, vec3!, pos3!, array!, vt!
  src/math.rs       sin, cos, sqrt, floor, pow, min, max, clamp and their decls
  src/error.rs      BuildError, BuildErrors
  tests/            typing agreement, derivation, slices (§10)
```

| Area | Change |
| --- | --- |
| Dependencies | `tungsten-builder` depends on `tungsten-ir` and `thiserror`. Dev-dependencies: `tungsten-check`, `tungsten-parse` |
| Facade | `pub use tungsten_builder as builder`, as a non-optional dependency like `tungsten-ir` |
| Macros | Exported from `tungsten_builder`, so they're reached as `tungsten_wg::builder::body!`. They don't conflict with the legacy root re-exports (`tungsten_wg::body!`), which go away at program-level milestone 5 |
| Registry helper | `tungsten_wg::registry_from(decls)`, behind the `check` feature (§7) |
| `docs/redesign.md` rule | Builder depends only on `ir`. Only the facade depends on the builder |

## 3. API

### 3.1 Program

`ProgramBuilder` owns the arenas. Everything it returns is a small `Copy` handle.

```rust
impl ProgramBuilder {
    pub fn new() -> Self;

    pub fn finish_unit(&mut self, f: FnBuilder) -> Unit;     // errors if f has params
    pub fn finish_helper(&mut self, f: FnBuilder) -> Helper;
    pub fn entry(&mut self, unit: Unit, grid: Grid);

    pub fn input(&mut self, source: Unit, grid: Grid) -> Input;
    pub fn input_with(&mut self, spec: InputSpec) -> Input;
    pub fn host(&mut self, name: &str, t: VariableType) -> Host;
    pub fn perlin_table(&mut self, ident: &str, sub: Option<&str>, index: usize) -> Table;
    pub fn sampler_table(&mut self) -> Table;                // Base3DNoise
    pub fn constant(&mut self, name: &str, value: impl Into<Expr>) -> Constant;

    pub fn fresh_name(&mut self, prefix: &str) -> String;    // prefix_0, prefix_1, ...
    pub fn build(self) -> Result<OptimizedProgram, BuildErrors>;
}

pub struct Grid { /* dimensions, scaled_origin, scaled_position */ }
impl Grid {
    pub fn new(dims: (i32, i32, i32)) -> Self;               // scales default to (1, 1, 1)
    pub fn origin_scale(self, s: (f64, f64, f64)) -> Self;
    pub fn position_scale(self, s: (f64, f64, f64)) -> Self;
}

pub struct InputSpec { /* source, grid, name */ }
impl InputSpec {
    pub fn new(source: Unit, grid: Grid) -> Self;
    pub fn named(self, name: &str) -> Self;
}
```

`finish_*` always returns a handle, even if the function has errors, so lowering continues and later errors are
reported too. Errors collect in the `ProgramBuilder`, and `build` returns them.

**Only finished units can be referenced.** `input` takes a `Unit`, and a `Unit` exists only after `finish_unit`, and
likewise for `Helper` and calls. Unit and call cycles therefore can't be written. No forward-declaration API is needed
in this version, including for refinement, whose reruns are declared after their units.

### 3.2 Merging

| Item | Key | On a match |
| --- | --- | --- |
| Input | Source, dims, scales by `f64` bits, and later `mapping` and `previous` | Returns the existing input. The first name wins |
| Host | Name | Same type: returns the existing host. Other type: `HostTypeConflict` |
| Table | The `PermutationTableInput` value | Returns the existing table |
| Constant, helper, unit | Not merged | Each call adds a new one |

Merging inputs means one input can be listed by several units. Backends already share the buffer through `Instance`,
and the inliner counts one use per `(unit, distinct input)`, so its decisions don't change. On the interpreter, a
shared input is built once and its failure is reported once (program-level-inputs §4.1). Scales compare by bits, so
`0.0` and `-0.0` make two inputs.

An input no unit reads stays in the arena, unlisted and inert (program-level-inputs §4.2). A host no function reads
stays too, and so remains a field in the generated host API, which lets a frontend keep that API stable.

### 3.3 Functions

`FnBuilder` is owned and borrows nothing, so several can be open at once. Whether it becomes a unit or a helper is
decided by which `finish_*` it goes to. `lower_normal_noise` in the frontend relies on this kind of choice through its
`as_density` flag.

```rust
impl FnBuilder {
    pub fn new(name: impl Into<String>, ret: VariableType) -> Self;
    pub fn source_hash(&mut self, hash: u64);                // default 0

    pub fn param(&mut self, name: &str, t: VariableType) -> Local;
    pub fn local(&mut self, name: &str, t: VariableType) -> Local;  // Name::Named; duplicate is an error
    pub fn temp(&mut self, prefix: &str, t: VariableType) -> Local; // Name::Prefixed
    pub fn anon(&mut self, t: VariableType) -> Local;               // Name::Anonymous

    pub fn assign(&mut self, target: Local, value: impl Into<Expr>);
    pub fn ret(&mut self, value: impl Into<Expr>);
    pub fn break_(&mut self);
    pub fn if_then(&mut self, cond: impl Into<Expr>, then: impl FnOnce(&mut Self));
    pub fn if_else(&mut self, cond: impl Into<Expr>,
                   then: impl FnOnce(&mut Self), other: impl FnOnce(&mut Self));
    pub fn while_(&mut self, cond: impl Into<Expr>, body: impl FnOnce(&mut Self));
    pub fn repeat(&mut self, count: usize, body: impl FnOnce(&mut Self));

    pub fn block(&mut self, body: impl FnOnce(&mut Self)) -> Block; // captures, doesn't append
    pub fn append(&mut self, block: Block);
}
```

Recursive lowering needs no special support. The current function and the program are separate values, and a closure
can use both:

```rust
fn lower(pb: &mut ProgramBuilder, f: &mut FnBuilder, d: &Density) -> Expr {
    match d {
        Density::Add(a, b) => lower(pb, f, a) + lower(pb, f, b),
        Density::Cached(inner, grid) => {
            let mut g = FnBuilder::new(pb.fresh_name("cached"), F64);   // f is still open
            let r = lower(pb, &mut g, inner);
            g.ret(r);
            let unit = pb.finish_unit(g);
            pb.input(unit, *grid).read()
        }
        Density::Max(a, b) => {
            let r = f.temp("max", F64);
            let a = lower(pb, f, a);
            f.assign(r, a);
            f.if_then(r.lt(0.0), |f| {
                let b = lower(pb, f, b);                                // uses pb inside the closure
                f.assign(r, b);
            });
            r.into()
        }
        // ...
    }
}
```

Assigning requires a `Local`, so an ambient can't be a target. Where the frontend's `water_levels` moves `pos3` today,
it declares a local and copies the ambient into it, which is what conversion generated implicitly.

### 3.4 Expressions

`Expr` holds an IR `Expression`, its type if known, and any problems so far (§4). It is `Clone`. `Local`, `Input`
(as an unindexed read), `Host`, `Table`, `Constant`, `f32`, `f64`, `i8`, `i32`, `i64` and `bool` all convert with
`Into<Expr>`, and handles have the same methods as `Expr`.

| Group | API |
| --- | --- |
| Ambients | Free functions `pos3()`, `origin()`, `origin_scale()`, `position_scale()` |
| Arithmetic | `+ - * /`, unary `-`, unary `!`. Literals work on the left: `2.0 * x` |
| Comparison, logic | `lt le gt ge eq_expr ne and or` |
| Bits | `bitand bitor bitxor shl shr ushr` |
| Structure | `index(i)`, `field(name, type)`, `x() y() z()` for `Vec3`/`Pos3` |
| Conversion | `cast(type)`, `as_ref()`, `as_mut_ref()` |
| Construction | `vec3!`, `pos3!`, `array!`, `Expr::record("Name", [("field", e), ...])` |
| Reads | `input.read()`, `input.at(index)`, `helper.call([args])`, extern functions (§7) |
| Reuse | `reusable()` returns a closure that clones, as in the legacy builder |

Method names match the legacy builder, except that `.x()`, `.y()`, `.z()` and `Expr::record` are new. Variables no
longer resolve by name: legacy's name-matched `pos3` and `origin`, and conversion's `find_named_var` fallback, have no
equivalent.

### 3.5 Macros

`body!` and `build!` keep today's syntax and expand onto §3.3's methods:

| Macro | Result |
| --- | --- |
| `build!(f, { ... })` | Appends to `f` |
| `body!(f, { ... })` | Returns a `Block`, for `f.append` |

| Form | Expands to |
| --- | --- |
| `x = e;` | `f.assign(x, e)` |
| `x += e;`, `x -= e;`, `*=`, `/=` | `f.assign(x, x <op> e)`. `*=` and `/=` are new |
| `let x: T = e;` | `f.temp("x", T)`, then assign. Binds `x` as a `Local` |
| `let x = e;` | Plain Rust `let` |
| `return e;`, `break;` | `f.ret(e)`, `f.break_()` |
| `if (c) { } else { }`, `while (c) { }`, `repeat (n) { }` | The closure methods |

`let x: T` uses `temp`, not `local` as legacy did. The same macro block can run twice in one function during
recursion, and with `local` the second run would be a `DuplicateLocal`.

Rust's `<` has to return `bool`, so comparisons stay `.lt()` and friends. A later proc macro could lift this (§12).

## 4. Typing and errors

### 4.1 Typing

Every `Expr` computes its type when it's built, following `tungsten-core/CHECK.md` §4.1 to §4.3, including:

- **No implicit conversions.** `x + 1` with `x: f64` is a problem.
- **No scalar broadcast.** `vec * 2.0` is a problem, while `Vec3 op Pos3` gives `Vec3`.
- **Indexes are `I32`,** both for arrays and for `input.at(...)`.
- **Equality is for scalars only.**

`Field` fills `base_type` from the base's type, which is the reason `Expr` is typed at all. On `Vec3` and `Pos3`, the
stated type is verified. On a record (`Extern("Name")`) the builder doesn't know the fields, so it trusts the
annotation and core checks it (`FieldAnnotation`). `Ref` has its operand's type.

Types are stored as `&'static VariableType` from a small process-wide interner, because `VariableType` has a `Box` in
`Array` and handles must stay `Copy` for operators to read well. Only distinct types are ever leaked, and there are
few of those. `Extern(&'static str)` already takes the same approach.

### 4.2 Poison and collect

Operators can't return `Result`, so a bad operation produces an `Expr` that carries the problem:

1. The operation records a `BuildError` with its `#[track_caller]` location.
2. Its type is the result type where that's clear (for example a call with the wrong arity keeps the return type,
   like core's §4.1), and otherwise unknown.
3. An unknown operand raises no further problem, so one mistake produces one error.
4. Later operations carry the problems upward. When the `Expr` reaches a statement, `FnBuilder` takes them.
5. `finish_*` moves them to the `ProgramBuilder`, and `build` returns all of them, in order, as `BuildErrors`.

An `Expr` that is dropped before reaching a statement takes its problems with it. It can't affect the program.

```rust
pub struct BuildError {
    pub kind: BuildErrorKind,
    pub function: Option<String>,
    pub location: &'static std::panic::Location<'static>,
}
```

| Kind | When |
| --- | --- |
| `BadBinary`, `BadUnary`, `BadCast` | §4.1 rules |
| `BadField`, `NotArray`, `BadIndex` | Structure rules, including a `Vec3`/`Pos3` annotation mismatch |
| `Arity`, `ArgumentType` | Helper and extern calls |
| `AssignType`, `ReturnType`, `ConditionType` | A statement's expression has the wrong type |
| `BadConstruct`, `EmptyArray`, `ArrayElement` | Construction rules |
| `ForeignHandle` | §4.4 |
| `DuplicateLocal` | `local` with a name already in the function |
| `NotConstant` | A `constant` value that isn't a constant expression. Names what was found |
| `HostTypeConflict` | §3.2 |
| `UnitHasParams` | `finish_unit` on a function with parameters |

Messages use the wording of the checker's. Nothing in this crate panics on a misused handle or an ill-typed
expression.

### 4.3 What stays in core

The builder doesn't check flow (missing returns, unassigned reads, `break` outside a loop), grid rules, or externs and
records against the registry. `compile_checked` reports those. The builder guarantees only that lists match bodies and
that the program's refs are valid, so a builder program always lowers past `DanglingRef` errors. It can still fail
the checker.

### 4.4 Handles from elsewhere

A `Local` belongs to one function. Legacy variables didn't: the frontend reuses one `y` across helpers, and
`find_named_var` quietly matched others by name. Each statement scans its expressions once when added, and a `Local`
not in the function's arena is `ForeignHandle`, located at the statement. Handles from another `ProgramBuilder` are
caught the same way at `finish_*`. This uses `Arena::contains`, which unlike `get` doesn't panic on a wrong tag.

## 5. Derived lists

`finish_unit` and `finish_helper` fill every list. There is no API to list anything by hand.

| IR field | Contents | Order |
| --- | --- | --- |
| `Function.host_inputs` | Hosts this function reads directly | First use |
| Unit `helper_functions` | Everything reachable from `main` | Callees before callers |
| Unit `host_inputs` | Union over `main` and its helpers | First use |
| Unit `permutation_table_inputs` | Tables read by any of them | First use |
| Unit `constants` | Constants read, and those their initializers reach | Dependencies first |
| Unit `density_inputs` | Inputs read by any of them | First use |

"First use" follows evaluation order through `main`, entering a callee at its first call, after the call's arguments.
Each finished helper stores its ordered summary, so a unit merges it at the call without re-walking the helper. The
result is deterministic, and it satisfies core's scoping rule (`f.hosts ⊆ U.hosts`, calls stay in `F(U)`).

**Consequences:**

- **A read decides.** A read in dead code still lists the input, and an unread input can't be listed. Eagerly building
  an input for its failure (program-level-inputs §8, "Eagerness") can't be written with the builder. Use `.tw` for it.
- **Helpers can read anything.** Inputs, hosts, tables and constants all propagate to every unit that uses the helper.
- **Order may differ from conversion's.** Conversion listed declared items first, and the frontend adds inputs when it
  creates them. A ported frontend may therefore schedule differently and drop unread inputs, which is why §9 compares
  values, not bytes.

`rebuild.rs` and `retag.rs` need no change. The builder writes the IR's existing fields.

## 6. Names and hashes

- **Names are required.** The frontend's fallback uses an address-derived name (`noise_function_{ptr}`), and addresses
  differ between runs. `fresh_name` gives a deterministic alternative.
- **Duplicates are allowed.** `Interface` already makes helper and constant names unique. Milestone B1 tests the same
  for units, and fixes the gap in `tungsten-interface` if there is one.
- **`source_hash` defaults to 0.** With names required, no code path falls back to the hash for a name. The Java
  backend prints it and the inliner mixes it, and both handle 0. A frontend that has a real source hash can set it.
  The legacy `Fingerprint` isn't carried over.

## 7. Externs and records

Each signature is written once, and produces both the call function and a declaration:

```rust
extern_fns! {
    group aquifer_externs;
    random_next_int(r: "Random", bound: I32): I32,
    packed_pos(x: I32, y: I32, z: I32): I64,
    checked_div(a: F64, b: F64): F64 @domain_error,
}

record_decls! {
    group structure_records;
    Piece { terrain_adjustment: I32, block_box: "BlockBox" },
}

let registry = tungsten_wg::registry_from([
    aquifer_externs(),
    structure_records(),
    tungsten_wg::builder::math::decls(),
])?;
tungsten_wg::compile_checked(&program, &registry, &mut [&mut rust])?;
```

- `group name;` generates `pub fn name() -> Decls`. It's a function because `VariableType` can't be `const`.
- Generated call functions take `impl Into<Expr>` and check arity and argument types, with `ArgumentType` and `Arity`.
  There is deliberately no untyped extern call, so the frontend's 29 inline `ExternCall` literals in the main lowering
  files would each get a declaration.
- `math` keeps the eight functions legacy shipped (`sin cos sqrt floor pow min max clamp`), so ported code keeps
  working, and their declarations are opt-in. A frontend's runtime must still provide them.
- `registry_from` feeds `RegistryBuilder`. It drops identical duplicates, so groups can overlap, and leaves conflicts
  for `RegistryBuilder` to report.

**What this does and doesn't check.** The call site and the registry now share a source, so `ExternSignature` can't
fail for macro-generated calls. This is declaration, not the inference `tungsten-check/README.md` rejected: a
program's calls can't add an extern, and each argument is still checked against the declaration. But agreement between
the declaration and the runtime that implements it isn't checked here. That still needs a backend compile or the
testsuite.

## 8. Effects on other specs

Edited in place, as those specs' own decisions ask.

| Spec | Change |
| --- | --- |
| `program-level-inputs` §1, §6.8, §7, §8, §9, §10 | Milestone 5 no longer needs the port. It needs the frontend pinned (below) |
| `density-mappings` §5.6, §9 | Adds `InputSpec::mapping`, included in the merge key. The frontend's move waits for its port |
| `refinement` §3, §12 | Adds `InputSpec::previous`, included in the merge key, and `pb.refinement(...)`. Frontend adoption waits for its port |

**Pinning the frontend.** Before milestone 5 lands:

1. Tag the last commit with `tungsten-legacy`, after program-level milestone 4.
2. Switch `tungsten-mc-compile`'s `tungsten-wg` dependency from the path to that tag.
3. Confirm `cargo check --workspace` passes in `tungsten-frontends`.

The frontend then gets no mappings, refinement or builder until a port moves it to a newer version.

## 9. For a later `tungsten-mc-compile` port

Not part of this work, but agreed now so it isn't re-decided:

- **Proof:** generated code matches the code generated from the pinned tag, bit for bit in values, on vanilla, for
  every route on fixed seeds and chunks. Byte diffs are reviewed and don't block.
- **Checking:** every compile goes through `compile_checked`, including datapacks. Measure the cost, which is unknown.
- **Expected findings:** the frontend has never run the checker. `as_index` and `flat_y_zero_index` declare `F64` but
  are used as density indexes, which must be `I32`. Others are likely, and poison-and-collect exists to show them
  together.

How to sequence the port, for example a parallel module or a rewrite in place, is left to it.

## 10. Testing

### Slices: the API's proof

No real frontend will use the crate at first, so tests in `crates/tungsten-builder/tests/slices/` rewrite real frontend
patterns with the builder:

| Slice | From | Exercises |
| --- | --- | --- |
| `noise` | `noise.rs` | Generated statements from loops, tables, method-style bodies |
| `recursive` | `density.rs` | A small `Density` enum lowered recursively, nested open units, memoized inputs, merging |
| `top_surface` | `find_top_surface.rs` | `build!` with `while` and `break`, indexed reads, a 3D input read from a flat unit |
| `aquifer` | `aquifer.rs` | Helpers with parameters, hosts, constant arrays, `as_mut_ref`, nested `body!` blocks |
| `shared` | Program-level §2 | A helper reading an input from two units, which the frontend can't do today |

Each slice has a twin `.tw` file in the testsuite corpus, with expected values computed by hand. The test asserts that
the built program is `isomorphic` to the elaborated twin, and the corpus runs the twin on the interpreter and every
backend. Slices use externs from `registry.toml`, adding there where needed. A slice's real-frontend externs are
replaced where the toy runtime lacks them.

**Review gate.** Milestone B3 ends with each slice placed next to the frontend code it came from, and a written list
of friction points. API changes that come out of it land before the crate is called done.

### Other tests

| Concern | Test |
| --- | --- |
| Typing agrees with core | For every operator, unary and cast over all scalar types and `Vec3`/`Pos3`, generate a one-expression `.tw` program, run `check::prepare`, and compare core's verdict with the builder's verdict and result type. Exhaustive, and it fails if core's rules change |
| Derivation | Transitive helpers, a shared helper with different callers, constants reaching constants, order. `check::prepare` accepts each, and removing any list entry makes it fail |
| Merging | Each row of §3.2, including `-0.0`, first name wins, and `HostTypeConflict` |
| Merged inputs on the inliner | A merged input read by two units gives `MultipleUses` |
| Poison | One mistake gives one error at the right `file:line`, with several mistakes all reported, and none panic |
| `ForeignHandle` | A `Local` from another function, and each handle type from another `ProgramBuilder` |
| Macros | Each `body!` form gives the same IR as its method-style equivalent |
| Determinism | Build twice, and output is `isomorphic` with identical generated code |
| Text | Each slice round-trips through `tungsten_parse::print` |

**Mutation checks, done once and recorded.** Each must turn the suite red: dropping helpers from the summary merge;
leaving `previous` or `mapping` out of the merge key, once they exist; widening `Vec3 op Pos3` to any numeric pair;
listing inputs in reverse order.

### Milestones

Each lands green. All of them come after program-level milestone 3.

- **B1. Core.** `Expr` with typing and poison, `FnBuilder`, `ProgramBuilder`, derivation, merging, and their tests.
- **B2. Macros and declarations.** `body!`, `build!`, `extern_fns!`, `record_decls!`, `math`, `registry_from`.
- **B3. Slices.** §10's slices, twins, and the review gate.
- **B4. Documentation.** A crate `README.md`, `docs/redesign.md` (layout, rules, decisions), root `README.md`
  (workspace layout, usage).

Program-level milestone 5 needs §8's pin. It doesn't need the builder technically, but landing the builder first means
this repo always has a way to construct programs from Rust.

## 11. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Location | New `tungsten-builder` crate | In `tungsten-ir`: the leaf crate gains operators, macros and a derivation pass. In `tungsten-frontends`: other frontends and this repo's tests can't use it, and it drifts from the IR |
| Nesting | Owned `FnBuilder` plus `&mut ProgramBuilder` | `Rc<RefCell>`: runtime borrow panics and `!Send`. Central store with ids: every statement needs two arguments |
| Field types | `Expr` carries its type | Finish pass: errors far from their cause, and IR has no placeholder. Caller states it: noise at each call, and the frontend doesn't track types |
| Lists | Always derived | Plus explicit extras: more API for a case only tests need. Explicit: transitive helpers become frontend bookkeeping |
| Type errors | Poison and collect | Panic: one error per run, and it bends the redesign.md rule. Never judge: mistakes surface only if the frontend runs the checker |
| Merging inputs | By default | Per unit: clashes with program-level handles. Never: every frontend writes the same cache |
| Body syntax | Methods as the base, `body!` on top | Macro only: fits generated loops poorly. Methods only: the wordiest, and existing `body!` code would need rewriting. Statements as values: locals must be declared outside, more nesting. `.tw` templates: errors at runtime, and a dependency on `tungsten-parse` |
| Static or dynamic types | Dynamic `Expr` | `Expr<F64>`: needs a marker per record, const generics for arrays, and type erasure wherever lowering's result type varies |
| Externs | One declaration, two outputs | Separate registry: signatures written twice. Builder takes a registry: string-named calls |
| Landing | After program-level m3 | Before: internals rewritten at m3. After m4: delays with no dependency |
| Frontend port | Out of scope, frontend pins the last legacy tag | Keep m5 blocked on the port: density-mappings and refinement stay blocked too |
| API proof | Real slices with a review gate | Toy frontend: misses what makes real code awkward. Unit tests only: verify correctness, not how the API reads |
| Spec location | `specs/builder.md` | Root `SPEC.md`: holds the testsuite design |

Decided without an interview question, and easy to revisit: `FnBuilder::new` rather than `FnBuilder::unit`, since the
kind is chosen at finish; `let x: T` in macros using `temp`; names being required; `source_hash` defaulting to 0;
interned `&'static` types; keeping `math`; `Block` as an opaque type; the `group` and `@domain_error` syntax; the
`Grid` and `InputSpec` shapes; and the milestone split.

## 12. Open and deferred

- **Proc macro.** A `tw!` macro with real `<`, `>>`, `&&` and `as`, expanding onto the same `FnBuilder` methods, could
  be added later with no change to existing code. Look at `../tungsten-macros` before starting. This spec's author
  hasn't read it.
- **Raw IR escape hatch.** There's no way to insert a raw `Expression`. Add one only if the slices or the port need it.
- **The frontend port.** No owner or timing, and it's what brings mappings and refinement to `tungsten-mc-compile`.
- **Unmeasured:** the cost of poison tracking in `Expr` on large programs, and the core checker on vanilla (§9).
- **Other backends' tests.** Program-level §6.8 moves them to `.tw`, and that stays. They could use the builder later.
