# Combined rollout: workers, density mappings and refinement

Status: in progress, phases 1 and 2A done. Implements `specs/workers.md`, `specs/density-mappings.md` and `specs/refinement.md` together,
one layer at a time. Run it with `/implement specs/combined-rollout.md`, one phase per run.

This file only decides **order** and the **temporary states** that order creates. All behaviour, names, error kinds and
tests come from the three specs. Where they disagree with this file about landing order or milestone grouping, this file
wins. On anything else, they win.

## 1. Summary

The three specs plan to land one after another, each going through every backend. This plan cuts across them instead:

| Phase | Layer | Contents | Replaces |
| --- | --- | --- | --- |
| 1 | Model | IR, text format, builder, `Interface` worker layout | Density M1, refinement M1, workers M1 |
| 2A | Interpreter | Validation, desugaring, inliner rules, corpus | Density M2, refinement M2 |
| 2B | Rust | Workers, then mappings, then refinement | Workers M2 and M5 (Rust), density M3, refinement M3 |
| 3 | Java | Same order as 2B | Workers M3 and M5 (Java), density M4, refinement M4 |
| 4 | CUDA | Same order as 2B | Workers M4 and M5 (CUDA), density M5, refinement M5 |
| 5 | Closeout | Inspect, documentation, spec status | Workers M6, density M6, refinement M6 |

Workers has no IR change. Its M1 is the `Interface` layout, which belongs in phase 1 because it changes no generated code.

Inside a backend the order is always **workers, mappings, refinement**. That keeps `specs/workers.md` D17: lerp helpers
and refine calls are written once, on workers, and never on the old `Orchestration`.

### Goals

- Every phase ends with `cargo test --workspace` green on the backends whose toolchains are installed.
- No backend ever emits silently wrong code for a feature it doesn't implement yet (§3.1).
- Each spec's testing section is fully covered by the end.

### Non-goals

Everything the three specs list as non-goals or deferred stays out. In particular: triggered stages, porting
`tungsten-mc-compile` or `tungsten-mc`, changes to `tungsten-core`, and `perform_dce` / `perform_cse` integration, since
neither pass exists on this branch (`specs/refinement.md` §8.1, "whichever lands second").

## 2. Current state

Checked against the code on `v0.3` at `ae393a1`:

- `WorkerLayout`, `DensityMapping` and `Refinement` don't exist yet.
- `specs/compiler.md`, `specs/auto-consts.md`, `specs/builder.md` and `specs/program-level-inputs.md` are implemented.
- The vanilla mapping scan that gates density M2 isn't recorded anywhere (§5.1).
- Backend trials generate through the low-level path, which doesn't validate. So lowering rejecting a feature doesn't
  protect a backend from it (§3.1).

## 3. Temporary states

The per-spec plans didn't need these, because each feature finished every backend before the next began.

### 3.1 Backend guard

From phase 1 until a backend's own phase, it must refuse programs using a feature it doesn't implement.

Add one helper in `tungsten-interface`, called at the start of each backend's generation, with that backend's list of
implemented features. It returns `BackendError::Codegen` naming the feature for:

| Feature | Refused when | Dropped in |
| --- | --- | --- |
| Mappings | Any input has a mapping other than `Identity` or `Expand([Repeat; 3])` | Rust 2B, Java 3, CUDA 4 |
| Refinement | `refinements` is non-empty, or any input has `previous` | Rust 2B, Java 3, CUDA 4 |

- **`Expand([Repeat; 3])` must pass the guard.** `specs/density-mappings.md` §6 adds it to existing corpus files such as
  `density/dims_one.tw`. `Repeat` needs a size-1 axis, so the input is `(1,1,1)`, and the existing cell-0 branch already
  generates the right code. Refusing it would break those files on Java and CUDA between 2A and their own phases.
- Without the guard, a case could pass by accident (for example a refine case whose run step is right) and trip the
  stale-`@knownfail` error at random. The guard makes every such file a `GenFailed` for every case.
- Delete the helper in phase 4, once all three backends implement everything.

### 3.2 Interim `@knownfail`

New `density/map_*` and `refine/*` corpus files get one file-level note per backend still pending:

```text
@knownfail(java, "pending: combined-rollout phase 3")
```

- Use exactly this reason format, so a phase can find its own notes with a search.
- A phase removes its backend's notes **in the same commit** that makes them pass. A covered case that passes is an error.
- These aren't bugs, so they don't go in `PAPER.md`. A real backend gap found along the way follows
  `docs/testing-policy.md` as usual: its own narrow note, logged in `PAPER.md`.
- Worker corpus files (`specs/workers.md` §8) are ordinary programs and should pass on the old Java and CUDA API too. If
  one doesn't, investigate before adding a narrow note.

### 3.3 Harness protocol

`specs/refinement.md` §8.3 adds `steps` to the request, and all three drivers share that protocol, while only Rust moves in
phase 2B. Today every driver reads a fixed `REQUEST_LEN` and `run_cases_opts` takes one `response_bytes`.

In phase 2B:

- `protocol.rs` always sends `steps`, and `tests/golden/protocol.txt` is regenerated once.
- Response length becomes per case, since it depends on the case's steps.
- The Rust driver and `proto.rs` read steps properly.
- Java and CUDA only add 4 to `REQUEST_LEN` and ignore the value. The guard guarantees it is 0 for them.

Phases 3 and 4 replace that with real step handling.

### 3.4 Mixed generated APIs

Between 2B and 4, backends expose different generated APIs. That's expected (`specs/workers.md` §8): harnesses move per
backend. `Interface` keeps whatever the unmoved backends use until the last one has moved.

## 4. Phase 1: Model

No generated code changes. Corpus results and L2 snapshots don't change.

### Density mappings (M1)

- [x] `AxisFactor`, `DensityMapping`, `DensityInput::mapping`, `DensityMapping::is_identity_on` (§2, §5.4)
- [x] Parser and printer for `@expand` and `@lerp` (§5.5), and the generators in `tungsten-parse/tests/common/mod.rs`
- [x] `InputSpec::mapping` in the builder, joining the input merge key (§5.6)

### Refinement (M1)

- [x] `Refinement`, `Rerun`, `OptimizedProgram::refinements`, `DensityInput::previous` (§3)
- [x] `retag.rs` and `rebuild.rs`, with a test that `Clone` and `rebuild` keep refinement refs valid (§9)
- [x] Parser and printer for `refinement`, `@march`, `rerun` and `@previous`, with contextual keywords (§8.2). `@refine` and
      `after` wait for 2A
- [x] `isomorphic` over `refinements`, and the generators
- [x] Builder: `InputSpec::previous` in the merge key, `pb.refinement`, `RerunSpec` (§3)

### Shared

- [x] Lowering rejects a non-`Identity` mapping and any refinement with one temporary `LowerErrorKind`, covered by unit
      tests only (no `reject/` file, since the kind goes away in 2A)
- [x] The backend guard (§3.1), with unit tests

### Workers (M1)

- [x] `WorkerLayout`, `Pool`, `Slot`, `Interface::worker`, `slot`, `worker_schedule`, `input_params`, `entry_hosts` (§4)
- [x] Naming for workers, host consts and factories in `tungsten-interface::naming` (§2)
- [x] The Interface and Naming test rows of §8

`worker_schedule` is the plain closure here. Refinement extends it in 2B.

**Exit.** `cargo test --workspace` is green, and generated output for the existing corpus is byte-identical to the start of
the phase on all three backends. `tungsten-core` is untouched.

## 5. Phase 2A: Interpreter

### 5.1 Gate: vanilla mapping scan

Do this first. See `specs/density-mappings.md` §8 and `specs/program-level-inputs.md` §8.

- [x] Using `tungsten-mc-compile` in `../tungsten-frontends` on v0.2, list every un-indexed read whose reader dims differ
      from the input's dims and whose input isn't `(1,1,1)`
- [x] Record the list in the Implementation notes of `specs/density-mappings.md`
- [x] **Stop and ask** about each read found. Strict 1-to-1 isn't merged until each one is decided

### 5.2 Mappings (M2)

- [x] Validation with the new `LowerErrorKind` variants (§4). Compute reader pairs through one function that includes
      `march`, which is 0 until §5.3 fills it in
- [x] Desugaring in `tungsten-check/src/lower/expr.rs` (§5.1)
- [x] `InlineRejection::MappedRead` in `Planner::plan` (§5.4)
- [x] Corpus migration (§6): `dims_different.tw` becomes a reject file, and files reading a `(1,1,1)` input from a larger
      grid add `@expand(*, *, *)`
- [x] New `density/map_*` and `reject/` files (§8), with §3.2 notes for all three backends
- [x] Formula agreement test against `tungsten-mc`'s `interpolate`, by bits (§8)

### 5.3 Refinement (M2)

- [x] Validation with the new `LowerErrorKind` variants (§5), next to the mapping validation. Remove the temporary kind
- [x] Fill in `march` for the mapping reader rule (density §4.2)
- [x] `eval_sequence`, the per-entry store and refine-step lowering (§7)
- [x] `InlineRejection::Refinement`, checked before `MappedRead`, and `consumer_grid` (§8.1)
- [x] `@refine` and `@expect .. after(n)`, with their load-time errors (§8.2)
- [x] New `refine/` and `reject/` files (§9), with §3.2 notes for all three backends
- [x] One L2 snapshot per refinement, named `<path>.<refinement>` (§9)
- [x] Marched slices equal full grid, on the interpreter (§9)

**Exit.** `cargo test --workspace` is green. The interpreter and inliner pass every new file, since `@knownfail` can't excuse
them. Generated output for the existing corpus differs only in the §6 migration files.

## 6. Phase 2B: Rust

Three separate steps, each green before the next.

### 6.1 Workers (M2, plus Rust's part of M5)

- [ ] `BufferBounds` in `tungsten-interface` (§5.5), `.buffer_bounds(..)` on the Rust extension trait
- [ ] Generated API (§3.1), pools and offsets (§4.2, §4.3), per-entry hosts (§4.4), reads and writes (§5.1, §5.2)
- [ ] Remove the Rust items in §7
- [ ] Test runtime records derive `Default`, and its handles are `Send + Sync`
- [ ] Rust harness moves, compiling with `PerBuffer`
- [ ] §8 rows for Rust: shared instances, shared helper reads, pools, host consts, concurrency, bounds modes, static
      assertion, records, large arrays, naming

### 6.2 Mappings (M3)

- [ ] `lerp_helper` and `lerp_helpers` in `Interface`, with naming (§5.2)
- [ ] Expand as an inline index and lerp helpers on pools (§5.2), following `BufferBounds`
- [ ] `BackendError::Codegen` for what can't be emitted (§5.3)
- [ ] Drop the guard's mapping check for Rust and remove its §3.2 notes
- [ ] Mutation checks, done once and recorded: swapped lerp axes, and a `t == 0` shortcut (§8)

### 6.3 Refinement (M3)

- [ ] `Interface` (§6.1): discovery ignoring `previous`, rerun-only instances, the extended `worker_schedule` and
      `entry_hosts`, `refine_waves`, `refine_kernel`, `refinement_names`, `lerp_helpers` covering refined units
- [ ] Rust refine kernels, the stored origin and `refine_<name>` (§6.2)
- [ ] Protocol and driver, as in §3.3 above
- [ ] Drop the guard's refinement check for Rust and remove its §3.2 notes
- [ ] Mutation checks (§9), the marched-slices check and worker isolation on Rust. Interface tests of §9

**Exit.** `cargo test --workspace` is green with no Rust `pending` notes left.

## 7. Phase 3: Java

- [ ] **Workers (M3).** §3.2 and §5.3, the `.buffer_bounds` setter, `size.rs` including the new parameters, the 255-parameter
      `BackendError::Codegen`, harness move, and the Java §8 rows, including the `ExecutorService` concurrency test
- [ ] **Mappings (M4).** Expand and the `private static` lerp helpers. Drop the guard and notes. Mutation checks
- [ ] **Refinement (M4).** Refine kernels and `refine_<name>`, real step handling in the driver and `Proto.java`. Drop the
      guard and notes. Mutation checks, marched slices, worker isolation

**Exit.** `cargo test --workspace` is green with no Java `pending` notes left.

## 8. Phase 4: CUDA

Needs `nvcc` and the GPU.

- [ ] **Workers (M4).** §3.3 and §5.4, the per-worker `<Base>Layout`, shared device autoconsts, worker streams, `Option<BufferBounds>`,
      harness move, removal of `BufferLayout`, and the CUDA §8 rows, including two workers on two host threads
- [ ] **Mappings (M5).** Expand and the `__forceinline__` lerp helpers. Drop the guard and notes. Mutation checks
- [ ] **Refinement (M5).** Refine kernels, `enqueue_refine_<name>`, real step handling in the driver and `tw::proto`. Remove
      notes. Mutation checks, marched slices, worker isolation
- [ ] Delete the guard helper (§3.1) and remove anything in `Interface` only the old APIs used

**Exit.** `cargo test --workspace` is green with CUDA enabled, and a search for `pending: combined-rollout` finds nothing.

## 9. Phase 5: Closeout

- [ ] `tungsten-inspect`: mappings (density §5.6) and refinements (refinement §8.4)
- [ ] Documentation for all three: workers M6, the density §5.6 table, the refinement §8.4 table
- [ ] Update the three specs' status lines and add Implementation notes for anything that differed
- [ ] Final walk through each spec's testing section

## 10. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Order inside a backend | Workers, mappings, refinement | Any other order writes lerp helpers or refine calls on the old API and rewrites them |
| Unimplemented features on a backend | Explicit guard returning `BackendError::Codegen` | Relying on `@knownfail` alone: unguarded backends emit wrong code, and accidental passes make the notes flaky |
| Concurrency tests | With each backend's workers step | A separate M5 at the end: leaves the thread-safety claim unchecked for two whole phases |
| Protocol | Changed once in 2B, Java and CUDA ignoring `steps` until their phase | One protocol version per backend: more harness code that is deleted two phases later |
| Mapping validation's `march` rule | One reader-pair function from the start | A temporary 0-only rule, rewritten in the same phase |
| Inspect and docs | Last | Per phase: the generated API and text are still moving |

## 11. Implementation notes

Add deviations here as phases land, and in the relevant spec if they change its design.

**Phase 1.**

- Lowering's temporary kind is `LowerErrorKind::NotSupportedYet`. It also refuses a `@previous` input in a program with no
  refinement, which §4 didn't list: such an input would otherwise lower as an ordinary read. 2A removes the kind.
- `DensityMapping::is_identity_on` counts `Repeat` only on an axis of size 1. That is the one use of its `input_dims`
  parameter, and `Repeat` on a wider axis is invalid anyway.
- `naming::worker_names` also fixes `<Base>Layout`, for CUDA. A `FactoryStyle` gives the spellings in `specs/workers.md` §3.
  If any of a worker's three type names is taken, all three move to `<Base>_N`, so one entry keeps one stem.
- **For 2A:** `specs/refinement.md` §9 asks for one `reject/` file per error, but two can't be written in text.
  `RefineBadEntry` needs an entry index no label can name (an unknown label is an elaboration error), and `RefineBadMarch`
  needs a negative `@march`, which the `UINT` grammar refuses. They need unit tests on hand-built IR. A negative factor
  has the same limit, but `MappingBadFactor` is still reachable with factor 0.
- Exit criterion checked by generating all 86 corpus programs before and after the phase, on three backends and two stages
  (516 outputs, driver included), and comparing bytes: no differences.

**Phase 2A.**

- **Vanilla gate.** The scan found no un-indexed read across shapes (recorded in `specs/density-mappings.md` §10),
  so there was nothing to ask about.
- **Migration beyond §6.** `density/shared_dims.tw` read a `(4,1,1)` input un-indexed from a `(2,2,1)` entry. That
  read is now illegal, so its `square` entry uses an explicit index with the same values. Unit tests and
  `tungsten-parse/tests/corpus/basic.tw` that relied on flat-number reads across shapes moved the same way.
  `tests/snapshots/density__dims_different.snap` is no longer referenced, since that file is now a reject file.
- **Vanilla shapes** are two `@tier(large)` files in `large/`, not `density/`: backend trials build one batch per
  directory and run the interpreter on all its files, so a slow file slows the whole directory. The lerp file
  (`(5,49,5)` to `(16,384,16)`) takes about 12 minutes in a release build with memo off, as density-mappings §5.1
  predicts.
- **Interpreter API.** `Lowered` has `instances`, `visible` and `steps` (`LoweredStep`, one per refinement).
  Hidden entries come after the program's entries in the orchestrate program. `Prepared::eval_sequence` takes a
  `SequenceCase` (complete hosts per call) and returns a `SequenceResult`. A step in which any cell fails, hidden
  entries included, ends the sequence. The testsuite's `Interp` has `steps` next to `results`.
- **Inliner.** `InlineRejection::Refinement` is checked first in `Planner::plan`, before `sole_user`, so a `with`
  unit reports it instead of `Unused`. Every unit listed in some `reruns` counts as running. That is a superset of
  the reachable ones, and an unreachable one is a validation error anyway.
- **Text.** `after(n)` counts from 1, and `after(0)` is a load error. A `@refine` naming a refinement name that
  several entries use is a load error. `specs/refinement.md` §2's example uses `main` as an entry label, which is a
  keyword. The corpus uses other labels.
- **Left for 2B.** The rule that a case with `OutOfFuel` at any refine step isn't sent to a backend (§8.3) only
  checks orchestrate today. The guard makes every refinement program a `GenFailed`, so it can't matter yet.
