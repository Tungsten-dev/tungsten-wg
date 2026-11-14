# Select: `select(c, a, b)`

Status: implemented (see §10). Adds one expression kind across IR, builder, parser, core, lowering and every backend.

## 1. Summary

`select(c, a, b)` is `a` when `c` is `true` and `b` otherwise. Unlike `if` (a statement) and C/Java's `?:` (lazy),
select is an **expression** whose operands are **all evaluated**, left to right. Because both arms are always
evaluated, a backend can always lower it to a branchless instruction:

| Target | Emitted code | Expected machine code |
| --- | --- | --- |
| Rust | `::core::hint::select_unpredictable(c, a, b)` | `cmov` (x86-64), `csel` / `fcsel` (aarch64) |
| CUDA | `tw_select(c, a, b)`, a `__forceinline__` helper with `c ? a : b` | PTX `selp`, SASS `SEL` / `FSEL` |
| Java | `tw$select(c, a, b)`, a `private static` helper with `c ? a : b` | C2 inlines it, and may emit `cmov` |

### Goals

- A frontend can write a data-dependent choice as one expression, with no `if`, temporaries or divergent branch.
- Every backend emits it so the target compiler has every reason to go branchless. Eager operands come from
  function-call argument evaluation, so no backend has to hoist statements.
- Results match the interpreter bit for bit (NaN excepted, as everywhere), including which cases fail (§4.2).

### Non-goals

- `ConstExpr::Select`. Select in a constant is an error (D5).
- An optimizer pass that rewrites `if`/assign patterns into selects (D6, deferred).
- *Guaranteeing* machine instructions. We emit the strongest portable hint per target. Tests check the emitted
  source, not the assembly (D7).
- Records and arrays as operands (D3).
- Lazy semantics or a `?:` surface syntax (D1, D4).

## 2. Example

```text
unit clamp_floor {
    fn main() -> f64 {
        let d = noise3(base_table, vec3(f64(pos3.x), f64(pos3.y), f64(pos3.z)) * position_scale);
        let floor = f64(pos3.y) * -0.01;
        return select(d < floor, floor, d);   // max(d, floor) without a branch
    }
}
```

## 3. Data model

### 3.1 IR (`crates/tungsten-ir/model.rs`)

```rust
pub enum Expression {
    // existing variants
    /// `then` if `cond` is true, else `otherwise`. All three are evaluated, in this order (eager).
    Select {
        cond: Box<Expression>,
        then: Box<Expression>,
        otherwise: Box<Expression>,
    },
}
```

- Add it **after** `Eval`, so existing discriminants (and any source hashes derived from them) stay the same, as with
  `LogicalShiftRight`.
- It holds no `Ref`s. `retag.rs` and `rebuild.rs` only recurse into it.
- Typing (shared by the builder, the core checker and the backends' type derivation):
  - `cond: Bool`, with no int truthiness.
  - `then` and `otherwise` have the **same** type `T`, where `T ∈ {F32, F64, I8, I32, I64, Bool, Vec3, Pos3}`. There is
    no promotion: `select(c, vec3, pos3)` and `select(c, 1i32, 1i64)` are type errors.
  - The result type is `T`.

### 3.2 Core IR (`crates/tungsten-core`)

```rust
pub enum Expr {
    // existing variants
    Select { cond: Box<Expr>, then: Box<Expr>, otherwise: Box<Expr> },
}
```

- `wf.rs` / `check/wf.rs`: recurse into the three children.
- `check/types.rs`: the rule from §3.1, with new error kinds `SelectCond { found }` (cond is not `Bool`),
  `SelectMismatch { then, otherwise }` (arm types differ) and `SelectType { found }` (record or array arm).
  Document them in CHECK.md §4.1 and §10.
- `exec/expr.rs`: evaluate `cond`, then `then`, then `otherwise`, stopping at the first error, then pick one.
  Termination is structural, there's no fuel, and there are no new `EvalError`s or `StuckReason`s (a non-`Bool` cond
  is `Stuck(Shape)`).
- `util.rs`: extend `clone_expr` and the other per-variant helpers.
- SEMANTICS.md §5: a new row in the expression table, and a sentence beside the short-circuit note saying that
  select is **not** lazy, unlike `&&`/`||`.
- Verus: a new variant in an existing recursive match. `scripts/verify.sh` must still pass with no new
  `external_body`.

### 3.3 Lowering (`crates/tungsten-check/src/lower/expr.rs`)

- `Expression::Select` maps one-to-one to `core::Expr::Select`. There's no desugar (D2).
- Every walker in tungsten-check recurses into it: the `unit_eval` desugar, `map.rs`, `refine.rs`, `step.rs`
  and `mapping.rs`, wherever expressions are walked.
- A select inside a constant's `ConstExpr` can't be built (§3.4, §3.5), so lowering doesn't need to reject it.

### 3.4 Builder (`crates/tungsten-builder`)

```rust
/// `select(c, a, b)`: `a` if `c`, else `b`. All three are evaluated.
#[track_caller]
pub fn select(c: impl Into<Expr>, a: impl Into<Expr>, b: impl Into<Expr>) -> Expr;
```

- It's a free function in the prelude, typed when built. A wrong cond type, mismatched arms or a record or array
  arm adds a `BuildErrorKind::{SelectCond, SelectMismatch, SelectType}` problem at the caller's location, and
  poisoning works as it does for `binary`. When the arms' types are unknown, the result type falls back to the
  known arm's type.
- `derive.rs` recurses into it, so unit lists, density inputs and hosts read inside either arm are derived as
  usual. **Both** arms count as read, which is correct under eager semantics.
- Constants: the builder's const path rejects an `Expression::Select` it can't convert to a `ConstExpr`, as it does
  for other non-const nodes.

### 3.5 Text format (`crates/tungsten-parse`, its SPEC.md §4, TESTING.md §8)

```
select_expr = "select" "(" expr "," expr "," expr ")" ;
```

- `select` becomes a **reserved word**, so an extern or function named `select` is a parse error. Check the
  corpus and the testsuite registry for collisions.
- It's a primary expression, so it adds no precedence level.
- The printer emits `select(c, a, b)` in canonical form, and print → parse round-trips it.
- In a `const` initializer it's an elaboration error ("select is not allowed in constants").

### 3.6 Interface and optimizer

- `tungsten-interface`: every expression walker (eval-site collection in `eval.rs`, reached-set scheduling,
  workers, refine and scale helpers) recurses into `Select`. An `eval(...)` inside an unselected arm still
  counts as reached and is scheduled, because it runs.
- `tungsten-optimizer/src/inline.rs`: `visit_expr` and the mutable visitor recurse into it. Inlining a density read
  that appears in an arm is allowed. No select-specific folding (D6).

## 4. Behavior

### 4.1 Semantics

1. Evaluate `cond`. If that fails, the select fails with that error.
2. Evaluate `then`. If that fails, the select fails, **even if `cond` is `false`**.
3. Evaluate `otherwise`. If that fails, the select fails, **even if `cond` is `true`**.
4. The result is `then` if `cond` is true, otherwise `otherwise`.

Selecting returns the value's bits unchanged. A NaN, a `-0.0` or a NaN payload in the chosen arm is returned
exactly. (This is why select can't be lowered as `c*a + (1-c)*b`.)

### 4.2 Edge cases

| Case | Behavior |
| --- | --- |
| Unselected arm divides an int by zero, indexes out of bounds, or hits an extern domain error | The cell fails with that error. Backends must also fail (SEMANTICS.md §10.4), which they do because the arm is a call argument and really runs. |
| Both arms fail | The `then` error is reported (left to right). Backends may report a different one (§10.4 doesn't compare which). |
| `cond` is a literal | Still evaluates both arms. No folding (D6). |
| Nested selects | Allowed, evaluated inner-first by argument order. Nesting depth only affects emitted line length. |
| Select inside a `while` condition | Fine. It's an ordinary expression. |
| `Vec3` / `Pos3` arms | `cond` is evaluated once. The result is the whole chosen vector (§5). |
| Bool arms | Allowed. `select(c, x, false)` is *not* rewritten to `c && x`, because that would make `x` lazy. |
| Density read with an out-of-range index in an arm | Fails under eager semantics. Frontends that guard a read with a select must clamp the index instead. Document this in the builder README. |

## 5. Backends

All three backends get eager evaluation from **function-call argument evaluation**. Rust, C++ and Java all evaluate
arguments before the call, so no backend has to hoist statements. Rust and Java evaluate left to right. C++ argument
order is unspecified, but every fallible operation reports its error in a way that doesn't depend on order (§10.4),
and there are no side effects, so order doesn't matter.

### 5.1 Rust (`crates/tungsten-rust`)

- Emit `::core::hint::select_unpredictable(c, a, b)` as an `rcl::Expression::FunctionCall` on that path. This
  works for any `T`, including the Vec3/Pos3 structs.
- **The generated code needs Rust 1.88 or later.** Note this in the README and the generated crate's docs.
- `types.rs`: type derivation gives `Select` the type of `then`.

### 5.2 CUDA (`crates/tungsten-cuda`)

- Emit `tw_select(c, a, b)` as `cuda::Expression::Call`.
- Prepend overloads to the translation unit, only for the operand types the program uses:

  ```cpp
  __device__ __forceinline__ double tw_select(bool c, double a, double b) { return c ? a : b; }
  // float, signed char, int, long long, bool likewise
  __device__ __forceinline__ Vec3 tw_select(bool c, Vec3 a, Vec3 b) {
      return Vec3{ c ? a.x : b.x, c ? a.y : b.y, c ? a.z : b.z };
  }
  // Pos3 likewise
  ```

  The struct overloads select per component, so each component becomes its own `selp`, with no struct copy through
  memory.
- `tw_select` is reserved in the CUDA `NamingRules`.

### 5.3 Java (`crates/tungsten-java`)

- Emit `tw$select(c, a, b)` as a method call. Generate `private static` overloads on the generated class for the
  types the program uses (`double`, `float`, `byte`, `int`, `long`, `boolean`, `Vec3`, `Pos3`). The Vec3/Pos3 overloads
  return the chosen reference: the vectors are immutable, so no copy is needed.
- Don't use the model's existing `Conditional` (`?:`) directly for select, because it's lazy (D1).
- `size.rs`: count `Select` like a 3-argument call.
- `tw$select` is reserved in the Java `NamingRules`.

### 5.4 Inspect (`crates/tungsten-inspect`)

- IR text: `select(c, a, b)`.
- Dataflow, graph and inline analyses recurse into all three children, and both arms are edges (both are read).

## 6. Decisions

| # | Decision | Rationale | Rejected |
| --- | --- | --- | --- |
| D1 | **Eager** semantics: all operands are evaluated, and an error in either arm fails the cell | Branchless code is always legal, and the interpreter and backends agree on failure | *Lazy*: a cmov is then only legal when both arms are proven infallible, which needs an analysis and a branchy fallback. *Infallible arms only*: rejects too much (indexing, int division, domain externs) |
| D2 | Add `Expr::Select` to **core**, verified | Core expressions have no `let`, so a desugar would need statement hoisting, which is awkward in `while` conditions. One variant is a small Verus cost: structural, no fuel, no new errors | Desugaring to locals plus an `If` in tungsten-check |
| D3 | Operand types: scalars plus **Vec3/Pos3** (per component). Records and arrays are rejected | The hardware selects scalars, and vectors map cleanly to three selects. Wide types would need a copy or branch, which defeats the point | Scalars only (too limited for position math). Any type (no branchless lowering) |
| D4 | Syntax `select(c, a, b)` and a reserved word | Doesn't suggest the lazy `?:` meaning, and adds no precedence level | `c ? a : b`, or both |
| D5 | Not allowed in constants | Constants are evaluated once, so branchlessness doesn't matter there, and it keeps the surface small. Can be added later | `ConstExpr::Select` everywhere |
| D6 | Explicit only, no if→select pass and no folding | Keeps the feature small. A pass needs a cost and infallibility model | An optimizer pass in this feature |
| D7 | Tests check the **emitted source** only, not PTX or asm | The machine code is the target compiler's job, and asm checks break across compiler versions | PTX `selp` checks, x86 `cmov` checks |
| D8 | Rust: `core::hint::select_unpredictable` | Eager by construction and the strongest stable cmov/csel hint | Bound locals plus `if` (left to LLVM's heuristics). Inline asm (per arch, unsafe, blocks optimization) |
| D9 | CUDA: `__forceinline__` overloads with `?:` | Eager by being a call. nvcc/ptxas reliably emit `selp` on register operands, which works for every type including i8 | Inline PTX `selp` (no `.b8`, opaque to the optimizer). Hoisting temps (converter complexity) |
| D10 | Java: `private static` overloaded helpers | Eager by being a call, and C2 inlines them | A plain `?:` when the arms are infallible (needs an analysis, and it's lazy) |

## 7. Testing

- **Core** (`tungsten-core` unit tests and `scripts/verify.sh`): typing accepts every allowed `T` and rejects each
  error kind. Exec evaluates both arms: an erroring unselected arm fails the cell, both arms failing reports the
  `then` error, and a NaN or `-0.0` in the chosen arm comes back with its bits unchanged.
- **Builder** (`crates/tungsten-builder/tests/select.rs`): types, each `BuildErrorKind`, poisoning, `derive.rs`
  listing inputs from both arms, and a select in a constant rejected.
- **Parser**: round-trip print → parse, `select` reserved as an identifier, rejected in `const`, and the fuzz
  token list extended with `select`.
- **Check** (`crates/tungsten-check/src/tests/`): lowering is one-to-one, and an `eval` inside an arm is
  desugared and reached.
- **Corpus** (`tungsten-testsuite`, `corpus/select/`): differential tests against Rust, Java and CUDA covering
  every scalar type, Vec3, Pos3, nested selects, select in a `while` condition, a fallible unselected arm with
  `@expect error(...)` (OutOfBounds and DivByZero), NaN and `-0.0` arms, and an arm that does a density read
  and an `eval`.
- **Backend source tests**: Rust output contains `::core::hint::select_unpredictable(`. CUDA output contains
  `tw_select(` and only the overloads it uses. Java output contains `tw$select(` and its overloads, and never
  emits `?:` for a select.
- **Inspect**: IR text and dataflow edges for both arms.

## 8. Rollout

1. IR variant, all tungsten-ir walkers, builder `select`, parser and printer.
2. Core `Expr::Select`: wf, check, exec, docs, Verus run.
3. Lowering and the tungsten-check walkers, interface and optimizer walkers.
4. Backends: Rust, CUDA, Java, then inspect.
5. Corpus, backend source tests, READMEs (builder, check, parse SPEC, core SEMANTICS and CHECK).

## 9. Deferred / open

- An if→select optimizer pass (D6), with a cost and infallibility model, possibly with `cond` folding.
- `ConstExpr::Select` (D5).
- An optional PTX/asm smoke test if select is ever found not to lower branchless (D7).
- A lazy counterpart (`if`-expression) if frontends need guarded fallible reads. For now they clamp the index.

## 10. Implementation notes

- **Paths.** The three children use the existing `Arg(0..2)` steps (core `CoreStep` and
  tungsten-check `Step`), as for a three-argument call, instead of new `Cond`/`Then`/`Otherwise`
  steps. Errors in an arm render as `… › value › arg 1: …`.
- **Core typing details.** `SelectType` also covers handle arms, not just records and arrays. The
  three errors are reported at the select node. A bad cond doesn't poison the result (the arms'
  type is kept); mismatched or unselectable arms give no type. The builder follows the same rules.
- **Rust** emits `select_unpredictable` as an `rcl::Expression::LateBoundCall` (call by name),
  because `FunctionCall` needs a `FunctionRef` to a generated definition. The type derivation is
  `FunctionConverter::derive_type` in `converter.rs`, not `types.rs`. The Rust 1.88 note is in the
  root README, on `RustOutput`, and in the generated module's header comment.
- **CUDA** device types are `double3`/`int3`, so the vector overloads are
  `double3 tw_select(bool, double3, double3)` and `int3 …`, built per component with brace
  initialization. The overloads go first in the namespace, before lerp helpers and functions.
- **Overload resolution.** C++ promotes `signed char` and `bool` operands of some operators to
  `int`, and Java widens `byte` to `int`, which would make the CUDA overloads ambiguous or pick
  Java's `int` overload. So `i8` arms (CUDA and Java) and `bool` arms (CUDA) are cast back to their
  type unless they already are such a cast (or, on CUDA, a bool literal).
- **Java `size.rs`** needed no change: a select is emitted as a `java::Expression::Call`, which is
  already counted like any three-argument call.
- **Walkers with nothing to do.** `map.rs`, `refine.rs` and `step.rs` in tungsten-check, and
  `refine.rs`/`scale.rs` in tungsten-interface, don't walk expressions themselves. The walkers
  that do (`lower/mapping.rs`, `lower/unit_eval.rs`, interface `eval.rs` and `workers.rs`) recurse
  into all three children.
- **Tests.** The core typing/exec tests live in tungsten-check (`tests/eval.rs`,
  `tests/check_errors.rs`, `tests/normalization.rs`, `tests/unit_eval.rs`), which drives core
  from `.tw` text, as for every other core rule. The parser's random program generator and
  `eval_equivalence.rs`'s walker also produce and handle `Select`. The CUDA corpus trials
  couldn't run on the development machine (the CUDA runtime failed on `cudaMalloc` there for
  every program), but `nvcc` compiled every `corpus/select` program.
