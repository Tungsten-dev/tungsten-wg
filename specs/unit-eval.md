# Unit evaluation: `eval(unit, p)`

Status: proposed. Nothing implemented. Extends `specs/density-mappings.md` and `specs/refinement.md`, and changes
neither.

## 1. Summary

A density function (compute unit) already computes one value per position. Today the only way to use one is to
materialize it over a grid, as an entry or a density input, and read the buffer. This spec adds an expression that
evaluates a unit's body at a position the reader chooses, with no buffer behind it:

```text
eval(ore_noise, p) @origin_scale(1, 1, 1) @position_scale(1, 1, 1)
```

The result is the value the unit's main function returns when `Pos3` is `p`, `origin` is the run's origin, and the
scales are the ones written on the expression. The unit is computed only where it's read, so a small unit can be
probed over a space far larger than any buffer, including negative and off-grid positions.

### Goals

- A frontend can evaluate a unit at any `Pos3`, as often as it likes, without declaring an input or a grid.
- Every backend emits it natively (a Rust fn, a Java method, a CUDA `__device__` function), so there is no buffer,
  kernel or wave for the evaluated unit.
- Results match bit for bit across the interpreter and every backend.
- For positions on a grid, the result equals the buffered result of the same unit on that grid with the same scales
  (§7). This is the main correctness property.

### Non-goals

- Changing `tungsten-core` or its Verus proofs. The interpreter gets `eval` through a desugar in `tungsten-check`
  (§5.1).
- Caching or CSE. Two `eval`s at the same position compute the body twice (§4.5).
- A lazy mode on density inputs (`@lazy`, virtual grids, `density_at`). That was considered and dropped (§9, D1).
- Bounds. An evaluated unit has no grid, so it has no range to check (§4.3).
- Moving `tungsten-mc-compile` to `eval`. That's follow-up work.

## 2. Example

```text
unit ore_noise {
    fn main() -> f64 {
        let w = vec3(f64(pos3.x), f64(pos3.y), f64(pos3.z)) * position_scale + origin * origin_scale;
        return noise3(ore_table, w);
    }
}

unit vein {
    fn main() -> f64 {
        let s = 0.0;
        // Probe a 5×5 neighbourhood around the cell at y + 32, outside any grid `vein` runs on.
        repeat 5 (dx) { repeat 5 (dz) {
            s = s + eval(ore_noise, pos3(pos3.x + dx - 2, pos3.y + 32, pos3.z + dz - 2))
                    @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
        } }
        return s;
    }
}

entry vein @dims(16, 96, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
```

(`repeat` counters are written informally here. The point is that `ore_noise` never runs as a kernel: it has no
buffer, and is computed 25 times per `vein` cell.)

## 3. Data model

### 3.1 IR (`crates/tungsten-ir/model.rs`)

```rust
pub enum Expression {
    // existing variants
    /// Evaluate `unit`'s main function at `position` (`Pos3`), with these scales and the run's origin.
    Eval {
        unit: Ref<ComputeUnit>,
        position: Box<Expression>,
        origin_scale: (f64, f64, f64),
        position_scale: (f64, f64, f64),
    },
}
```

- **Inline, not an arena** (D4). Nothing lists eval sites. `Interface` and the analyses collect them by walking
  bodies, and dedup by `(unit, origin_scale bits, position_scale bits)`, which is the **eval key**.
- The type is the unit's main return type, records included.
- `retag.rs` and `rebuild.rs` must remap `unit`. Unlike `DensityMapping`, this variant holds a `Ref`.
- The scales are compared by their bits (`to_bits()`), as in core's `F64x3`.

### 3.2 Scoping

A unit's lists (`density_inputs`, `host_inputs`, `auto_consts`, `constants`, `helper_functions`) stay per unit and
describe its own body. The evaluated unit keeps its own lists, which are not copied into the reader's.

What a reader **reaches** is its own closure plus the closures of every unit it evaluates, transitively. Everything
that schedules or allocates uses the reached set (§5.3):

- buffered inputs read inside an evaluated body,
- hosts an evaluated body reads, which join the entry's `HostConsts`,
- autoconsts and tables, already global on `Orchestration`.

### 3.3 Builder (`tungsten-builder`)

```rust
/// `eval(unit, p)` with the given scales.
pub fn eval(unit: UnitHandle, p: impl Into<Expr>, origin_scale: (f64, f64, f64), position_scale: (f64, f64, f64)) -> Expr;
```

- It is typed from the unit's return type and poisons on a non-`Pos3` `p`, as other typed expressions do.
- A `Grid`-taking overload `eval_on(unit, p, &grid)` reads only the scales from `grid`, for frontends that already hold
  one. Its dims are ignored.
- The derived unit lists (`derive.rs`) don't add the evaluated unit's items to the reader (§3.2). Derivation walks
  into `Eval` only to find the `unit` for dependency ordering.
- A unit that is only evaluated is still a unit of the program. It needs no entry or input.

### 3.4 Text format (`crates/tungsten-parse/SPEC.md` §4, `TESTING.md` §8)

```
eval_expr = "eval" "(" label "," expr ")" eval_attrs ;
eval_attrs = ( "@origin_scale" vec3_lit | "@position_scale" vec3_lit )* ;
```

- Both attributes are required, in either order, each at most once. They aren't defaulted, so a printed program
  is never ambiguous.
- `label` names a unit. The printer writes it back the same way.
- In the grammar, the attributes bind to the `eval(...)` call, tighter than any binary operator.

## 4. Semantics

### 4.1 One evaluation

`eval(u, p) @origin_scale(os) @position_scale(ps)`, read in a context with run origin `o`:

1. Evaluate `p` in the reader's context.
2. Run `u`'s main function with these ambients:

   | Ambient | Value inside the evaluated body |
   | --- | --- |
   | `Pos3` | `p` |
   | `Origin` | `o`, the run's origin, the same as the reader's |
   | `OriginScale` | `os` |
   | `PositionScale` | `ps` |

   Hosts, autoconsts, tables, consts and externs are the same objects the reader sees.
3. The result is the main function's return value.

Ambients are dynamically scoped. A helper called from the evaluated body sees the evaluated ambients, and a nested
`eval` inside it sets its own.

### 4.2 Inside a refine step

`eval` itself doesn't know about refinement. In a rerun unit, the reader's `Pos3` is the marched position
(`specs/refinement.md` §4.3), so a `p` built from `pos3` moves with the march. Inside the evaluated body, `Pos3` is `p`,
with **no further offset**, because the offset was already applied when the reader computed `p`.

### 4.3 Positions

Any `Pos3` is valid, negative ones included. There is no bounds check and no failure mode belonging to `eval` itself.
Whatever the body does at that position is the value. Indexed buffered reads in the body follow their own
`BufferBounds` (§4.4).

### 4.4 What an evaluated body may read

This covers the evaluated unit's main function and every helper it reaches, transitively through calls but not
through nested `eval`, which is checked on its own:

| Read | Allowed? |
| --- | --- |
| Consts, hosts, autoconsts, tables, externs, ambients | Yes |
| Nested `eval` | Yes. Cycles are an error (§6) |
| Indexed density read `density(name)[i]` of a buffered input | Yes. It reads that input's buffer at `i`, which is scheduled before the reader (§5.3) |
| Un-indexed density read `density(name)` | **No**: there is no cell to map from (`EvalUnindexedRead`) |
| `@previous` input | **No** (`EvalPrevious`): `@previous` means the reader's own cell |

These rules apply only when the unit is evaluated. The same unit can also be an entry or a buffered input, where
un-indexed reads are fine. The rules attach to eval sites, not to units.

### 4.5 Cost

- Each `eval` runs the full body. Nothing is cached across sites, cells or calls.
- A body called in a loop is the frontend's cost to manage. `tungsten-inspect` reports evaluations per site (§5.5).
- Backends may inline the generated function. That's the target compiler's choice (`#[inline]` hints are fine), and
  must not change the bits.

### 4.6 Interactions

| Feature | Behaviour |
| --- | --- |
| Entries, inputs | A unit can be evaluated and also be an entry or input. Those instances are independent of its eval sites |
| Mappings | Unaffected. `eval` has no mapping |
| Refinement `reruns` | Only names instances. A unit that is only evaluated has none, so naming it is the existing "not an instance" error. A rerun unit's evals follow §4.2 |
| Frozen buffers in a refine step | An indexed read inside an evaluated body is classified like the reader's own reads: frozen if its instance is frozen, rerun otherwise |
| Inliner (`perform_inline`) | An eval site is not a use, so `Unused` and `MultipleUses` count only inputs and entries. A unit that is only evaluated is `Unused` and not inlined. Inlining a caller that contains `eval` is fine: the expression moves with the body. Inlining into an evaluated body is allowed only if it doesn't add an un-indexed read (§4.4), and is otherwise rejected with a new `InlineRejection::EvaluatedCaller` |

## 5. Pipeline

### 5.1 Lowering and the interpreter (`tungsten-check`)

Core has no `eval` and doesn't change. `tungsten-check/src/lower` desugars it:

- For each eval key reached from an entry or a refinement, it clones the evaluated unit's main function and the
  helpers it reaches into new core functions. Each one takes an extra first parameter `p: Pos3`, and calls inside the
  clone pass `p` on.
- In the clones, `Ambient(Pos3)` becomes `Local(p)`, `Ambient(OriginScale)` and `Ambient(PositionScale)` become
  `MakeVec3` literals of the key's scales, and `Ambient(Origin)` stays as it is.
- `Eval { .. }` becomes `Call { func: clone_main(key), args: [p] }`.
- Indexed density reads in the clones stay `Density { input, index: Some(..) }`, and the reader unit's core `inputs`
  gains those inputs.
- Hosts read in the clones join the scoping lists of the reader unit and of the clones.
- Clones are made once per eval key and shared by every site with that key. Nested evals produce nested keys.
- `LowerMap` records each clone back to its source function, so diagnostics use source names.
- Refine-step lowering (`specs/refinement.md` §7.2) runs on the lowered program. Because the clones' `Pos3` is now a
  local, the march offset applies only where the reader computes `p` (§4.2), which is what that section requires.

The desugar is trusted, like the rest of `tungsten-check`. The differential suite covers it (§7).

### 5.2 `Interface` (`tungsten-interface`)

```rust
pub fn eval_fn(&self, unit: Ref<ComputeUnit>, origin_scale: (f64,f64,f64), position_scale: (f64,f64,f64)) -> &str;
pub fn eval_fns(&self) -> &[EvalFn];      // key and name, in discovery order
pub fn eval_helper(&self, key: &EvalKey, f: Ref<Function>) -> &str;  // helpers reached from an evaluated body
```

- `NamingRules` fixes names such as `eval_ore_noise_s1x1x1_1x1x1`. The suffix is derived from the scale bits so it
  is stable. A collision falls back to the eval key's discovery index.
- Keys are collected from units reachable from entries and refinements, and through nested evals, in deterministic
  discovery order.
- **Helpers are cloned per eval key.** They read ambients, so a helper shared between a kernel and an evaluated body
  needs two versions. A helper that reads no ambient (transitively) may be shared, as an optimization the backend may
  skip.
- Schedules (§5.3) include the reached set.

### 5.3 Scheduling and worker layout

- A reader's dependencies are its own inputs **plus** the buffered inputs read inside every unit it evaluates,
  transitively. `Interface::build` adds those edges, so the waves put each buffer before every reader that reaches it.
- `entry_hosts(entry)` includes hosts reached through eval.
- An evaluated unit has **no** `Instance`, buffer, slot or kernel, unless it's also an entry or input.
- `input_params(f)` and `offset_param` extend to the generated eval functions. An eval function that reads buffered
  inputs takes the same pool and offset parameters a kernel helper does (`specs/workers.md` §4.3).

### 5.4 Backends

Each backend emits one function per eval key, plus its cloned helpers:

| Backend | Form | Signature |
| --- | --- | --- |
| Rust | `fn` (`#[inline]` hint), like a density helper | `(p: Pos3, origin: Vec3, ctx/pool, offs.., auto_constants, host_consts) -> T` |
| Java | `private static` method | the same, with Java's existing helper parameters |
| CUDA | `__device__` function | the same, with `const DeviceCtx&`. No recursion, since cycles are rejected (§6) |

- In the generated body, `Pos3` is the parameter `p`, and the scales are literals folded from the key. `origin` is
  passed through.
- An `Eval` expression lowers to a call with the reader's `p`, `origin` and context.
- Indexed buffered reads follow the compile's `BufferBounds`, as they do in kernels.
- Java: each eval function is its own method, so `size.rs` estimates it against the 64 KB limit separately, not as
  part of the reader.
- No new runtime symbols. `tungsten-mc`, `cuda-density` and the test runtimes don't change.
- The low-level path (`specs/compiler.md` §6) doesn't validate. There, a backend returns `BackendError::Codegen` for
  what it can't emit: an eval cycle, or an un-indexed read in an evaluated body.

### 5.5 `tungsten-inspect`

- IR text prints `eval(unit, p) @origin_scale(..) @position_scale(..)`.
- Graphs draw an edge from the evaluated unit to the reader, labelled `eval` (dotted, distinct from input edges and
  the dashed `@previous`), with one edge per key.
- Analyses report, per reader, the eval sites and how many there are statically (a site inside `repeat n` counts
  `n` times. Under a `while`, it reports "unbounded").

### 5.6 Other crates

| Crate | Change |
| --- | --- |
| `tungsten-ir` | `Expression::Eval`, plus `retag`/`rebuild` |
| `tungsten-builder` | `eval`, `eval_on`, derivation (§3.3) |
| `tungsten-parse` | Grammar and printer (§3.4) |
| `tungsten-check` | Validation (§6) and the desugar (§5.1) |
| `tungsten-interface` | Eval keys, naming and the reached-set scheduling (§5.2, §5.3) |
| `tungsten-optimizer` | Use counting and `EvaluatedCaller` (§4.6) |
| `tungsten-rust`, `-java`, `-cuda` | Eval functions (§5.4) |
| `tungsten-inspect` | §5.5 |
| `tungsten-core` | None |

## 6. Validation

These are new `LowerErrorKind` variants, so `@expect reject(Kind)` can name them. They are reported in arena and
discovery order, once per eval key unless stated otherwise:

| Rule | Error |
| --- | --- |
| `position` has type `Pos3` | the existing type mismatch |
| `unit` is a unit of the program | `EvalUnknownUnit` |
| No cycle through `eval` and density reads (a unit evaluating itself, directly or indirectly, or evaluating a unit that reads it) | `EvalCycle` (names the cycle) |
| No un-indexed density read in an evaluated body's reach (§4.4) | `EvalUnindexedRead { unit, function, input }` |
| No `@previous` read in an evaluated body | `EvalPrevious` |
| Every scale component is finite | `EvalBadScale` |

- Bodies that are never evaluated aren't checked against §4.4.
- `EvalCycle` reuses the acyclicity walk, with `Eval` as a unit-to-unit edge alongside the input edges.

## 7. Testing

### Corpus (`tungsten-testsuite`)

- **Equivalence with buffered reads (the main property).** For a unit `u` and grid `g`, a reader on `g` that returns
  `eval(u, pos3) @scales(g)` produces the same bits as one that reads `u` buffered on `g` with an Identity input.
  The harness generates the eval form from existing input cases, so the whole corpus becomes eval cases.
- Off-grid and negative positions, compared across the interpreter and every backend.
- Nested evals with different scales, and a helper shared between a kernel and an evaluated body, to catch ambient
  leaks in helper cloning.
- An evaluated body that reads a buffered input by index, to check that the buffer is scheduled before the reader
  (and in a refine step, frozen versus rerun).
- Evals in a rerun unit with a march (§4.2).
- Hosts read only inside an evaluated body appear in `HostConsts`.
- A record return type.
- Rejections: `EvalCycle` (direct, indirect, through an input), `EvalUnindexedRead` (in main and in a helper),
  `EvalPrevious`, `EvalBadScale`, and a non-`Pos3` position.

### Other checks

- Builder: typing, poisoning, `eval_on`, and derived lists not absorbing the evaluated unit's items.
- Parser and printer round-trip, with attributes in either order.
- `Interface` determinism: names and discovery order are stable when built twice.
- Inliner: a unit that is only evaluated is `Unused`, and `EvaluatedCaller` is reported.
- Java size estimate: an eval function counts on its own.
- Inspect snapshots for the IR text and graph edges.

### Milestones

1. IR, builder, parser and printer, validation, and the interpreter desugar, with corpus cases on the interpreter
   only.
2. `Interface` (keys, naming, reached-set scheduling) and Rust.
3. Java.
4. CUDA.
5. Inspect, optimizer use counting, and docs (`book/src/builder/expressions.md`, `mappings.md` cross-reference).

## 8. Open and deferred

- **CSE and memoization.** Hoisting identical `eval(u, p)` sites in one function into a local is safe (bodies are
  pure apart from externs, so extern purity must hold), and could be an optimizer pass.
- **Scale-independent sharing.** One generated function per eval key may duplicate code when only the scales differ.
  Passing scales as parameters is a possible size optimization, but it gives up constant folding.
- **Un-indexed reads in evaluated bodies.** These could be allowed later by mapping `p` through the input's mapping
  (Identity and Expand, not Lerp), which needs a position override in every backend's mapping codegen.
- **Core semantics for `eval`.** If the desugar ever needs proving, core would need a call with ambient override.
- Frontend migration (`tungsten-mc-compile`).

## 9. Decisions

| # | Topic | Decision | Rejected, and why |
| --- | --- | --- | --- |
| D1 | Shape | An **expression** that evaluates a unit at a position. Units are already per-position, so it is the direct form | A lazy input mode (`@lazy` with a virtual grid and `density_at`): it kept grid bounds and the input machinery for something that has neither. A new `DensityMapping` variant: mappings choose a buffer cell, and a lazy read has no buffer |
| D2 | Use case | Evaluate a small unit over a space larger than any buffer, off-grid and negative positions included | Equivalence with a buffered grid as the definition: it would impose bounds that the use case doesn't want. Equivalence is kept only as a test property on grids (§7) |
| D3 | Scales | Written on the expression, as static constants. `origin` is the run's | Inheriting the reader's scales: the result would depend on the reader's grid. Forbidding scale ambients: existing units read them |
| D4 | IR shape | Inline on `Expression::Eval`. Sites are collected by walking bodies and deduped by eval key | An `evals` arena with units listing their evals (`EvalNotListed`): more bookkeeping for frontends, with no extra safety the walk doesn't give |
| D5 | Syntax | `eval(unit, p) @origin_scale(..) @position_scale(..)`, inline | A program-level `eval e = ...;` declaration: it only made sense with the arena |
| D6 | Bounds | None. Any `Pos3` is valid | Non-negative only: nothing in an evaluated body depends on being on a grid |
| D7 | Body reads | Nested `eval` and indexed buffered reads. Un-indexed and `@previous` reads are errors | Allowing un-indexed reads mapped through `p`: duplicates the mapping logic in every backend (deferred, §8). Leaf units only: too restrictive for layered noise |
| D8 | Who emits it | Each backend emits native functions per eval key. `tungsten-check` desugars to core calls for the interpreter | A shared IR→IR desugar before backends: less control over the shape of device functions, and the desugared clones would show up in every backend's output and in inspect as anonymous helpers |
| D9 | Caching | None. Each site evaluates the body | CSE in this spec: it needs extern purity guarantees (deferred, §8) |
| D10 | Spec location | `specs/unit-eval.md`, next to the other feature specs | Overwriting `SPEC.md`, which holds the in-progress mdBook spec |
