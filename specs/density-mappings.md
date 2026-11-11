# Static density-input mappings

Status: design agreed, not implemented. Lands after `specs/workers.md`, and before `specs/refinement.md`, which extends
it.

## 1. Summary

A density input can declare how an un-indexed read maps the reader's cell to the input's buffer.
The compiler then knows the mapping statically, instead of seeing a runtime extern call in an index
expression.

There are three mappings:

| Mapping | Text | Example | Replaces in the frontend today |
| --- | --- | --- | --- |
| 1-to-1 | none (the default) | `(5,49,5)` read from `(5,49,5)` | `DensityVariable(input, None)` |
| Expansion | `@expand(fx, fy, fz)` | `(5,1,5)` read from `(5,49,5)`: `@expand(1, *, 1)` | `flat_y_zero_index` |
| | | `(4,1,4)` read from `(16,384,16)`: `@expand(4, *, 4)` | `biome_column_index` |
| Linear interpolation | `@lerp(fx, fy, fz)` | `(5,49,5)` read from `(16,384,16)`: `@lerp(4, 8, 4)` | `corner*_8`, `*fract*` and `interpolate` |

### Goals

- Backends generate interpolation themselves, with shifts, offsets and fractions folded to constants,
  instead of 11 extern calls per read.
- The frontend's mappings can be expressed with no extern runtime support.
- All three mappings behave the same on the interpreter and every backend, bit for bit.

### Non-goals

- Changing `tungsten-core` or its Verus proofs.
- Moving the frontend to mappings. That is a follow-up in `tungsten-mc-compile` (§9).
- Hoisting work out of the kernel loop, or materializing a full-resolution interpolated buffer.
- A lower-precision lerp. The lerp's type is the input's element type. If a frontend wants `f32`
  interpolation, it gives the input an `f32` unit.
- Checking that scales agree with factors (§7).
- Validation on the low-level path (`specs/compiler.md` §6).

## 2. Data model

`crates/tungsten-ir/model.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisFactor {
    /// The input has size 1 on this axis. Every reader coordinate reads coordinate 0.
    Repeat,
    /// Each input coordinate covers `n` reader coordinates. `n >= 1`.
    Factor(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DensityMapping {
    #[default]
    Identity,
    Expand([AxisFactor; 3]),
    Lerp([AxisFactor; 3]),
}

pub struct DensityInput {
    // existing fields
    pub mapping: DensityMapping,
}
```

- The mapping belongs to the input, not to each read. To read one buffer in two ways, a program declares
  two inputs and a unit lists both. Every unit that lists a shared input reads it with the same mapping.
- **The mapping isn't part of `Instance`.** A buffer depends only on unit, dims and scales, so two
  inputs that differ only in mapping share a buffer and its kernel.
- Factors don't depend on the reader's size, so one mapping can serve a unit that runs at several
  dims, as long as each of those dims passes §4.
- `DensityMapping` holds no `Ref`, so `retag.rs` and `rebuild.rs` don't change.
- A read with an explicit index ignores the mapping and reads `buf[index]`, exactly as it does today.

## 3. Semantics

In these formulas `p` is the reader's `Pos3`, and `(sx, sy, sz)` are the input's dims. Reader
coordinates are never negative, so `/` and `%` are floor operations here. Inside a refine call, `p` is the marched
position (`specs/refinement.md` §4.3), which equals the cell only outside one.

For an input flat index: `flat(X, Y, Z) = X + Y*sx + Z*sx*sy`.

### 3.1 Identity

The read keeps today's rule: cell `0` for a `(1,1,1)` input, otherwise the reader's flat number. §4
makes this strict by requiring equal dims.

### 3.2 Expand

Per axis, the input coordinate is:

| Factor | Coordinate |
| --- | --- |
| `Repeat` | `0` |
| `Factor(n)` | `p / n` |

The result is `buf[flat(X, Y, Z)]`. Any element type is allowed.

### 3.3 Lerp

The element type `T` must be `f32` or `f64`. Per axis:

| Factor | Interpolated? | Low coordinate | High coordinate | Fraction `t` |
| --- | --- | --- | --- | --- |
| `Repeat` | No | `0` | | |
| `Factor(1)` | No | `p` | | |
| `Factor(n)`, `n > 1` | Yes | `p / n` | low + 1 | `T(p % n) / T(n)` |

The canonical formula, for all three axes interpolated, is today's `interpolate`:

```
lerp(a, b, t) = a + t * (b - a)

x00 = lerp(v000, v100, tx)    x10 = lerp(v010, v110, tx)
x01 = lerp(v001, v101, tx)    x11 = lerp(v011, v111, tx)
y0  = lerp(x00, x10, ty)      y1  = lerp(x01, x11, ty)
result = lerp(y0, y1, tz)
```

An axis that isn't interpolated drops out, with no lerp and no extra load. So `@lerp(4, *, 4)` is 4
reads and 3 lerps, and a mapping with no interpolated axis is a copy.

### 3.4 Exactness

Every backend must produce the same bits as this formula. The rules that follow from it:

- **Allowed:** a shift or mask when `n` is a power of two, `* 0.25` in place of `/ 4.0`, or any other
  rewrite that can't change the bits.
- **Forbidden:** skipping a lerp when `t == 0`. `-0.0 + 0 * (b - a)` is `+0.0`, and an infinite or NaN
  `b` makes the result NaN. Reordering axes is also forbidden, as are FMA and `* (1/n)` when `n` isn't
  a power of two.

The CUDA `Tolerance` profile gets no special handling. It compiles the same source, and any
difference comes from the flags that profile already allows.

## 4. Validation

Validation runs in `tungsten-check` lowering, so `check::prepare` and `Compiler` report it along with
other lowering errors. Only the low-level path skips it (§5.3).

The errors are new `LowerErrorKind` variants, so `@expect reject(Kind)` can name them.

### 4.1 Shape, for every input with a non-Identity mapping

| Rule | Error |
| --- | --- |
| Every `Factor(n)` has `n >= 1` | `MappingBadFactor` |
| `Repeat` only on an axis where the input's size is 1 | `MappingRepeatOnWideAxis` |
| `Lerp` only on an input whose unit returns `f32` or `f64` | `MappingNotFloat` |

`specs/refinement.md` §5 adds that a `@previous` input must be `Identity`.

### 4.2 Reader grids, only for a unit that reads the input un-indexed

An input read only through explicit indices has no reader rule. Its index expression is its own
mapping, as the aquifer's is. So the default `Identity` costs those inputs nothing.

For each `(reading unit, dims)` pair reachable from the entries, and with `r` the reader's size on an
axis. A reading unit lists the input, and its main function or a helper reads the input un-indexed. Each
such unit of a shared input is checked separately:

| Mapping | Requirement | Error |
| --- | --- | --- |
| `Identity` | Reader dims equal input dims | `MappingDimsMismatch` |
| `Expand`, `Factor(n)` | `(r - 1) / n < s` | `MappingOutOfRange` |
| `Lerp`, `Factor(n)`, `n > 1` | `(r - 1) / n + 1 < s` | `MappingOutOfRange` |
| `Lerp`, `Factor(1)` | `r - 1 < s` | `MappingOutOfRange` |

These checks matter because a coordinate past the end of an axis often gives a flat index that is
still in range, in the next row. That reads the wrong cell without any error.

- **With refinement,** reader pairs become `(reading unit, dims, march)`, reachable from entries and from refinements.
  For `Expand` and `Lerp`, `r - 1` becomes `r - 1 + march` on each axis, so a march can't leave the input's grid.
  `Identity` still requires equal dims, with no `march`. Until `specs/refinement.md` lands, `march` is 0.
- The high corner counts even when its weight is 0. Interpolating `(5,…)` with factor 4 fits a reader
  up to 16 wide. A 17-wide reader is rejected.
- A reader smaller than the input allows is legal.
- Unreachable units and unlisted inputs have no reader, so only §4.1 applies to them.
- Grid arithmetic uses `i64`. Pairs with dims below 1 are skipped, and the core checker reports
  those as `DimsTooSmall`.
- Errors are reported once per `(input, reader dims)`, in arena and discovery order, and give the
  axis and both sizes.

Worked examples:

| Input | Reader | Mapping | Check |
| --- | --- | --- | --- |
| `(5,1,5)` | `(5,49,5)` | `@expand(1, *, 1)` | x, z: `4/1 = 4 < 5` |
| `(4,1,4)` | `(16,384,16)` | `@expand(4, *, 4)` | x, z: `15/4 = 3 < 4` |
| `(5,49,5)` | `(16,384,16)` | `@lerp(4, 8, 4)` | x, z: `15/4 + 1 = 4 < 5`; y: `383/8 + 1 = 48 < 49` |
| `(5,49,5)` | `(16,1,16)`, `@march(0, 383, 0)` | `@lerp(4, 8, 4)` | x, z: `15/4 + 1 = 4 < 5`; y: `(1 - 1 + 383)/8 + 1 = 48 < 49` |

## 5. Pipeline

### 5.1 Lowering and the interpreter

`tungsten-core` doesn't change. `tungsten-check/src/lower/expr.rs` desugars each un-indexed read:

| Mapping | Lowered read |
| --- | --- |
| `Identity` | `Density { input, index: None }`, as today |
| `Expand` with all `Repeat` | `Density { input, index: None }`, since core reads cell 0 of a `(1,1,1)` input |
| Other `Expand` | `Density { input, index: Some(flat(..)) }` over `Field(Ambient(Pos3), axis)` |
| `Lerp` | The §3.3 formula as a nested expression tree |

- Core has no modulo and no `let`. `p % n` becomes `p - (p / n) * n`, and shared subexpressions are
  repeated. A 3-axis lerp lowers to 27 `Density` nodes.
- With memo off, each of those nodes rebuilds its buffer. The corpus keeps these grids small for that
  reason, and a vanilla-sized grid is `@tier(large)`.
- Lowered core programs for existing input don't change, so the L2 snapshots stay as they are.
- This formula is the reference that backends are compared with. It sits outside the verified crate.

### 5.2 Backends

Generated code is self-contained: there are no new runtime symbols, so `tungsten-mc`, `cuda-density`
and the test runtimes don't change.

**Identity:** generated code stays exactly as it is, including the existing `(1,1,1)` branch.

**Expand:** the read's index is an inline expression, folded from constants, and is then read like any
index (`specs/workers.md` §5.1). Factor 1 and `Repeat` drop out, and powers of two use shifts:

```
(p.x >> 2) + (p.z >> 2) * 4       // (4,1,4) read with @expand(4, *, 4)
```

**Lerp:** each distinct `(input dims, factors, element type)` gets one generated helper that takes the
worker's pool, the input's offset and the position. Expression-only emission would repeat each corner several times, and would
threaten Java's 64 KB method limit, which `size.rs` estimates against. The helper body holds the folded
offsets, the fractions, the loads and the lerps in §3.3 order.

| Backend | Helper form |
| --- | --- |
| Rust | `#[inline(always)] fn`, taking `&[T]` (the pool), `off: usize` and `Pos3` |
| Java | `private static` method, taking `T[]` (the pool), `int off` and the coordinates. Small enough for the JIT to inline |
| CUDA | `__device__ __forceinline__` function, taking `const T*` (`tw::buffer<T>(ctx.buffers, off)`) and `int3` |

The caller passes its `off_<input>` parameter (`specs/workers.md` §4.3). Each load follows the compile's
`BufferBounds` as in `specs/workers.md` §5.1, with `LEN` from the input's dims, so `BufferBounds` is part of the
generated helper, which is fixed per compile.

`Interface` fixes the helper names through `NamingRules`, in the module namespace:

```rust
pub fn lerp_helper(&self, input: &DensityInput) -> &str;
pub fn lerp_helpers(&self) -> &[LerpHelper];   // shape and name, in discovery order
```

Names look like `lerp_5x49x5_by_4x8x4_f64`. Helpers are collected from inputs of reachable units, including
refined units once `specs/refinement.md` lands, so the set is deterministic and shared across readers.

Backends read the same mapping regardless of the reader's dims, so density functions don't need a
per-dims variant for it.

### 5.3 Low-level path

`Compiler` always validates. The low-level path (`specs/compiler.md` §6) doesn't, and that path is for tools and tests:

- A mapping that fails §4 generates code that can read the wrong cell.
- An `Identity` read across different shapes keeps today's flat-number behavior there. So strict 1-to-1
  applies only on the checked path.
- A backend returns `BackendError::Codegen` only for what it can't emit at all: `Lerp` on a non-float
  element, and a factor below 1.

### 5.4 Optimizer

`perform_inline` currently treats every un-indexed read as an identity read. That becomes wrong, so
`Planner::plan` gets a new criterion, using a new `DensityMapping::is_identity_on(input_dims)`. That
returns true for `Identity`, and for `Expand` or `Lerp` where each axis is `Factor(1)` or `Repeat`.

| Case | Inlined? |
| --- | --- |
| Reads with a mapping that is identity on the grid | Yes, including a `(1,1,1)` input with all `Repeat` in a `(1,1,1)` reader |
| Other mapped reads | No: new `InlineRejection::MappedRead { function }`. `InlineRejection::Refinement` (`specs/refinement.md` §8.1) is checked first |
| A callee that has its own mapped inputs | Yes. The caller lists the input from then on, and the existing same-grid rule means its reader dims don't change |

### 5.5 Text format

`crates/tungsten-parse/SPEC.md` §4 grammar and `TESTING.md` §8:

```
input     = "input" label "=" label grid_attrs mapping? ";" ;
mapping   = ("@expand" | "@lerp") "(" factor "," factor "," factor ")" ;
factor    = UINT | "*" ;
```

- `*` is `Repeat`. A missing mapping is `Identity`, and the printer omits it.
- Entries don't accept a mapping. The attribute may appear anywhere among the input's attributes, and
  at most once.
- Mappings are never derived. A `.tw` file with a `(1,1,1)` input read from a larger grid must say
  `@expand(*, *, *)`.

### 5.6 Other crates

`tungsten-legacy` is gone by the time this lands (program-level-inputs §6.8). `tungsten-builder` gains
`InputSpec::mapping`, and the mapping joins its input merge key (`specs/builder.md` §8). `tungsten-mc-compile` stays on
its pinned version until it's ported. Once it is, for strict 1-to-1 to accept its existing reads, a `(1,1,1)` input read
from a larger grid must be `Expand([Repeat; 3])`, which generates the same code.

| Crate | Change |
| --- | --- |
| `tungsten-inspect` | IR text prints the mapping, graphs label the edge, and dataflow treats mapped reads as reads of the whole buffer |
| Documentation | `SEMANTICS.md` §9.3 notes that mappings are desugared before core. Update `tungsten-check/README.md`, `CHECK.md` if it lists lowering errors, and the `README.md` |

## 6. Migration of the corpus

- `density/dims_different.tw` becomes a `reject` file for `MappingDimsMismatch`. Its flat-number read
  across shapes is no longer legal.
- `density/dims_one.tw` and other files that read a `(1,1,1)` input from a larger grid add
  `@expand(*, *, *)`. Their expected values don't change.
- The warning in `SEMANTICS.md` §9.3 about flat-number reads stays for core, but no longer applies to
  source programs.

## 7. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Where the mapping lives | On `DensityInput` | Per read: every walker, the inliner and the printer change. New expression variants: widest change, and repeats `DensityVariable`. |
| Reader dims | Per-axis factors, `*` for repeat | Explicit reader dims: one mapping couldn't serve several grids. Derived per instance: the program wouldn't state the factor, so a wrong grid changes the factor silently. |
| Interpreter | Desugar in lowering | Native in core: Verus work for a mapping set that may still change. |
| Default read | Strict 1-to-1 | Keeping flat-number reads as a separate default: keeps a read `SEMANTICS.md` itself warns about. |
| Reader validation | Bounds only | Scales too: a mapping is only an index formula, and synthetic tests and unusual programs stay legal. A wrong scale remains a frontend bug with no diagnostic. Exact coverage: rules out partial readers. |
| Validation site | In lowering, which `Compiler` always runs | Panic in `Interface::build`: user-reachable paths should return errors. |
| Lerp order | Today's X, Y, Z, with `a + t*(b-a)` | Z, Y, X: easier hoisting, but changes values and may no longer match Minecraft. Per-backend order with a tolerance: breaks bit comparison. |
| Lerp types | `f32` and `f64`, following the input's element type | `f64` only: too narrow. `Vec3`: backends' layouts differ, and nothing uses it. |
| Fast lerp | Out of scope; the frontend picks the type | An FP32 lerp under `Tolerance`: precision is decided by what the input emits, not by the backend. |
| First-version optimization | Inline, const-folded, per read | Kernel hoisting: needs a design per backend and changes the uniform parameters. Materialized buffer: about 786 KB per lerped `(16,384,16)` `f64` input, plus scheduling. |
| Adoption | IR only, and the frontend sets mappings once it builds IR directly | Mapping in legacy: that crate is deleted first. Recognition pass: ties the optimizer to frontend extern names. |
| Spec location | `specs/density-mappings.md` | Root `SPEC.md`: holds the testsuite design, which still has deferred work. |

Decided without an interview question, and easy to revisit: helpers instead of expression-only lerp,
non-power-of-two factors being legal, `Factor(1)` not interpolating in a lerp, explicit-index reads
ignoring the mapping, and the `@expand` and `@lerp` spellings.

## 8. Testing

All corpus files run through the existing trials (`roundtrip`, `lowering`, `interp`, `inline`, and each
backend on both stages). Expected values are computed by hand.

### Corpus

New `density/map_*` files:

| Concern | Files |
| --- | --- |
| Identity | Equal dims; explicit-index input whose dims differ from the reader (stays legal) |
| Expand | Repeat in y; factors with repeat; factor 3; all repeat; partial reader; non-float element (record, array) |
| Lerp | 3-axis; 2-axis with repeat; factor 1 on an axis; factor 3 (non-power-of-two fraction); `f32` and `f64` |
| Lerp bits | `-0.0` corner at `t = 0` gives `+0.0`; infinite and NaN corners at `t = 0`; the last reader cell of each axis |
| Sharing | Two inputs with different mappings over one buffer; one unit at two reader dims; a mapped read inside a helper; one lerp shape in two units; one mapped input listed by two units with different reader dims; a mapped read in a shared helper |
| Inlining | Identity-on-grid read inlined; mapped read not inlined; callee with its own mapped input inlined |
| Vanilla shapes | The three §1 examples at full size, `@tier(large)` |

New `reject/` files, one per error in §4, including a lerp whose high corner is out of range at
weight 0.

### Other checks

- **Mutation checks on the real path**, done once and recorded: swapping lerp axes, and replacing the
  `t == 0` case with a shortcut, must turn the lerp corpus red on each backend.
- **Regression check while developing:** generated output for the existing corpus doesn't change, except
  for the files changed in §6. This is a development aid, not a compatibility requirement.
- **Formula agreement:** a test compares the lowered lerp with a copy of `tungsten-mc`'s `interpolate`
  on edge values, by bits.
- **Text:** parser and printer round-trip, plus the generators in `tungsten-parse/tests/common/mod.rs`.
- **Verus:** `scripts/verify.sh` needs no change, since `tungsten-core` is untouched.
- **Vanilla gate:** milestone 2 starts from the vanilla mapping scan recorded by
  `specs/program-level-inputs.md` §8, done with the frontend on v0.2. Each read it found is a real
  un-indexed read across shapes and needs deciding before strict 1-to-1 is merged.

### Milestones

Each lands green.

1. **Model.** `DensityMapping`, parser and printer, `InputSpec::mapping` in the builder and its merge key, no semantics
   yet. Mapped input is rejected by lowering as unsupported.
2. **Interpreter.** Validation, desugaring, inliner rule, corpus, §6 migration, vanilla gate. Backend gaps
   are `@knownfail`.
3. **Rust.** `Interface` helpers, expansion and lerp. Its `@knownfail`s expire.
4. **Java.**
5. **CUDA.**
6. **Inspect.** Documentation lands with that of the other v0.3 specs.

## 9. Open and deferred

- **Frontend follow-up** in `tungsten-mc-compile`: emit `@expand` and `@lerp` in place of the extern calls, once it's
  ported to `tungsten-builder`. New IR fields don't break builder users.
- **Unmeasured:** how much faster folded lerp is than the extern calls. Measure when the frontend moves,
  before starting on any of the deeper options.
- **Later optimizations:** kernel hoisting and materialized buffers, without IR changes. Hoisting benefits
  little while `x`, the fastest axis, is the first lerp.
- **Mapping as core semantics:** move it into `tungsten-core` if the proof work needs it.
- **Scale agreement:** source scale equal to reader scale × factor holds across today's frontend grids.
  A lint could report it later, without rejecting.

## 10. Implementation notes

### Vanilla mapping scan (gate for milestone 2)

Done on 2026-11-11 with `tungsten-mc-compile` at `9d4d2cf` (plus its uncommitted working-tree changes) against
tungsten-wg `v0.2` (`9a9c848`). A scan converted the vanilla overworld (`spmt::conversion::convert`), walked every
`(unit, reader dims)` pair reachable from the seven entries, and looked at each `DensityVariable` in the unit's main
function and helpers. It was run before and after `perform_inline`:

| | Pairs | Un-indexed reads | Indexed reads across shapes | Un-indexed `(1,1,1)` from a larger grid | Mismatches |
| --- | --- | --- | --- | --- | --- |
| Not inlined | 73 | 152 | 169 | 0 | **0** |
| Inlined | 53 | 118 | 169 | 0 | **0** |

**No reads found.** Vanilla reads every input that has different dims through an explicit index (`flat_y_zero_index`,
`biome_column_index`, `corner*_8`, the aquifer). So strict 1-to-1 rejects nothing in vanilla, and the gate had nothing
to decide.

### Milestone 2

- Lowering folds `/ 1` and `* 1` out of the desugared indices and drops zero terms of `flat`. The bits don't change.
- `MappingDimsMismatch` and `MappingOutOfRange` report the first failing axis. The error is reported at the input,
  and it names the reading unit by its main function, as other lowering errors do.
- The formula agreement test is `tungsten-check/src/tests/mapping.rs`. It checks every cell of an `(8,16,8)` reader
  of a `(3,3,3)` `@lerp(4, 8, 4)` input with edge values against a copy of `tungsten-mc`'s `interpolate`.
