# Testing policy: verified interpreter, text format, differential backend tests

Status: partly implemented. `tungsten-parse`, `tungsten-core` (checker and interpreter, no
functional proofs yet), `tungsten-check` and `tungsten-testsuite` exist. The testsuite does
differential testing against Rust, Java and CUDA (milestones 1 to 3 and 5; per-milestone notes in
`SPEC.md`). Backend conformance is still open (§4 item 4) and is tracked through `@knownfail`. The CI
workflow is deferred (§12). Per-item status is marked in §3 and §4. The design of
`tungsten-testsuite` is in `SPEC.md` at the repo root; this file reflects its decisions.

## 1. Summary

The backends are tested independently of any frontend. A test is a small program written in a
text format, plus input cases. A **verified interpreter** is the ground truth. Every backend
compiles the program, runs it out of process on the same cases, and must match the interpreter
**exactly**, with NaN as the only exception: any NaN equals any NaN.

New pieces:

- `tungsten-parse`: a lenient parser from text to `OptimizedProgram` and a canonical printer
  back to text.
- `tungsten-core`: the single **Verus-verified** crate, with no dependencies besides Verus'
  own. It holds the core IR, the well-formedness and type checker, the spec semantics written
  as `spec fn`s, and the executable interpreter, which is proven to refine that spec. Everything
  in this crate is proven, and everything outside it is tested.
- `tungsten-check`: plain Rust, unverified. The registry, the lowering from
  `OptimizedProgram` to the core IR, and the entry points `tungsten_check::{prepare, evaluate}`
  (lower → check → evaluate). User-facing through the facade's `check` module and `Compiler`. See its
  `README.md`.
- `tungsten-testsuite` (`publish = false`): the harness, the test corpus, the extern registry,
  the prelude, and minimal test runtimes for Rust, Java and CUDA.

## 2. Decisions

| Topic | Decision |
| --- | --- |
| Verification tool | Verus. The crates build with plain `cargo` (`verus!{}` erases). `vstd` comes from crates.io, `=`-pinned. Proofs run with `scripts/verify.sh` (downloads the pinned release, runs `cargo verus verify -p tungsten-core`), locally only. They don't gate CI. |
| What is proven | Spec + refinement: exec interpreter == spec semantics. Plus type soundness of the checker, fuel monotonicity, and the layout lemma (§6.3). Floats are uninterpreted in both spec and exec. |
| Verified IR | A lowered **core IR** (dense `Vec` indices, resolved types, no names). The lowering from `OptimizedProgram` is ordinary Rust and is tested, not proven. |
| Checker | Verified. `check(p) == Ok` ⇒ the interpreter never gets stuck. |
| Termination | Fuel **per `While` execution**: one `While` may run at most `fuel` iterations, else `OutOfFuel`. This outcome is interpreter-only: it fails the test unless the file says `@expect "<case>" error(OutOfFuel)`, and either way that case is never sent to a backend, which has no fuel. `Repeat` costs nothing. Recursion and cyclic consts/inputs are rejected by the checker. Every function, const, input and unit has a **rank** (its depth in the dependency DAG), and every call/read checks that the rank decreases. |
| Edge semantics | Java semantics, normative for every backend: wrapping integer arithmetic, masked shift amounts, saturating float→int casts with NaN→0. Integer `/` and `%` by zero is an error. |
| Short-circuit | `And`/`Or` are lazy. Everything else is evaluated left to right, and the first error wins. |
| Typing | Strict: operands must have identical types, with one exception: `Vec3 ∘ Pos3` / `Pos3 ∘ Vec3` for `+ - * /`. |
| Float comparison | Bit-exact for every backend, including `-0.0` vs `0.0`. NaN is a class: payloads are unspecified and unobservable. CUDA builds with strict IEEE flags (§9.3). |
| Transcendentals | Not primitives. Written as IR helper functions in a prelude, so every backend compiles the same `+ - * /`. |
| min/max | Java `Math.min`/`max`: NaN propagates, and `-0.0 < 0.0`. |
| Bitcasts | Only canonicalizing `float_to_bits` / `double_to_bits` (Java `floatToIntBits` / `doubleToLongBits`, all NaN → canonical) and `bits_to_float` / `bits_to_double`. |
| Externs | A test-owned registry with one implementation per target. Tests can only use registered externs. Extern types are registry-declared records. |
| Permutation tables / samplers | A toy deterministic builder from the registry, identical in all targets. Real noise correctness stays with tungsten-mc. |
| Evaluation model | **Eager**, independent of `Interface` scheduling. Every const is evaluated once per case before any entry; one failing const fails every entry cell. Before a unit is evaluated in a context, every input it declares is built over all of its cells, read or not; a failing input fails every cell of that unit. Buffers are memoized per `InputId`. Out-of-bounds is an error. |
| Ambient variables | IR change: `Expression::Ambient(AmbientKind)` replaces the four `Named` default variables. |
| Scale keys | Merging two scales through fixed-point collision is a bug. There's a regression test for it, marked `@knownfail` until fixed. |
| Text format | Lenient parser, canonical printer. `parse(print(m)) ≅ m` over compacted arenas, and `print ∘ parse` is a fixpoint on printed text. Not a bijection: comments and whitespace are free. |
| Float literals | Shortest round-trip decimal with type suffix, `inf`/`-inf`, and `nan(0x…)` with explicit bits. |
| Test file | Program + `@case` + optional `@expect` + `@knownfail`. Without `@expect`, the interpreter is the oracle. |
| Known failures | `@knownfail(backend, "reason")` in the test file. A knownfail that passes is an error. |
| Execution | Out-of-process driver per backend. One batch per backend, top-level corpus directory and stage, with a per-test rebuild if a batch fails. One process per (test, stage), binary request/response protocol. |
| Backend selection | `TW_BACKENDS` picks backends (default `rust,java`, plus `cuda` when `nvcc` and a GPU are detected). A missing toolchain means ignored tests with a notice, unless the backend is listed in `TW_REQUIRE`, which makes it a failure. |
| Build failures | A `generate` panic or error, or a failed build, fails every case of that test, including cases where the interpreter errors. |
| Grid sizes | Small (≤ 4×4×4) by default. `@tier(large)` runs only with `TW_LARGE=1`. |
| Test levels | Hand-written corpus, differential, optimizer equivalence, lowering and round-trip. Random well-typed programs (L6) and interface property tests are out of scope for now (except the scale-key regression). |
| CI gate | Rust + Java execution of the default tier. CUDA and Verus run locally only. |

## 3. Crate layout

```
crates/tungsten-parse/         # implemented, tests pass
  src/lib.rs          # load(src, file, &dyn Env) -> Result<TestFile>, print / print_test_file
  src/{lexer,parser,ast}.rs  src/elaborate/  src/printer/   # layout and API: crates/tungsten-parse/SPEC.md
crates/tungsten-core/          # all inside verus!{}, depends only on vstd/builtin
  src/ir.rs           # core IR
  src/registry.rs     # registry types: extern + extern-type signatures (data, filled by the caller)
  src/value.rs        # value domain, EvalError/StuckReason, Case, trait Externs, value_has_type
  src/util.rs         # bounds-checked Vec access, clone_* for the recursive types
  src/layout.rs       # len / flat / decode, overflow-checked
  src/prim.rs         # trusted int/float/cast primitive layer (external_body)
  src/check/          # checker: wf twin, types, flow, scope, acyclic (ranks); later well_typed
  src/exec/           # executable interpreter; later refinement proofs
  src/spec/           # (not started) spec semantics (spec fn)
  src/proofs/         # (not started) type soundness, fuel monotonicity, layout lemma
crates/tungsten-check/         # plain Rust, unverified
  src/registry.rs     # Registry, RegistryBuilder (names + decls)
  src/lower/          # OptimizedProgram -> core IR
  src/check.rs        # checker errors → Location; Invalid
  src/eval.rs         # prepare, Prepared::eval, evaluate (lower → check → eval)
crates/tungsten-testsuite/     # Rust, Java and CUDA differential done; full layout in SPEC.md §2
  src/harness/        # batches and cache, protocol, processes, driver generators
  src/registry.rs     # registry.toml loader
  src/env.rs          # tungsten_parse::Env + tungsten_check::Registry
  src/externs.rs      # Externs adapter over runtime/rust, for the interpreter
  src/{oracle,compare,knownfail}.rs  # interpreter runs, scoring, knownfail reconciliation
  src/gen.rs          # random well-typed program generator (proptest); out of scope for now
  registry.toml       # extern + extern-type declarations
  prelude/prelude.tw  # sin, cos, exp, lerp, smoothstep, ... in IR
  runtime/{rust,java,cuda}/
  corpus/<dir>/**/*.tw  # hand-written tests; each top-level dir is one batch
  corpus/regressions/ # shrunk failures from the generator (out of scope for now)
  tests/{roundtrip,lowering,corpus,random}.rs  # libtest-mimic, harness = false; random.rs is out of scope for now
  tests/snapshots/    # insta snapshots for L2
scripts/verify.sh     # runs Verus over tungsten-core (one invocation)
```

Dependencies:

```
tungsten-core (verified, Verus crates only)
      ▲
tungsten-check (registry, lowering, prepare/evaluate) ──▶ tungsten-ir
      ▲
tungsten-testsuite ──▶ tungsten-parse ──▶ tungsten-ir, and every backend
```

`tungsten-core` doesn't depend on `tungsten-ir`, so Verus never sees arenas, `Ref` retagging or
the lowering, and the trust boundary is the crate boundary. Keeping the checker and the
interpreter in one crate lets the type soundness proof (§7.2), which needs `well_typed`, the
spec semantics and value typing together, live in one place without cross-crate spec
export/import. Verus can still verify one module at a time while iterating. If proof times ever
force a split, it can happen later.

The registry is an input, not a dependency: `tungsten-core` defines the declaration types
(extern and extern-type signatures, domain errors), and the core `Program` holds them, so the
checker takes only the program. `tungsten-check` fills them from a `Registry` (the testsuite
builds one from `registry.toml`).
Exec-side extern implementations are supplied through the `Externs` trait (§7.3); the testsuite
implements it over its shared Rust runtime (§10).

Verus crates (`vstd`, `builtin`, `builtin_macros`) come in as `[workspace.dependencies]`, pinned
to one Verus release.

## 4. Required changes outside the new crates

Status as of 2026-11-01 is in brackets.

1. [done] **`Expression::Ambient(AmbientKind)`** in `tungsten-ir`, with `AmbientKind = Origin | Pos3 |
   OriginScale | PositionScale`. It carries no `Ref`, so `retag.rs` is unaffected. Remove
   `declare_default_parameters` from legacy conversion and emit `Ambient` reads instead. Every
   backend and inspect lowers `Ambient` to `config.origin`, `p.pos`, and so on. Ambients are
   read-only. A legacy body that assigns to one of the four names gets a real local of that name,
   initialized from the ambient (`water_levels` in `tungsten-mc-compile` does this with `pos3`).
2. [done] **`compact()` and structural equality** in `tungsten-ir` (`compact`, `isomorphic` and
   `permute` are in `rebuild.rs`, next to `retag.rs`, and reuse its walker).
   `compact` removes holes and resets generations (it renumbers refs). Structural equality
   (`isomorphic`) ignores tags, compares arenas in iteration order, and compares floats by bit
   pattern.
   The derived `PartialEq` on `Expression` makes `Float(NaN) != Float(NaN)`, so it can't be used
   for this.
3. [open: `tungsten-rust/src/types.rs` still derives `I32 ∘ Bool` and has no shift/bitwise cases] **`derive_binop_type`** (shared by rust/java) matches §5.3: shifts and bitwise ops become
   derivable, and `I32 ∘ Bool` is dropped.
4. [open] **Backend conformance** with §5 (wrapping ops, shift masks, cast helpers, Java min/max,
   short-circuit). This is tracked through `@knownfail` and isn't a prerequisite for the suite.
5. [not checked] **`Interface` scale keying** on exact `f64` bits. Tracked by the scale-key regression.
6. [done] **`entry_method_names`**: public in `tungsten_interface::naming`, extracted from the Rust
   and Java orchestration, so generated drivers can call `orchestrate_<entry>`. It takes a third
   parameter, `used: HashSet<String>`, which each backend seeds with the names already taken (Rust:
   `"new"`, Java: `rules.used_set()`): `entry_method_names(program, rules, used) -> Vec<String>`.
   CUDA uses the sibling `entry_base_names(program, rules, used)`, which returns the unit base names
   without the `orchestrate_` prefix (`enqueue_<base>` and `result_<base>` derive from them).
   Generated output is unchanged (diffed for Rust and Java).

## 5. Semantics (normative)

### 5.1 Values

`Bool`, `I8`, `I32`, `I64`, `F32`, `F64`, `Vec3 {x,y,z: f64}`, `Pos3 {x,y,z: i32}`,
`Array(T, n)`, `Record(name, fields)` for registry extern types, `PermTable(handle)` and
`Sampler(handle)`. `DensityInput` never appears as a value; it only exists as a read
(§6). `UnaryOperator::Ref(_)` is the identity on values.

### 5.2 Outcomes

`eval : Program × Case × Fuel → (per entry, per cell: Ok(value) | Err(e), failures)` with
`e ∈ { DivByZero, OutOfBounds, OutOfFuel, ExternDomain, Stuck(_) }`. Results are **per cell**:
the first error inside a cell ends that cell only. `Stuck` is a per-cell error that is
unreachable for checker-accepted programs and cases (§7). `failures` lists every failing const
and input once, as diagnostics. Evaluation is left to right: call arguments, binary operands
(except `And`/`Or`, which short-circuit), construct and array-literal elements, record fields in
written order. The error choice is canonical: lowest failing `ConstId`, then lowest failing
`InputId` of the unit, then the first cell in flat order, then left to right inside the cell.

### 5.3 Typing (checker)

- Arithmetic `+ - * /`: identical numeric operand types, or the `Vec3`/`Pos3` exception. In the
  exception, each `Pos3` component is converted i32→f64 exactly and the op is applied to each
  component in f64. The result is `Vec3`.
- `%` isn't in the IR. Comparisons need identical scalar types. `Equal`/`NotEqual` aren't defined
  on `Vec3`/`Pos3`/arrays/records.
- `And`/`Or`: `Bool` only. `BitAnd`/`BitOr`/`BitXor`: identical integer or `Bool` types.
- Shifts: left operand integer, right operand `I32`.
- `Not`: logical on `Bool`, bitwise complement on integers. `Negate`: numeric types.
- `ExplicitCast`: between all numeric types, `Bool → integer` (0/1), `Pos3 → Vec3`. No other
  casts.
- `Construct`/`ConstructExtern`: `ConstructExtern` needs an `Extern` type whose registry fields
  it lists. The parser accepts any type there (and repeated `parameters`). A non-`Extern` type
  or an unknown field name is a lowering error (`crates/tungsten-check/README.md`); missing or
  repeated fields are checker errors.
- Grids: every `dimensions` component of an entry point or density input is at least 1. The
  parser accepts any `i32`.
- Statements: every variable is definitely assigned before it's read, every path returns,
  `Break` only appears inside `While`/`Repeat`, the call graph is acyclic, constants are acyclic,
  and every extern and extern type is in the registry with matching signature. Flow analysis is
  conservative: loops never contribute assignments or "always returns" (the error carries a
  hint when a loop is the reason). Dead code after `return`/`break` is allowed.
- Vectors: `Vec3∘Vec3` and `Pos3∘Pos3` for `+ - * /` componentwise (i32 wrapping for `Pos3`,
  the first zero divisor in x, y, z order is `DivByZero`), negation of both. No scalar
  broadcast. `== !=` on `Bool` and numeric types only, `< <= > >=` on numeric types only.
- Indices (`Index`, explicit `Density` index) are `I32`.
- A const init contains no `Local`, `Call`, `Density`, `Ambient`, `Host`, `ExternCall` or
  `Table`. A unit's `main` has no params and any return type.
- Grids: additionally `dx*dy*dz ≤ i32::MAX`.
- Scoping, for every unit `U` and every `f ∈ F(U) = {U.main} ∪ U.helpers`: (a) calls stay in
  `F(U)`; (b) hosts read are in `f.hosts ⊆ U.hosts`; (c) tables in `U.tables`; (d) consts, and
  every const their inits reach, in `U.consts`; (e) density reads only of inputs in `U.inputs`.
  A function listed by several units is checked against each of them.

### 5.4 Integer and float operations (Java)

- `+ - *` wrap modulo 2^width. `/` truncates toward zero. `MIN / -1 = MIN`. `/ 0` is
  `DivByZero`.
- Shift amount is masked to `width-1` (31 for i32, 63 for i64). `>>` is arithmetic and
  `LogicalShiftRight` is logical.
- `I8` operations: sign-extend to i32, apply the i32 rule, truncate to i8. This is exactly Java's
  `byte` promotion, including `>>>` on negative bytes.
- Float→int: round toward zero, saturate at bounds, NaN→0. Int→float and f64→f32 round to
  nearest even. Integer narrowing truncates.
- Floats follow IEEE-754 binary32/binary64, with each operation rounded to its own type and no
  contraction into FMA. Comparisons follow IEEE, so NaN compares false except with `!=`.
- `fma` exists only as an explicit registry extern.

### 5.5 Statements

`Repeat { count }` runs the body `count` times. `Break` leaves the innermost `While`/`Repeat`.
`Return` leaves the function. One `While` execution may run at most `fuel` iterations (fuel is
per loop, not shared). `Repeat` costs no fuel.

## 6. Entry and density-input evaluation

### 6.1 Grid and positions

An evaluation context is `Config { origin: Vec3, origin_scale: Vec3, position_scale: Vec3,
dimensions: (i32,i32,i32) }` plus a cell `c ∈ [0,dx)×[0,dy)×[0,dz)`. `Ambient(Pos3)` is `c` (the
local grid coordinate), and the other ambients are the `Config` fields. World positions are
computed by the program itself (`origin*origin_scale + pos3*position_scale`), not by the
interpreter.

An entry point is evaluated for every cell of its `dimensions`, with `origin` from the `@case`
and the scales from the entry. A `DensityInput d` is evaluated with the same `origin` and
`d.scaled_origin`, `d.scaled_position`, `d.dimensions`.

### 6.2 Layout

`flat(c, dims) = c.x + c.y*dx + c.z*dx*dy`, with x varying fastest. This matches `as_index` and
`Iter3D` in tungsten-mc. `decode(i, dims)` is its inverse on `[0, dx*dy*dz)`.

### 6.3 Reads

- `DensityVariable(d, None)`: if `d.dimensions == (1,1,1)`, read cell `(0,0,0)` of `d`.
  Otherwise, take the caller's flat index `flat(c, caller_dims)` and read cell
  `decode(i, d.dimensions)` of `d`. This mirrors `buffers.d[index(it)]`. An index outside `d`'s
  grid is `OutOfBounds`.
- `DensityVariable(d, Some(e))`: `e` evaluates to an integer index. A negative index or one
  ≥ `len(d)` is `OutOfBounds`. Otherwise, read cell `decode(e, d.dimensions)`.
- Buffers are built eagerly (§2 Evaluation model) and memoized per `InputId`: a buffer's value
  depends only on the input, never on the reader. Memoization is an exec-only optimisation, and
  (later) proven equal to the unmemoized spec; it is tested now (memo on == memo off).

Lemma (verified): `decode(flat(c, dims), dims) == c` for every `c` in range, and
`flat(decode(i, dims), dims) == i`.

## 7. Verification

### 7.1 Structure

- `spec fn eval_*` define the semantics over the core IR with explicit fuel.
- Exec `fn eval_*` have `ensures result == spec_eval_*(…)`. The memo invariant is per id:
  `∀ c ∈ memo.consts. memo[c] == spec_eval_const(p, case, fuel, c)`, and likewise for
  `memo.inputs` with `spec_build_input`.
- `check` has `ensures result.is_ok() ⇒ well_typed(p)`.

### 7.2 Theorems

1. **Refinement**: exec interpreter == spec, for every fuel.
2. **Type soundness**: `well_typed(p)` ⇒ spec evaluation never returns `Stuck`, and every `Ok`
   value has its declared type.
3. **Fuel monotonicity**, per cell: if fuel `f` gives a cell result other than `OutOfFuel`,
   every `f' ≥ f` gives the same result for that cell.
4. **Layout**: §6.3 lemma.

### 7.3 Trusted base

Verus itself. The primitive layer `prim.rs`: every integer, float and cast operation is an
`external_body` fn whose `ensures` (added in the proof phase) equates it to an uninterpreted
`spec fn`. Externs are supplied through the `Externs` trait
(`call(&self, ExternId, &[Value]) -> Result<Value, ExternFail>`), which gets an uninterpreted
spec too; its results are checked against the decl at runtime. Everything else outside
`tungsten-core` (the lowering in `tungsten-check`, the parser, the harness and the runtimes) is
tested, not proven. Kani harnesses for the lowering
and for cast/shift edge cases are possible later work.

## 8. Text format

The full format (grammar, elaboration, printer, envelope) is specified in
`crates/tungsten-parse/SPEC.md`, which supersedes the syntax below where they differ.

### 8.1 Properties

- `parse` is lenient: free whitespace, `//` comments, and derivable fields may be omitted (extern
  parameter and return types come from the registry, `Field` types from the base type,
  `source_hash` defaults to 0).
- `print` is canonical and fully explicit: 4-space indent, fully parenthesized binary ops,
  explicit types everywhere.
- Required: `parse(print(m)) ≅ compact(m)`, and `print(parse(print(m))) == print(m)`.
- Variable, host and input names: `Named(s)` prints as `s`, `Prefixed(s)` as `s#N`, and `Anonymous` as `%N`,
  where `N` is the compacted slot index.
- Floats: shortest round-trip decimal with a mandatory `.` (`1.5f`, `-0.0`, `2.0`), `inf`,
  `-inf`, `inf_f32`, and NaNs with their full bits: `nan_f32(0x7fc00000)` /
  `nan(0x7ff8000000000000)`. Integers: `1b`, `1`, `1L`.
- Required only within the nesting limit (256 levels of expressions, blocks, types and values;
  SPEC §6.2). Deeper programs print, but the text doesn't load.

### 8.2 Test file

```text
// Description and comments are free text.
@knownfail(cuda, "f2i cast is UB in device code")

program {
    host scale: f64;
    table t0 = perlin("minecraft:offset", none, 0);
    const half = 0.5;

    fn lerp(a: f64, b: f64, t: f64) -> f64 {
        return (a + (t * (b - a)));
    }

    fn base_main() -> f64 @hosts(scale) {
        let x: f64 = ((ambient(pos3) as vec3).x * host(scale));
        return lerp(x, half, 0.25);
    }

    unit base {
        main base_main;
        tables [t0];
        hosts [scale];
        helpers [lerp];
        consts [half];
    }

    input b = base @dims(4, 1, 4) @origin_scale(1.0, 1.0, 1.0) @position_scale(4.0, 1.0, 4.0);

    fn top_main() -> f64 {
        return (density(b) * 2.0);
    }

    unit top {
        main top_main;
        inputs [b];
    }

    entry top @dims(4, 1, 4) @origin_scale(1.0, 1.0, 1.0) @position_scale(4.0, 1.0, 4.0);
}

@case "origin" origin(0, 0, 0) param(seed = 1) host(scale = 2.0)
@case "far"    origin(1000000, -64, 3) param(seed = 1) host(scale = -0.0)
@expect "origin" top[0, 0, 0] = 0.25
@expect "far" error(OutOfBounds)
```

Every fn and every `input` is top level, and a unit names its main fn (`main f;`). Lists (`helpers`, `tables`,
`hosts`, `consts`, `inputs`) are explicit and never derived. Simple statements and items end in `;`.

Other directives: `@fuel(n)`, `@tier(large)`, and `@cells(sample = n, seed = s)` to compare only
a sample of cells on large grids.

## 9. Harness

### 9.1 Flow per test file

1. `load(src, file, env)` (the testsuite supplies the registry and the prelude as the `Env`), then check the round-trip (§8.1).
2. Lower and check. A rejected program is a test failure, unless the file says
   `@expect reject`.
3. Interpret every case. If `@expect` is present, the interpreter must match it (this tests the
   ground truth itself).
4. For each backend: `compile`, then run the driver and compare (§9.4).
5. Run `perform_inline`. The interpreter result on the inlined program must equal step 3, and
   the backends rerun on the inlined program.
6. Reconcile with `@knownfail`: an expected failure that passes is an error.

### 9.2 Drivers

Generated code, the test runtime, generated records, and a generated driver per test (each test
in its own module/package) go into `$CARGO_TARGET_TMPDIR/tw-cache/<backend>/<hash>/`. A batch is
(backend, top-level corpus directory, stage). It builds once with `rustc` / `javac` / `nvcc`, and
the build is cached by a hash of the sources, runtime, flags and compiler version. A filter that
selects any test in a batch builds the whole batch. If a batch build fails, each of its tests is
built alone, so a compile error only fails its own test.

There is one process per (test, stage), with a fresh `Orchestration` per case. Protocol:
little-endian binary on stdin and stdout, request/response per case. A case goes in as origin
(3×f64), the auto params in declaration order, and host inputs as raw bits in declaration order. The result comes back as a
status byte, then either every entry's cells as raw bits, or an error kind and message. A driver
responds only after the whole case succeeds. A watchdog (`TW_TIMEOUT_SECS`, default 10 per case) or
a crash scores that case as a backend failure, and the run resumes from the next case in a new
process.

### 9.3 Backend build flags

- Rust: direct `rustc --edition 2024 -C opt-level=3 -C overflow-checks=on -C panic=unwind
  --cap-lints allow`. A non-wrapping op in generated code panics and shows up as a mismatch. A panic
  maps to an error. Capping lints lets a literal `1 / 0` in a body reach runtime; a failing Rust
  `const` is still a compile error, which counts as a build failure (§9.4).
- Java 21: FP is strict by default. `ArithmeticException` maps to `DivByZero` and
  `ArrayIndexOutOfBoundsException` to `OutOfBounds`. Any other `Throwable` is also an error,
  including `ExceptionInInitializerError` from a failing constant.
- CUDA: `-fmad=false -prec-div=true -prec-sqrt=true -ftz=false`, no `--use_fast_math`. Cases
  whose interpreter result is an error aren't compared on CUDA, since it can't trap reliably.

### 9.4 Comparison

Backends are compared per case. If no interpreter cell has an error and there are no
`failures`, every backend value must equal the interpreter value bit for bit, and any two NaNs
are equal. Otherwise the backend must fail the case (any error, panic, exception or watchdog);
the kind appears in the report but is never compared, so the `Interface` may schedule freely
(the interpreter's canonical error choice is semantics, not a schedule). This also holds for a
failing const or input that nothing reads: backends must evaluate every listed const, and
`@knownfail` tracks the ones that don't yet.

A case where the interpreter reports `OutOfFuel` isn't run or scored. A `generate` failure or a build
failure fails every case of the test, whatever the interpreter says, so a real compile bug can't
pass unnoticed in an error-only test.

On a mismatch, the report shows the entry, the first differing cell and how many others differ, and
both values in hex and decimal. Future work: a first-diverging-decision report, probably by an IR
pass that makes each `If`/`While` condition call a trace extern, so verified code stays untouched.

## 10. Registry and prelude

- **Primitive externs**: `sqrt`, `floor`, `ceil`, `trunc`, `abs`, `min`, `max` (Java), `copysign`,
  `fma`, each with a `_f32` variant (names are unique, with no overloads); `float_to_bits(f32) ->
  i32` and `double_to_bits(f64) -> i64` (canonical); `bits_to_float`, `bits_to_double`;
  `perm_get(PermTable, i32) -> i32`; `sampler_sample(Sampler, Vec3) -> f64` (toy, integer-hash
  based, exact); and `test_domain(i32) -> i32`, which fails for negative input so `ExternDomain` is
  reachable. Each one is declared in `registry.toml` with its signature and any domain error.
- **Implementations**: hand-written in each runtime. The Rust source is shared: the interpreter's
  `Externs` adapter includes `runtime/rust/`, and it's also copied into generated builds. A shared
  bug can't show up as interpreter vs Rust, so Java comparison and hand-computed `@expect` files
  cover it.
- **Toy builders**: defined over the arguments generated code already passes, since it never sees
  the name strings. A Perlin table folds splitmix64 over `(seed, id_lo, id_hi, subident_index,
  sub_lo, sub_hi)`, with the ids given as literal `i64` initializer arguments (the corpus files write them out); a Base3D table is `splitmix64(seed)`, so all
  Base3D tables of one seed are identical. `perm_get` reads `perm[i & 255]` and has no domain error.
  Full definition in `SPEC.md` §5.3. Each language has an identical implementation.
- **Extern types**: declared records with typed fields, with definitions generated per language
  from `registry.toml`. `ConstructExtern`/`Field` work on them structurally.
- **Prelude** (`prelude.tw`): `sin`, `cos`, `exp`, `log`, `pow`, `lerp`, `smoothstep`,
  `as_index`, `corner*` helpers, all written with basic operations. A test pulls them in with
  `use prelude::{sin, lerp}`, which adds them as helper functions.

## 11. Test levels

| Level | What | Where |
| --- | --- | --- |
| L0 | Unit tests of the parser, checker, interpreter and lowering | each crate |
| L1 | Round-trip (§8.1) over the corpus and generated programs | `tests/roundtrip.rs` |
| L2 | Lowering: `insta` snapshot of the core IR for the corpus (regressions excluded), plus invariants (types and arity preserved, every ref resolved, `lower` deterministic). A lowering bug also shows up indirectly as a differential mismatch | `tests/lowering.rs` |
| L3 | Interpreter vs `@expect` (hand-computed) | `tests/corpus.rs` |
| L4 | Interpreter vs each backend | `tests/corpus.rs` |
| L5 | Optimizer equivalence (§9.1 step 5) | `tests/corpus.rs` |
| L6 | **Out of scope for now; not implemented.** Random well-typed programs (proptest, shrinking). A fixed seed and a small count run in the normal suite; `TW_FUZZ=<n>` runs longer locally. Gating below | `tests/random.rs` |

L6 gating (design for later, out of scope for now): interpreter-side checks (checker accepts, no `Stuck`, round-trip, memo on == off, inlined
interpreter == original) always fail the run. Backend mismatches only fail it for backends listed in
`L6_STRICT`, which starts empty; a backend is added once it conforms. For other backends, mismatches
are printed, and are shrunk and written only when `TW_FUZZ` is set.

Programs are sampled and built in one batch. A failure is shrunk against only the failing backend and
stage, within a time budget (`TW_SHRINK_SECS`, default 120). Locally it's written to
`corpus/regressions/<content-hash>.tw` without a `@knownfail`. When `CI` is set, it goes to the log
and a workflow artifact instead.

Required corpus coverage, at least one file each: every operator × every type, overflow and
shift masks, every cast edge case (NaN, ±inf, out-of-range), `-0.0`, short-circuit with an
erroring right side, break/return inside nested loops, `Repeat 0`, constants that reference
constants, density inputs at `(1,1,1)` / equal / different dims, explicit-index corner reads at
the grid edge (OOB), the scale-key collision, each extern with hand-computed `@expect`,
`@expect error(OutOfFuel)`, an unread failing const, and a failing input that nothing reads.

## 12. CI

GitHub Actions with JDK 21: `TW_BACKENDS=rust,java TW_REQUIRE=rust,java INSTA_UPDATE=no cargo test
--workspace` runs L0–L5 on Rust and Java, so a missing toolchain fails the job rather than being
skipped. CUDA runs locally only, and so does Verus (`scripts/verify.sh`).

Status: deferred. `test.yml` was written in milestone 3 and then removed on purpose, so there is no
CI workflow in the repo yet. The command above is what it ran.

## 13. Open items

- Confirm in tungsten-mc that every density input of an entry gets the entry's `origin` (§6.1).
  (`Vec3` is f64 for the Rust backend: generated code imports `crate::mathf64`.)
- Error-kind mapping for Rust panics (needs identifiable panic messages from the generated
  code), or compare only error vs non-error.
