# TODO

State at the end of the first session: milestone 1 (Foundation) is done and green. See `SPEC.md`
at the repo root (§9.4 for the milestones, §14 for implementation notes) and `README.md` here.
Nothing is committed yet. TESTING.md, SPEC.md and docs/testing-policy.md were already uncommitted
before this work.

## Before starting

- Commit milestone 1 (suggested message in the last report: "Add tungsten-testsuite foundation
  (milestone 1)"), or at least decide how to split it from the TESTING.md/SPEC.md edits.
- Run `INSTA_UPDATE=no cargo test --workspace` to confirm the baseline is still green.

## Milestone 2: Rust differential (next)

- [ ] **`entry_method_names`.** Extract the inline name computation in
  `tungsten-rust/src/orchestration.rs` (~line 403) and `tungsten-java/src/orchestration.rs`
  (~line 205) into a public `tungsten_interface::naming::entry_method_names`. The two backends seed
  the `used` set differently (Rust: `{"new"}`, Java: `rules.used_set()`), so the function needs a
  seed parameter. Generated output must not change; check by diffing output before and after.
- [ ] `runtime/rust/mathf64.rs`: `Vec3` (f64) math, `Pos3`, `Config`, `iter_3d`, `index`,
  `make_boxed`, samplers. Check what generated code actually imports
  (`use crate::mathf64::*; use crate::utilsf64::*;`).
- [ ] Add the table constructors the generated code calls (`make_permutation_table`,
  `make_base3d_perm_table`) to `runtime/rust/utilsf64.rs`. Today it only has the pure functions.
- [ ] Records generated per batch from `registry.toml` (Rust `#[repr(C)]` `Copy` struct).
- [ ] `src/harness/`: `protocol.rs` (SPEC §7.4), `process.rs` (watchdog, resume after crash),
  `batch.rs` (batches per corpus dir, content-hash cache under `$CARGO_TARGET_TMPDIR/tw-cache/`,
  per-test fallback), `rust.rs` (driver generator, `rustc --edition 2024 ... --cap-lints allow`).
- [ ] `src/compare.rs` (scoring table, SPEC §6.3), `src/knownfail.rs` (reconciliation, §6.4),
  mismatch report (§6.5).
- [ ] `<path>::rust::original` and `<path>::rust::inlined` trials in `tests/corpus.rs`. Skip cases
  where the interpreter gives `OutOfFuel` (`oracle::skipped_for_backends` exists). Toolchain probes
  and ignore-with-reason, `TW_REQUIRE`.
- [ ] Mark real gaps with `@knownfail`, one concern per file, and log them in `PAPER.md`.
- [ ] Confirm that `HostConsts` and `Orchestration` members are visible to a sibling `driver` module
  (SPEC §13). If not, a small backend change is needed.
- [ ] `mathf64.rs` was left out of milestone 1 and is not in the SPEC §14 notes as "done".

## Validation still owed (SPEC §12)

- [ ] Mutation self-tests under `selftest::` in `tests/corpus.rs` (wrong value, sign of zero, NaN
  payloads must pass, crash, hang, truncated response, build error in one test of a batch, stale
  knownfail, resume after crash).
- [ ] Protocol round-trip proptest and golden bytes shared by Rust and Java.
- [ ] Toy-table golden values for fixed seeds, in every language. Milestone 1 only has a splitmix64
  golden and property tests.
- [ ] Cache checks: a second run does no builds, and changing a runtime file invalidates the key.

## Later milestones

- [ ] **3. Java differential and CI.** Java runtime and driver, `.github/workflows/test.yml` (JDK 21,
  `TW_BACKENDS=rust,java TW_REQUIRE=rust,java INSTA_UPDATE=no`), the TESTING.md §8.2 example end to end.
- [ ] **4. L6 random programs.** `src/gen.rs`, `tests/random.rs`, batch sampling, manual shrinking,
  `L6_STRICT`, regressions. `proptest` is already a dev-dependency.
- [ ] **5. CUDA.** `runtime/cuda/tw/`, exact flags, host driver, GPU serialization. Interpreter
  errors are not scored on CUDA.

## Known gaps in milestone 1

- [ ] Nothing in the corpus uses the `Wrap` record (record inside a record) or any `@cells` /
  `@tier(large)` directive. Add files for them, and for array-typed entries and multiple entries
  per program.
- [ ] No corpus file has `@expect reject(Kind)` with a kind. Only plain `@expect reject` is used.
- [ ] The test error for entries returning `PermTable` or `Sampler` (SPEC §3.3) is not implemented.
  `values::has_table` exists but is unused. It belongs with the backend trials.
- [ ] Expected values in the corpus were written by hand. A few were corrected after the
  interpreter disagreed (casts, shifts, i8). Worth a second look at `casts/` and `ops/i8.tw`,
  to be sure the interpreter is right and not the expectation.
- [ ] `selection.rs` only validates names. Toolchain probes are not implemented.

## Open questions from SPEC §13

- Does every density input of an entry receive the entry's `origin` in tungsten-mc (TESTING.md §13)?
- Compile time of a whole Rust or Java batch is unmeasured. Split a corpus directory if it gets slow.
- Inlining has no `@knownfail` escape hatch by design. If a long-lived inliner bug appears, consider
  a `stage=inlined` skip.
