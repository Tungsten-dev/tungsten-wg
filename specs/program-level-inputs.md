# Program-level density inputs

Status: design agreed. Milestone 1 implemented; milestones 2–6 not implemented. Lands before `specs/density-mappings.md` and `specs/refinement.md`,
which already assume this design.

## 1. Summary

Density inputs move from an arena inside each compute unit to one arena on the program. A unit lists the
inputs it uses, as it already does for hosts, tables and constants.

```rust
// Before
ComputeUnit::DensityFunction { density_inputs: Arena<DensityInput>, .. }

// After
OptimizedProgram { density_inputs: Arena<DensityInput>, .. }
ComputeUnit::DensityFunction { density_inputs: Vec<Ref<DensityInput>>, .. }
```

This has two effects:

- **Simpler code.** Nested arenas, the per-unit `find_density_input` searches, the inliner's ref remapping and
  the placeholder variables all go away.
- **Sharing.** Several units can list one input, and a helper shared between units can read it if each of those
  units lists it.

This version also deletes `tungsten-legacy`, as the last code milestone here (§6.8). `tungsten-mc-compile` stays on a
pinned version from before the deletion until it's ported to `tungsten-builder` (`specs/builder.md`), which is separate
work in `tungsten-frontends`.

### Goals

- One input arena per program, matching core's single `inputs` table, so IR inputs map 1:1 to core inputs.
- Shared inputs and shared-helper reads, with the same results on the interpreter and every backend, bit for bit.
- Programs that don't go through the inliner generate byte-identical code.
- `tungsten-legacy` and `VariableType::DensityInput` are gone at the end, and nothing in this repo needs them.
- Milestones 1–4 need no change in `tungsten-mc-compile`.

### Non-goals

- Merging structurally equal inputs (§9).
- Porting `tungsten-mc-compile` to build IR directly. The work is in `tungsten-frontends`, and `specs/builder.md` §9
  records what a port must show.
- Extending the legacy model before it's deleted, or relaxing its `DensityInputInHelperFunction` error.
- Changing `Instance`, buffers, scheduling or the backend runtimes.
- Byte-identical code for inlined programs. Their values must still match bit for bit (§7).

## 2. Example

```text
program {
    fn base_main() -> i32 {
        return ((ambient(pos3).x * 10) + ambient(pos3).y);
    }
    unit base {
        main base_main;
    }

    input b = base @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);

    fn shared() -> i32 {
        return density(b);
    }

    fn left_main() -> i32 {
        return shared();
    }
    unit left {
        main left_main;
        helpers [shared];
        inputs [b];
    }

    fn right_main() -> i32 {
        return (shared() + density(b));
    }
    unit right {
        main right_main;
        helpers [shared];
        inputs [b];
    }

    entry left @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry right @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
```

Today core rejects this program, because `shared` is listed by two units and reads an input.

## 3. Data model

### 3.1 `crates/tungsten-ir/model.rs`

```rust
pub struct OptimizedProgram {
    // existing fields
    pub density_inputs: Arena<DensityInput>,
}

pub enum ComputeUnit {
    DensityFunction {
        // In build order. Other fields are unchanged.
        density_inputs: Vec<Ref<DensityInput>>,
        ..
    },
}

pub struct DensityInput {
    pub name: Name,                  // replaces `var: Ref<Var>`
    pub density_function: Ref<ComputeUnit>,
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
    pub dimensions: (i32, i32, i32),
}
```

| File | Change |
| --- | --- |
| `retag.rs` | The program's input arena joins the tag map. `walk_program` visits each input's `density_function`, and `walk_compute_unit` visits the list. `ComputeUnit` has no arena left, so its clone needs no retagging |
| `rebuild.rs` | `rebuild` copies the input arena at the top level. `float_bits` and `fingerprint` read it there, and input fingerprints become flat slot indices |

### 3.2 Placeholder variables and `VariableType::DensityInput`

`DensityInput.var` existed only to supply a name, so it goes away along with the placeholder variables. No
function's `variables` holds one any more.

`VariableType::DensityInput` has to survive until milestone 5, because `tungsten-legacy/src/model.rs` re-exports
the IR's `VariableType`, and the legacy builder and `tungsten-mc-compile` use this variant for legacy vars. Until
then, no valid IR program uses it:

- Lowering keeps rejecting it with `DensityInputType`.
- Backends keep their arms for it.
- The `density` type keyword stays, so the printer stays total. `vars x: density` parses, and lowering rejects it.

Milestone 5 deletes all of these (§6.8).

## 4. Semantics

### 4.1 Reads

A function running as part of unit `U` can read an input only if `U` lists it. The index rules in `SEMANTICS.md`
§9.3 don't change.

- **Shared functions.** A function in several units is checked against each of them, as for tables and constants.
- **Buffers.** A buffer depends only on the input, so a shared input is built at most once per case with memo on,
  and all readers see the same values. Its failure appears once in the diagnostics.
- **Build order.** Before `U`'s cells run, every input `U` lists is built in list order, as today (§9.2).
- **Different reader dims.** Units with different dims can share an input. Each read follows §9.3 for its own
  reader, including the flat-number warning there.

### 4.2 Unlisted inputs

An input that no unit lists is legal and inert. It has an `InputId` and is printed, but it is never built or
scheduled, so a failing source has no effect. Reading it is `InputNotListed`.

### 4.3 Duplicate listings

`inputs [a, a]` is legal, as duplicates are in the other unit lists. It means the same as `inputs [a]`, apart from
a second build when memo is off. The inliner counts a unit once per distinct input.

## 5. Core

`exec` never reads `owner`, so evaluation code doesn't change.

| Item | Change |
| --- | --- |
| `ir.rs` | Remove `InputDecl.owner` |
| `wf.rs` | Remove `wf_input_links` and its clause in `wf_program`. `wf_input` checks only `source` |
| `check/wf.rs` | Remove the input-link check and the owner bounds check |
| `check/scope.rs` | Remove the `InputReaderShared` branch and `SeenKind::Shared`. The owner test becomes "the unit's `inputs` contains this input", reporting `InputNotListed` |
| `check/error.rs` | Remove `WfInputLink`, `InputNotOwned` and `InputReaderShared`. Add `InputNotListed { unit, input }` |

The new error reads ``input `d` isn't in the inputs of unit `a_main` ``. Like the other "not listed" errors, it is
reported once per `(unit, input)`, at the first occurrence.

`acyclic.rs` needs no change. Its edges already run from functions to inputs and from inputs to source units, so a
cycle through a shared helper is reported as `Cycle`.

`scripts/verify.sh` must pass after this change. No proof outside `wf.rs` refers to the removed definitions.

## 6. Pipeline

### 6.1 Lowering (`tungsten-check`)

- Input ids follow the program arena's iteration order, as for other tables.
- `core::Unit.inputs` comes from the unit's list. A stale ref reports `DanglingRef { kind: DensityInput }` at
  `Step::List { list: "inputs", index }`, replacing today's silent `filter_map`.
- `LowerMap` takes names from `DensityInput.name`, falling back to `#N`. `input_vars` is removed.
- `check.rs` renders the new error and drops the removed ones.

### 6.2 `tungsten-interface`

`discover_instances` walks the unit's list and resolves each ref in the program arena. Dependency order is the list
order, so instance discovery, names and waves don't change for a program that wasn't inlined.

### 6.3 Backends

Each `find_density_input` becomes `program.density_inputs[r]`, including Rust's `FunctionConverter.density_inputs`
field. Generated code doesn't change. Helpers already take the uniform parameters, so shared-helper reads need no
new codegen, though no test has exercised them until now (§8).

Plain `compile` still doesn't validate. Reading an input the running unit doesn't list behaves as it does today: it
reads whatever the buffer holds, or panics in `buffer_field` if no scheduled instance owns the buffer.

### 6.4 Inliner (`tungsten-optimizer/src/inline.rs`)

| Step | Change |
| --- | --- |
| `sole_user` | Counts one use per `(unit, distinct listed input)` pointing at the callee. Unlisted inputs don't count. `MultipleUses(n)` keeps its meaning |
| `consumer_grid` | Reads grids from listed inputs only, so an unlisted input can't block inlining |
| Absorbing inputs | The caller's list loses `plan.input`, then gains the callee's list through `extend_unique`. Refs stay valid, so `remap` and the rewrite of callee bodies go away |
| Removing inputs | `plan.input` leaves the arena, since the caller was its only lister. Unlisted inputs whose source is the callee also leave, or they would dangle |

The `InlinePlan.input` lookup and `count_density_uses` don't otherwise change.

**What changes in inlined output:**

- The arena reused freed slots, so a later merge's inputs could land before older ones. The list always appends.
  Discovery, wave and name order can shift as a result.
- `inlined_density_{N}` counts a function's variables, so removing placeholders renumbers `N`.

### 6.5 Text format

`crates/tungsten-parse/SPEC.md` §4 and `TESTING.md` §8:

```
item      = ... | input ;
input     = "input" label "=" label grid_attrs ";" ;
unit_item = ... | ("helpers" | "tables" | "hosts" | "consts" | "inputs") "[" labels? "]" ";" ;
```

| Topic | Rule |
| --- | --- |
| Keywords | `inputs` becomes a keyword |
| Old syntax | `input` inside a unit is an error that points to the new form. `@var` is removed |
| Reads | Always `density(d)` or `density(d, index)`. `density(u.d)` and the "exactly one unit" rule are removed |
| Unlisted reads | Not an `ElabError`. The checker reports them, so `reject` files can name `InputNotListed` |
| Labels | Program-wide. `Name` follows the host row of SPEC §5.2: `x` is `Named`, `x#N` is `Prefixed`, `x##N` is a duplicate `Named`, `%N` is `Anonymous`. Inputs are no longer syntax-only, and `d<N>` generation goes away |
| Local variables | A variable and an input can share a label, since `density(...)` is its own syntax. The collision error goes away |
| Lists | Never derived. An omitted `inputs` list is empty |
| Printer | Input items go after functions and before units. `inputs` is the last unit list |
| Elaboration | Declare units, then inputs, then resolve `inputs` lists. References are order-independent |
| Source map | `inputs` is keyed by `Ref<DensityInput>`. The `(unit, input)` entity becomes `input` |

### 6.6 Legacy conversion, until milestone 5

Conversion stays working through milestones 2–4, so vanilla can still be compiled for the byte comparison (§8).
`tungsten-legacy`'s model and builder don't change.

- `density_input` inserts into `self.program.density_inputs` and pushes the ref to the unit's list.
- It sets `name` from `di.var.name` and no longer calls `declare_var`.
- Step 3, "Declared variables", skips vars of type `DensityInput`.
- The per-unit dedupe in `density_direct_map` stays as it is. Nothing merges across units.

### 6.7 `tungsten-inspect`

- IR text prints inputs as a program section.
- `InputName` holds a program-wide unique name and loses its `unit` field.
- `density_read` takes its unit from the function being analyzed.
- Per-edge flags such as `indexed` are computed per listing unit, since a shared input can be read differently by
  each one.

### 6.8 Removing `tungsten-legacy`

**Prerequisite.** `tungsten-mc-compile` is pinned to a tag on the last commit with `tungsten-legacy`, taken after
milestone 4 (`specs/builder.md` §8). It uses `tungsten_wg::spmt` and the re-exported macros in about 14 source files,
including `compile.rs` (`spmt::conversion::convert`), `main.rs` (`spmt::pretty`, `spmt::dag`) and the 5,000+ lines in
`transform_spmt/`, plus 35 macro uses. Its dependency on this repo is a path today, so without the pin it stops
compiling when milestone 5 lands. Porting it isn't a prerequisite.

| Area | Change |
| --- | --- |
| Crate | Delete `crates/tungsten-legacy` and its two entries in the root `Cargo.toml` (workspace member and dependency), plus its facade dependency |
| Facade (`src/lib.rs`) | Remove `pub use tungsten_legacy as spmt` and `pub use tungsten_legacy::{body, build, extern_fns, pos3, vec3}` |
| Dev-dependencies | Remove from `tungsten-rust`, `-java`, `-cuda`, `-inspect`, `-check` and `-parse` |
| Backend and inspect tests | `tests.rs` in each backend and in `tungsten-inspect`, and Rust's `model/tests/examples.rs`, move to `.tw` text through `tungsten-parse`, keeping what they assert |
| Legacy-only tests | Delete `tungsten-check/src/tests/legacy.rs` and `tungsten-parse/tests/legacy.rs`, as the path they cover is gone. Move any regression that isn't about conversion to the corpus |
| `VariableType::DensityInput` | Delete the variant, `TypeFail::DensityInput`, `LowerErrorKind::DensityInputType`, `RegistryError::DensityInputType`, each lowering and backend arm, and the `Density` type keyword in `tungsten-parse/src/tables.rs`. `density(...)` reads stay. `vars x: density` becomes a parse error |
| Comments | Remove mentions of legacy in `tungsten-rust/src/interned.rs` and `types.rs` |

Rust's `transformer.rs` mentions `spmt` only in a commented-out import, so that line goes too.

## 7. Compatibility

| Kind of program | Requirement |
| --- | --- |
| Not inlined | Generated code is byte-identical on all backends |
| Inlined | Values are bit-identical. Order and generated names may change |
| L2 snapshots | The only diff is removed `owner` lines. Corpus files keep their inputs in today's order, so ids don't move |
| Protocol goldens | No change |
| `tungsten-mc-compile`, milestones 1–4 | Compiles unchanged, as it uses only the legacy model and builds no `OptimizedProgram` literal |
| `tungsten-mc-compile`, milestone 5 | Needs the §6.8 pin. `tungsten_wg::spmt` and the macros are a breaking API removal, so this needs a version bump |

## 8. Testing

All corpus files run through the existing trials: `roundtrip`, `lowering`, `interp`, `inline`, and each backend on
both stages. Expected values are computed by hand.

### Migration

Ten `.tw` files with nested inputs move to the new form with the same expected values: eight in the testsuite corpus
(under `density/` and `errors/`) and `basic`, `inline` and `testing_md` in `tungsten-parse/tests/corpus`. In-code
fixtures and the `gen_program` generator in `tungsten-parse/tests/common/mod.rs` move too.

### New corpus: `density/shared_*`

| Concern | Files |
| --- | --- |
| Sharing | Two units list one input; a shared helper reads it from both (§2), which is the first such test on each backend |
| Grids | A shared input read from different reader dims, through a `(1,1,1)` input and through an explicit index |
| Failure | A failing shared input fails both readers, with one diagnostic |
| Eagerness | A listed, unread failing input still fails its unit |
| Unlisted | An unlisted input with a failing source has no effect |
| Duplicates | `inputs [a, a]` gives the same values as `inputs [a]` |
| Inlining | An input listed by two units blocks inlining (`MultipleUses`); a callee whose input the caller already lists leaves one listing; an unlisted input with the callee as source doesn't block it and is removed |

### New corpus: `reject/`

| Case | Error |
| --- | --- |
| A read of an input the unit doesn't list | `InputNotListed` |
| A shared helper reads an input only one of its two units lists | `InputNotListed`, naming the other unit |
| A read of an unlisted input | `InputNotListed` |
| An input whose source reaches itself through a shared helper | `Cycle` |

### Other checks

- **Byte comparison, done once and recorded.** Generate code for the corpus and for vanilla overworld from the
  commit before this work and after it, with inlining off, on all backends. They must be identical.
  This goes through legacy conversion, so it must happen at milestone 3, before the deletion.
- **Vanilla.** `check::prepare` accepts the vanilla overworld, inlined and not inlined.
- **Baseline for a later frontend port.** The §6.8 tag can regenerate vanilla's code, not inlined, on all backends. A
  port compares against that output to show that its IR means the same as conversion's (`specs/builder.md` §9).
- **After deletion.** `cargo tree` shows no `tungsten-legacy`, `cargo test` passes for the workspace with every feature, and
  `scripts/verify.sh` passes. Corpus results and L2 snapshots don't change at milestone 5.
- **Mutation checks, done once and recorded.** Each must turn the suite red: dropping the listed-input check;
  making `sole_user` count inputs once regardless of how many units list them; leaving a removed callee's unlisted
  input in the arena.
- **Text.** Parser and printer round-trip each `Name` kind. `gen_program` generates shared and unlisted inputs, so
  `isomorphic` and the parser's P7 slot-order property cover them.
- **IR.** `Clone`, `compact` and `permute` keep list refs and program-arena refs valid.
- **Lowering.** `tests/map.rs` drops `input_vars`. `wf_mirror.rs` and `check_errors.rs` follow the §5 changes.
- **Verus.** `scripts/verify.sh` passes after milestone 1.

### Milestones

Each lands green.

1. **Core.** §5 and the `check.rs` messages. Lowering omits `owner`, while IR inputs are still per unit. L2 snapshots
   update. A read of another unit's input now reports `InputNotListed`.
2. **Names.** `name` replaces `var`. Placeholders and `@var` go, conversion stops creating them, and labels follow the
   host rule, still per unit.
3. **Arena move.** §3.1 and §6, in all crates at once, since the type change can't be split. Corpus migration and the
   byte comparison happen here.
4. **Sharing.** New corpus and mutation checks, with any backend fixes they turn up. `tungsten-builder` lands after
   milestone 3 and before milestone 5, on its own milestones (`specs/builder.md` §10).
5. **Remove legacy.** §6.8. First tag the last commit with legacy and pin the frontend to it.
6. **Documentation.** `SEMANTICS.md` §9, `CHECK.md` (wf table, §6(e), error table), `tungsten-core/README.md`,
   `tungsten-check/README.md`, `tungsten-parse/SPEC.md` (§2, §4, §5, §6.1, §14), `TESTING.md` §8, `docs/redesign.md`
   (layout, dependency rule, decisions), the root `README.md` (workspace layout, usage) and `PAPER.md` where it
   describes the current state. Milestones 1–4 can ship their documentation early if milestone 5 waits a long time.

## 9. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Meaning of program level | Shared inputs, with core dropping `owner` | Shared with core unchanged: lowering splits inputs per unit, IR and core stop matching 1:1, and shared-helper reads stay rejected. Storage move only: no new capability |
| Core change | Accepted, unlike the other two specs' stance | `owner` isn't used by `exec` and no proof depends on it, so the cost is one `verify.sh` run and the checker edits |
| `var` | Replaced by `name: Name` | Keeping it: a shared input's var would point into an arbitrary function's arena |
| Removing legacy | In this spec, as the last code milestone, with the frontend pinned to the last legacy tag | Before this work: no vanilla path for the byte comparison. After these specs: density-mappings and refinement would have to extend or work around a crate about to go. Waiting for the frontend port: it has no owner or date, and would block both later specs |
| `VariableType::DensityInput` | Deleted with legacy at milestone 5 | Own enum in legacy in the meantime: type translation at each conversion site, for a crate about to be deleted |
| Compatibility | Byte-identical unless inlined | Byte-identical everywhere: keeps arena slot-reuse behaviour in the inliner permanently. Values only: gives up the byte comparison |
| Text format | Top-level `input` and a unit `inputs` list | Nested sugar as well: two forms for one thing |
| Unlisted inputs | Legal and inert | Rejected in lowering: every pass that drops listings would have to clean the arena |
| Merging | None | In legacy conversion: backends already share buffers through `Instance`, so it only helps the interpreter's memo and complicates the byte comparison. Optimizer pass: no measured need |
| Other specs | Edited in place | Amendments table: readers would combine several documents |
| Spec location | `specs/program-level-inputs.md` | Root `SPEC.md`: holds the testsuite design, which still has deferred work |

Decided without an interview question, and easy to revisit: ids in arena order; duplicate listings being legal; the
field name `density_inputs` on both the program and the unit; `InputNotListed` as the error's name; removing unlisted
inputs whose source is an inlined callee; the milestone split; and printing inputs before units.

## Implementation notes

- **Milestone 1.** Until milestone 3, the text format can't make two units list one input. So
  `check_errors::input_listed_by_two_units` edits the lowered core program to show that core now accepts a shared
  helper reading an input both of its units list. The `wf_mirror` agreement proptest no longer has an owner to
  break, so mutation 3 breaks an input's `source` instead. `CHECK.md`, `SEMANTICS.md` and `tungsten-core/README.md`
  still describe `owner` until milestone 6.

## 10. Open and deferred

- **Merging equal inputs.** An explicit optimizer pass could do this later. It must compare `mapping` and `previous`
  once those exist, and it changes `MultipleUses` counts.
- **Removing unlisted inputs.** This belongs in `perform_dce`, in `crates/tungsten-optimizer/SPEC.md`.
- **The frontend port.** Its owner and timing aren't set here, and it no longer blocks milestone 5. The builder macros'
  replacement is `tungsten-builder` (`specs/builder.md`). `tungsten-parse`'s printer and `tungsten-inspect` already
  print IR, so `spmt::pretty` gets no replacement.
- **Frontend use of shared inputs.** `tungsten-builder` merges equal inputs by default, so a ported frontend shares them
  and can read them from shared helpers.
