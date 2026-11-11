# Refinement: rerun part of an orchestration, marching along `Pos3`

Status: design agreed, not implemented. Lands last among the v0.3 code, after `specs/workers.md` and
`specs/density-mappings.md`. Its refine calls are worker methods.

## 1. Summary

A worker's `run` computes the entry's whole closure and returns its buffer. Refinement adds a second generated call on
the worker that reruns only part of that closure:

```text
worker.run(origin, hosts)                       // normal: everything, Pos3 offset 0
worker.refine_step(offset(0, 383, 0), hosts)    // reruns the marked instances; the rest stays frozen
worker.refine_step(offset(0, 382, 0), hosts)
...
```

- Marked instances **rerun**. Each runs either its own unit again or a **refined unit** that replaces it.
- Everything else is **frozen**: its buffer keeps the value from the last time it was written.
- A refined unit can read **its own output at the current cell** (`@previous`). The first such value comes
  from the normal unit it replaces.
- The origin and scales never change. Only `Pos3` moves, while the buffer index stays in the buffer. That lets a
  program compute a volume one slice at a time, without a large eager 3D buffer, and it keeps frozen data valid.

### Goals

- Avoid large eager buffers such as `find_top_surface`'s `(x, height / cell, z)` grid when the work can be
  done slice by slice.
- `@lerp` and `@expand` give correct results along the march, including reads of frozen coarse grids.
- Same results, bit for bit, on the interpreter and every backend.
- `tungsten-core` and its proofs are unchanged.

### Non-goals

- Changing the origin or scales between calls. Moving the origin makes frozen buffers stale (§11).
- Loops or stopping conditions in generated code. The caller drives the march and decides when to stop.
- Per-column offsets. One call has one offset. A unit can skip a finished column by checking `@previous`.
- Reruns at different rates along the march (§4.2).
- History deeper than one call. A refined unit can carry more state in a record or array element.
- Frontend adoption (§12).
- Core semantics for refinement, so the sequence isn't covered by T1–T5 (§7.3). This is planned follow-up work.
- Generated checks on call preconditions (§6.3).

## 2. Example

A top-down surface march at block resolution. Column data and a coarse grid are computed once. `detail`
and the refined `surface` unit rerun at every step.

```text
program {
    // fn definitions omitted

    input prev = seed   @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @previous;
    input d    = detail @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    input col  = column @dims(4, 1, 4)   @origin_scale(1, 0, 1) @position_scale(4, 1, 4) @expand(4, *, 4);
    input grid = coarse @dims(5, 49, 5)  @origin_scale(1, 1, 1) @position_scale(4, 8, 4) @lerp(4, 8, 4);

    unit surface {
        main surface_main;
        inputs [prev, d, col, grid];
    }

    entry main = seed @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);

    refinement step of main @march(0, 383, 0) {
        rerun seed   @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) with surface;
        rerun detail @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }
}
```

`run` on `main`'s worker runs `seed`, `column` and `coarse`. It doesn't run `detail`, since only a rerun reads it.
`refine_step` runs `detail`, then `surface`, and stores the result in `seed`'s buffer. `column` and
`coarse` aren't listed, so they stay frozen.

## 3. Data model

`crates/tungsten-ir/model.rs`:

```rust
pub struct OptimizedProgram {
    // existing fields
    pub refinements: Vec<Refinement>,
}

pub struct Refinement {
    pub name: String,
    /// Index into `entry_points`.
    pub entry: usize,
    /// Largest allowed offset per axis, each >= 0. An axis with 0 doesn't march.
    pub march: (i32, i32, i32),
    pub reruns: Vec<Rerun>,
}

pub struct Rerun {
    /// The buffer written, identified as an instance: unit, dims and scales.
    pub cu: Ref<ComputeUnit>,
    pub dimensions: (i32, i32, i32),
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
    /// The refined unit that runs instead of `cu`. `None` reruns `cu`.
    pub with: Option<Ref<ComputeUnit>>,
}

pub struct DensityInput {
    // existing fields, including `mapping`
    /// Reads this instance's output from before the current call. Only refined units may list it (§5).
    pub previous: bool,
}
```

- `reruns` holds instances, because a buffer belongs to an instance. One instance can't be rerun on one edge and
  frozen on another within a call.
- A refined unit has no grid of its own. It runs at its target's dims and scales, which enforces "same ambients"
  by construction.
- Readers name the **target**, also when it's replaced. `input d = detail ...` reads `detail`'s buffer, whichever
  unit wrote it last.
- A new `Ref`-holding field must be added to `retag.rs` and `rebuild.rs`, per the note on `OptimizedProgram`.
- `tungsten-builder` gains `InputSpec::previous`, which joins its input merge key, and a way to declare refinements
  (`specs/builder.md` §8). `pb.entry` returns an `Entry` handle, and validation stays in lowering:

```rust
let entry: Entry = pb.entry(seed, grid);
let prev = pb.input_with(InputSpec::new(seed, grid).previous());
// ... `surface` reads `prev` ...
pb.refinement("step", entry, (0, 383, 0), [
    RerunSpec::new(seed, grid).with(surface),
    RerunSpec::new(detail, grid),
]);
```

## 4. Semantics

### 4.1 One refine call

`refine_<name>(offset, hosts)` on the entry's worker:

1. **Reachability.** Starting at the entry's target, follow non-`previous` inputs of each running unit, but only
   through instances in `reruns`. The instances reached run in this call, in dependency order. Others are frozen.
2. **Execution.** Each running instance is evaluated over its own grid, with:
   - `Ambient(Origin)`: the origin of the last `run` on this worker.
   - `Ambient(OriginScale)`, `Ambient(PositionScale)`: the target's scales.
   - `Ambient(Pos3)`: `cell + offset`.
   - **Buffer index**: `flat(cell, dims)`, unaffected by the offset.
   - Hosts: the ones passed to this call.
3. **Result.** The running unit's value is written to the target's buffer. The entry's result is that buffer, in
   the same form as `run` returns it.

Each cell is written after its density function returns, as all kernels do today.

### 4.2 The offset

All running instances get the same `offset`. Its world effect is `offset * position_scale`, so all targets in a
refinement must have the same position scale on every marched axis (`RefineMarchScaleMismatch`, §5). Data at a
coarser rate along the march stays frozen and is read through `@lerp`.

The call always takes an offset, including when `march` is all zero, in which case the caller passes zeros.

### 4.3 Reads

`Pos3` and the index are already separate in each runtime: `it.pos` / `it.idx` in Rust, `p.pos` / `p.index` in
Java, `pos.pos` / `pos.idx` in CUDA, `ctx.cell` / `ctx.k` in core.

| Read | Uses | In a marching unit |
| --- | --- | --- |
| Identity, un-indexed | Index | Same slice cell. Requires equal dims, as in density-mappings §4.2 |
| `@previous` | Index | The current cell's value from before this write |
| `@expand`, `@lerp` | `Pos3` | The marched position, so the coordinate and fraction are correct across the march |
| Explicit index | The program's expression | Whatever the program computes, usually from marched `Pos3` |

`@lerp` needs no changes: its §3 formulas already take `p`, and with marching `p` is the marched position.

### 4.4 `@previous`

- **Current cell only.** A `@previous` read must be un-indexed and have an `Identity` mapping. Since a cell is
  written only after its function returns, the read always gets the old value. It needs no second buffer and no
  copy, and it can't race.
- Multiple reads in one cell give the same value.
- The first value comes from the normal unit, through `run`. After that it comes from the last
  refinement call that wrote this buffer, of any refinement.

### 4.5 What `run` computes

For an entry with refinements, the schedule is the union of the normal closures of:

- the entry;
- every frozen instance that a running unit of any of its refinements reads;
- every `with` target of its refinements, because the normal unit provides the first value.

A stateless rerun instance that only a rerun reads is left out. Callers who never refine still pay for this
schedule. Eager failure rules apply: a failing frozen buffer fails `run`.

`run` always uses offset 0 and is the same as before, apart from the larger schedule and storing the
origin. For a top-down march, the normal unit provides the seed state, and the first refine passes the top offset.

### 4.6 Several refinements

An entry can have any number of refinements. They share the worker's buffers, so one can continue from another's
output, and a frozen instance in one can hold what another just rerun. Calling `run` again recomputes the whole
schedule and restarts from there. Two entries never share buffers, even for the same instance, because each has its own
worker.

### 4.7 Frozen buffers

A frozen buffer holds what was last written to it. That includes the hosts of that write: changing a host in a
refine call only affects the units that run. Hosts are the intended way to pass step-dependent values, such as a step
count or an interpolation factor.

## 5. Validation

Validation runs in `tungsten-check` lowering, next to the mapping validation, so `check::prepare` and `Compiler` report
it. The errors are new `LowerErrorKind` variants that `@expect reject(Kind)` can name. As with mappings, only the
low-level path skips it (`specs/compiler.md` §6). That path panics on a cycle in `arrange_waves`, and it returns
`BackendError::Codegen` only for what it can't emit.

| Rule | Error |
| --- | --- |
| `entry` is a valid index; refinement names are unique per entry | `RefineBadEntry`, `RefineDuplicateName` |
| Every `march` component is `>= 0` | `RefineBadMarch` |
| `reruns` contains the entry's instance | `RefineEntryNotRerun` |
| No instance is listed twice in a refinement | `RefineDuplicateRerun` |
| A listed instance is reachable by §4.1 step 1 | `RefineRerunUnreachable` |
| A `with` unit returns the target's return type | `RefineTypeMismatch` |
| All targets have bit-equal position scales on marched axes | `RefineMarchScaleMismatch` |
| The instances that run have no cycle, ignoring `@previous` edges. This includes a refined unit reading its target without `@previous` | `RefineCycle` |
| A `with` unit is not an entry, and no normal input reads it | `RefinedUnitInNormalGraph` |
| A `@previous` input is listed only by `with` units | `PreviousOutsideRefinedUnit` |
| A `@previous` input's unit, dims and scales equal the target of **every** rerun that uses a `with` unit listing it | `PreviousNotTarget` |
| A `@previous` input has an `Identity` mapping | `PreviousMapped` |
| A `@previous` input is never read with an explicit index | `PreviousIndexed` |

The rest of density-mappings §4 applies to refined units too, including its reader rule with `march`.

Validation checks declarations only. Offsets and call order are caller preconditions (§6.3).

## 6. Generated code

### 6.1 `Interface`

`Interface::build` gains:

```rust
pub struct RefineRun {
    pub target: Instance,          // buffer written
    pub unit: Ref<ComputeUnit>,    // `with`, or `target.cu`
}

pub fn refinement_names(&self) -> &[String];          // per program.refinements, unique module-level names
pub fn refine_waves(&self, refinement: usize) -> &[Vec<RefineRun>];
pub fn refine_kernel(&self, run: &RefineRun) -> &str;
pub fn density_fn(&self, cu, dimensions);             // existing; now also covers refined units
```

- **Discovery.** `discover_instances` ignores `previous` inputs, so `@previous` adds no dependency and can't trip the
  panic in `arrange_waves`. Instances needed only by refinements are appended after the existing ones.
- **Buffers.** `instances()` includes rerun-only instances, so they get a field. `waves()` leaves them out. `with`
  units get no buffer of their own.
- **`worker_schedule(entry)`** (`specs/workers.md` §4.1) returns the schedule in §4.5, plus the rerun-only instances of
  the entry's refinements, so those instances get slots. `entry_hosts` also covers `with` units.
- **Names.** `tungsten_interface::naming` adds `refinement_method_names`: `refine_<name>`, unique within the worker,
  including against `run`.

### 6.2 Backends

`DensityVariable` read sites don't change: `buffer_field(Instance::from_input(di))` already gives the target's buffer
for a `@previous` read. Density functions don't change either.

**Refine kernels.** `DeviceCodegen` gets `generate_refine_kernel(&RefineRun)`, one per run, named `<unit fn>_<target>_refine_kernel`. It
works like a normal kernel with an extra `offset` parameter, and it adjusts the iterator's position only. Like every
kernel, it takes `out_off` and the input offsets of `specs/workers.md` §4.3, with `out_off` being the target's slot. No
runtime changes are needed, since these all use existing fields:

| Backend | Kernel change |
| --- | --- |
| Rust | Per cell: `let it = PositionIterator { pos: it.pos + offset, idx: it.idx };` before calling the density function |
| Java | `p.set(ix + ox, iy + oy, iz + oz, index)` |
| CUDA | After `tw::make_position`, add `offset` to `pos.pos` field by field, so it needs no `int3` operator from the runtime. Same `launch_shape`. `offset` is `int3`. The bounds check remains on `local` |

**Workers.** Emitted only for entries with refinements:

| Backend | Stored origin | Refine call |
| --- | --- | --- |
| Rust | Worker field `origin: Vec3`, set by `run` | `fn refine_<name>(&mut self, offset: Pos3, host_consts: &<Base>HostConsts)`, same return type as `run` |
| Java | Worker field, set by `run` | `refine_<name>(int ox, int oy, int oz, <Base>HostConsts hc)`. Returns the pool array, with the same overwrite rule as `run` |
| CUDA | `double3 origin` in the worker's `Impl`, set on the host when enqueued | `Status enqueue_refine_<name>(Pos3 offset, const <Base>HostConsts&)`. Same host-constant upload, ordered on the stream, followed by a copy to the result buffer. Results are read through `result()` after `sync()`, with the same validity rule as `enqueue` |

### 6.3 Caller contract

These are documented preconditions, with no generated checks:

- `refine_*` runs after `run` on the same worker. Otherwise values are unspecified, and Java may throw on a record
  buffer.
- Every offset component is within `[0, march]`. Outside it, a mapped read can address the wrong cell or go out of
  bounds, just as an invalid mapping can on the low-level path.
- A runtime failure in a refine call behaves like one in `run`: a panic, an exception or a sticky CUDA error. The
  worker is then unusable until a new one is created (CUDA) or `run` has run again (Rust, Java).

## 7. Interpreter

### 7.1 Sequence of ordinary core programs

`tungsten-core` doesn't change. `tungsten-check` gets a driver that runs one **core program per call**, keeping one store
of buffers per entry, keyed by instance, between calls. That matches workers, which never share buffers across entries:

```rust
pub fn eval_sequence(prepared, case: &SequenceCase, ...) -> Vec<StepResult>;
```

- **Run step.** The normal program, plus hidden entries for instances in §4.5 that aren't entries. Results fill the store
  of every entry whose schedule contains that instance. Hidden entries are excluded from what's compared.
- **Refine step.** A separate lowered program per refinement, built once and checked with `check` like any other. Its
  entry is the refined unit at the target's grid, and the rerun instances are its inputs. A step is evaluated with the
  buffers of its entry's store as hosts, and its results update that store.
- A step's failure ends the sequence. The case's result is an error, as in `TESTING.md` §6.

### 7.2 Lowering a refine step

| Source | Lowered |
| --- | --- |
| `Ambient(Pos3)` in a running unit, including the ones the mapping desugaring produces | `Binary(Add, Ambient(Pos3), Host(offset))`, where `offset` is a new `Pos3` host |
| Read of a frozen input or a `@previous` input | `Index(Host(buffer), index)`, where `buffer` is a new host of type `Array(T, n)` |
| Its index when un-indexed | `flat` over the **raw** `Ambient(Pos3)`, or `0` for a `(1,1,1)` input. This matches the kernel's index |
| Read of a rerun input | `Density { .. }` as usual, so core's `ctx.k` gives the index |

The new hosts come after the IR's hosts and the value-autoconst hosts (`specs/auto-consts.md` §4.3), so those `HostId`s
are the same in every step. They're added to the scoping lists of the unit and its functions. `LowerMap` records them, so diagnostics keep
using source names. Density-mappings' desugaring runs first and produces an index over `Pos3`, which then gets the
offset.

### 7.3 What is and isn't proven

Each step is an ordinary core program, so T1–T5 apply to each step. The driver that sequences them, the store and this
lowering are trusted, as the rest of `tungsten-check` is. The differential suite covers them. Core semantics for
sequences are planned follow-up work (§12).

### 7.4 Cost

`Expr::Host` clones the whole value (`exec/expr.rs`), so each read of a frozen buffer costs `O(n)`. A step is quadratic in the frozen
buffer's size. Corpus grids stay small, and vanilla-sized march tests are `@tier(large)` at most. Fixing this means
a core change, for example reading an index without cloning, so it waits for the core follow-up.

## 8. Other crates

### 8.1 Optimizer

`perform_inline`: new `InlineRejection::Refinement`, checked in `Planner::plan`, for a callee that is:

- a `with` unit;
- listed in any `reruns`;
- read directly by a unit that runs in a refinement.

Inlining a frozen input into a rerun unit would recompute it at each step, changing its value. Inlining deeper in a
frozen closure stays allowed and doesn't change frozen values. `consumer_grid` also counts a `with` unit's target grid as
a consumer. `inline` never touches `previous` inputs, since their target is listed in `reruns`.

`crates/tungsten-optimizer/SPEC.md`, which isn't implemented yet either, amends as follows. Whichever lands second does the integration:

- **`perform_dce`:** `with` units and instances reachable in §4.1 and §4.5 are roots. It removes a `reruns` entry that becomes
  unreachable after unread inputs are removed, unless it's the entry's own. An unread `@previous` input is removed like any other.
- **`perform_cse`:** nothing new. Reads of one input in a cell give one value, including `@previous` reads, since writes
  happen after the function returns. Units and inputs aren't merged.

### 8.2 Text format

`crates/tungsten-parse/SPEC.md` §4 and `TESTING.md` §8. `refinement`, `of`, `rerun` and `with` are contextual, so they don't
become reserved words. `@previous` is an input attribute that may appear anywhere among the input's attributes, at most once.

```
item         = ... | refinement ;
refinement   = "refinement" label "of" label march? "{" rerun* "}" ;
march        = "@march" "(" UINT "," UINT "," UINT ")" ;
rerun        = "rerun" label grid_attrs ("with" label)? ";" ;
directive    = ... | "@refine" STRING label "offset" "(" int "," int "," int ")" ("host" "(" binding ("," binding)* ")")* ;
expect       = "@expect" STRING ("after" "(" UINT ")")? expectation ;
```

- A missing `@march` is all zero. Grid attributes are written out, as lists aren't derived anywhere in the format.
- `@refine` directives of one case run in source order. A host not bound in `@refine` keeps its value from the previous
  call. Only a Static host is allowed, as in `@case`.
- `@expect "c" after(n)` checks the entry named by the expectation after the `n`th `@refine`. Without `after`, it refers to the
  orchestrate step, as today. An `after(n)` expectation must name the entry of that `@refine`'s refinement.
- At load time, an offset outside `[0, march]` or a `@refine` naming an unknown refinement is an error. Test files can't
  violate §6.3 by accident.

### 8.3 Test harness

Root `SPEC.md` §7:

```
request  := origin:[u64;3] param* host* steps:u32 step*
step     := refinement:u32 offset:[i32;3] host*
response := 0x00 value* (entry order) (step_value*)   // the step entry's buffer per step
          | 0x01 kind:u8 len:u32 utf8[len]
```

`steps` is always present, and is 0 for a program with no refinements, so the protocol golden file is regenerated once. The
driver builds a fresh `Orchestration` per case and one worker per expected entry. It runs each worker at the case's origin, then
runs each step on the worker of that step's entry. Hosts sent are complete for each step, resolved from the case's carried values by the harness. Scoring stays per
case (§6.3 of that spec): a failure at any step fails the case. A case with `OutOfFuel` at any step isn't sent to a backend, and
one with a cell error isn't sent to CUDA.

### 8.4 Others

| Crate | Change |
| --- | --- |
| `tungsten-inspect` | IR text prints refinements and `@previous`. Graphs draw `@previous` as a dashed self-edge and mark frozen and rerun instances per refinement. The schedule analysis lists refine waves |
| Documentation | `SEMANTICS.md` §3 and §9 note that refinement is desugared before core. Update `tungsten-check/README.md`, the root `README.md`, `tungsten-cuda/README.md` (host contract) and `docs/redesign.md`'s orchestration notes |

## 9. Testing

All corpus files run through the existing trials: `roundtrip`, `lowering`, `interp`, `inline`, and each backend on both stages.
Expected values are computed by hand.

### Corpus

New `refine/`:

| Concern | Files |
| --- | --- |
| Reuse | A frozen buffer keeps its value when a host changes between calls, while a rerun one takes the new value |
| Rerun | Rerun with the same unit; with a refined unit; a chain of reruns; frozen reading rerun keeps its old value |
| `@previous` | Accumulator over several calls; first value from the normal unit; two reads in a cell; a record element; in a helper; a `(1,1,1)` entry |
| March | `Pos3` moves while the index doesn't; top-down order; thick windows (`dims.y = 2`); x and z axes; march 0 with zero offset |
| Mappings | `@lerp` along the march across a frozen grid, compared with the same grid read at full resolution; `@expand` with `Repeat` on the marched axis; last valid offset; one input with two mappings |
| Schedule | A frozen instance read only by a refinement is computed by orchestrate; a stateless rerun-only instance isn't; a failing frozen buffer fails orchestrate |
| Multiple | Two refinements on one entry, sharing state; two entries, one refined; two refined entries sharing an instance, whose buffers stay separate; `run` restart |
| Inlining | Frozen input not inlined into a rerun unit, and values unchanged; inlining deeper in a frozen closure still happens |

New `reject/` files, one per error in §5, plus a mapped read whose range fails only with `march` included.

### Other checks

- **Mutation checks on the real path**, done once and recorded. Each must turn the corpus red on each backend:
  adding the offset to the index; dropping the offset; rerunning a frozen instance; a `@previous` read from the wrong buffer.
- **Marched slices equal full grid.** A corpus test compares a program that marches through a volume, slice by
  slice, with the same unit evaluated as a full 3D entry, by bits. It is the main check of the design's premise.
- **Worker isolation.** Creating a second worker of another entry between `run` and `refine_*` leaves the first worker's
  march unchanged.
- **Regression check while developing.** Generated output for the existing corpus and L2 snapshots don't change. This is a
  development aid, not a compatibility requirement.
- **Text.** Parser and printer round-trip, the generators in `tungsten-parse/tests/common/mod.rs`, and `isomorphic` over `refinements`.
- **IR.** `Clone` and `rebuild` keep refinement refs valid.
- **Interface.** Build twice and compare; `refine_waves` respects dependencies and excludes frozen instances; the §4.5
  schedule; `entry_hosts` includes `with` units.
- **L2.** Each refinement's lowered step program gets its own snapshot, named `<path>.<refinement>`.
- **Verus.** `scripts/verify.sh` needs no change, since `tungsten-core` is untouched.

### Milestones

Each lands green, and they start after density-mappings.

1. **Model.** `Refinement`, `previous`, retag and rebuild, parser and printer, the builder API (§3) with `previous` in its
   merge key. Lowering rejects a declared refinement as unsupported.
2. **Interpreter.** Validation, §7, inliner rule, corpus, `@refine` and `after`. Backend gaps are `@knownfail`.
3. **Interface and Rust.** `refine_waves`, names, kernels, worker methods, driver and protocol. Its `@knownfail`s expire.
4. **Java.**
5. **CUDA.**
6. **Inspect, and optimizer integration if DCE and CSE have landed.** Documentation lands with that of the other v0.3 specs.

## 10. Effects on `specs/density-mappings.md`

The `march` reader rule, the marched `p`, the `@previous` restriction, lerp helpers for refined units and the inliner's
check order are written into that spec. Milestone 2 here turns them on.

## 11. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| What moves | `Pos3` moves; origin and scales are fixed | Moving the origin (first idea): frozen buffers go stale, and `@lerp` reads `t = 0` on a 1-thick slice, since it's computed from `Pos3`. An `along()` declaration with origin-scale checks: extra machinery for a problem that goes away. An origin-aware lerp: changes the mapping semantics and exactness rules |
| Refine API | `refine(offset, hosts)`, origin stored at orchestrate | Re-passing the origin: a parameter that can only be wrong. Allowing both: brings back stale buffers |
| Declaration | Refinement table with a rerun list per instance | Role on each input edge: two edges can disagree. Flag on the unit: can't differ per refinement |
| `@previous` | Current cell only, written at function end | Snapshot copy: unneeded work for what the use case reads. Ping-pong: changes kernel signatures and buffer access. Arbitrary-index reads: needs one of those |
| Interpreter | Desugar outside core now, core semantics later | Core now: widens proof work already in progress, and every theorem would need restating |
| Early refine | Documented precondition | A runtime guard: extra state for a mistake that a caller's first test will show. Implicit init: which origin and hosts it would use is unclear |
| Reuse outside the entry's closure | Orchestrate computes it | Rejecting: the init unit would need a real read, which DCE or inlining could remove. A separate `prepare` call: another precondition |
| Rerun scales | Reject a mismatch on marched axes | Scaling the offset: per-instance offsets, and rounding to define across backends. No rule: silently misaligned |
| Multiplicity | Several named refinements per entry, sharing buffers | One per entry: little simpler, and names would change on relaxing it |
| March start | Orchestrate unchanged, init is the seed state | An offset on orchestrate: frozen buffers computed at an offset conflict with reading them by absolute `Pos3` |
| Inlining | Conservative | Inlining rerun-only chains: cheaper steps, more rules. Revisit once measured |
| Spec location | `specs/refinement.md` | Root `SPEC.md`: holds the testsuite design, which still has deferred work |

Decided without an interview question, and easy to revisit: the offset parameter being present when `march` is zero; `march`
being a maximum offset rather than a virtual grid size; offsets being non-negative; a refined unit being unable to read another
instance's `@previous`; `with` units getting their own kernels per target; contextual keywords; `@refine` hosts carrying over;
`after(n)` for expectations; and the Java parameters being three ints, matching how the entry methods take the origin.

## 12. Open and deferred

- **Frontend adoption.** `tungsten-mc-compile` can declare refinements once it's ported to `tungsten-builder`
  (`specs/builder.md` §9). `find_top_surface` is the first candidate.
- **Core semantics** for sequences, once T1–T5 land: a verified step semantics, and a host or frozen-buffer read without a clone (§7.4).
- **Unmeasured:** march cost compared with an eager 3D buffer. Each step launches its kernels, so the cost per step matters on CUDA. Measure before
  the frontend moves. On CUDA, wave fusion would reduce it.
- **Relaxing inlining** for rerun-only chains (§8.1).
- **Multi-rate reruns** along the march, if the frozen `@lerp` pattern turns out not to be enough.
- **Per-column offsets**, if skipping finished columns through `@previous` costs too much.
- **Whether a frozen instance can be declared with an explicit `reuse` line,** to catch a marked instance omitted by accident.
  Unlisted currently means frozen, silently.

## 13. Implementation notes

### Milestone 2 (`specs/combined-rollout.md` phase 2A)

- Errors carry `Item::Refinement` for per-refinement rules, `Item::Input` for the `@previous` rules and
  `Item::Program` for `RefinedUnitInNormalGraph`.
- `RefineBadEntry` and `RefineBadMarch` have unit tests on hand-built IR instead of `reject/` files, since text
  can't express them (`specs/combined-rollout.md`, phase 1 notes).
- A step program adds its offset and buffer hosts to every unit's and function's host list, not only those
  that read them. Units that don't run in the step are never evaluated, so this changes nothing.

### Milestone 3 (`specs/combined-rollout.md` phase 2B, step 6.3)

- `Interface::worker_schedule(entry)` returns only what `run` computes (§4.5). The rerun-only instances of §6.1 have slots
  and buffers, but aren't in it, since `run` iterates it.
- A refine kernel's `offset: Pos3` parameter follows `origin`. Kernel names are
  `<unit fn>_<target base>_refine_kernel`.
- Marching `idx` by the offset went unnoticed by the corpus until `refine/march_identity_reads.tw`, which reads a
  frozen and a `@previous` buffer on a grid wider than one cell at a non-zero offset.
