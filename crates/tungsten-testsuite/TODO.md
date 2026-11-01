# TODO

State at the end of the second session: milestones 1 (Foundation) and 2 (Rust differential) are done
and green. See `SPEC.md` at the repo root (§9.4 for the milestones, §14 for implementation notes) and
`README.md` here. Nothing from milestone 2 is committed yet.

## Before starting

- Commit milestone 2. It touches `tungsten-interface/naming.rs`, `tungsten-rust` and `tungsten-java`
  (the `entry_method_names` extraction), so consider splitting that into its own commit.
- Run `INSTA_UPDATE=no cargo test --workspace` to confirm the baseline is still green.

## Validation still owed (SPEC §12)

- [ ] Java halves: golden bytes (`tests/golden/protocol.txt`) and toy-table goldens in Java (milestone 3).
- [ ] Diff the generated Java output before and after the `entry_method_names` extraction. Only Rust was
  diffed; the Java change is the same loop and its tests pass.
- [ ] `runtime_coverage` (SPEC §5.2): one call to every registered extern per backend. The Rust runtime
  is covered by the per-extern corpus files; the Java one comes with milestone 3.
- [ ] Optional: a mutation check on the real generated path (edit `runtime/rust/externs.rs`, see the
  corpus go red). The selftests use stub drivers, so they don't cover this.

## Later milestones

- [ ] **3. Java differential and CI.** Java runtime and driver, `.github/workflows/test.yml` (JDK 21,
  `TW_BACKENDS=rust,java TW_REQUIRE=rust,java INSTA_UPDATE=no`), the TESTING.md §8.2 example end to end.
  Reuse `harness/protocol.rs` (the layout is language independent) and the `process.rs` watchdog; the
  JVM start-up allowance is `STARTUP_ALLOWANCE`.
- [ ] **4. L6 random programs.** `src/gen.rs`, `tests/random.rs`, batch sampling, manual shrinking,
  `L6_STRICT`, regressions. `proptest` is already a dev-dependency.
- [ ] **5. CUDA.** `runtime/cuda/tw/`, exact flags, host driver, GPU serialization. Interpreter
  errors are not scored on CUDA.

## Backend gaps found (all `@knownfail`, logged in `PAPER.md`)

- [ ] Rust integer arithmetic does not wrap (`ops/{i8,i32,i64}_{add,sub,mul}_wraps`,
  `_div_min_by_minus_one`, `_shift_masked`). Fix the backend, then delete the `@knownfail` lines (the
  trials go red until you do).
- [ ] Rust emits `>>>` as `>>` (`ops/{i32,i64}_logical_shift`).
- [ ] Rust float to `i8` casts saturate (`casts/to_i8_from_float`).
- [ ] Rust `const` items make a failing constant a build error (`errors/unread_failing_const`). Once it
  builds, check that the "unread" part of the contract holds too.

## Known gaps in the corpus

- [ ] Expected values in the corpus were written by hand. A few were corrected after the interpreter
  disagreed (casts, shifts, i8). Worth a second look at `casts/` and `ops/i8*.tw`, to be sure the
  interpreter is right and not the expectation. (The Rust backend now disagrees with them in the way
  Rust semantics predict, which is some evidence the expectations follow Java.)
- [ ] There is no corpus file for `@cells` that fails (a sampled-out cell that differs is meant to be
  ignored). The selection function is unit tested.
- [ ] `selection.rs` only knows `rust` for toolchain probes; `javac`, `java` and `nvcc` come later.

## Open questions from SPEC §13

- Does every density input of an entry receive the entry's `origin` in tungsten-mc (TESTING.md §13)?
- Compile time of a whole Rust batch is about 1 to 2 seconds per directory with the current corpus;
  Java is unmeasured. Split a corpus directory if it gets slow.
- Inlining has no `@knownfail` escape hatch by design. If a long-lived inliner bug appears, consider
  a `stage=inlined` skip.
- A `@knownfail` without `case=` fails when one of the cases it covers passes (SPEC §6.4 read literally).
  If that proves too strict for multi-case files, relax it to "stale only if none of the covered cases fail".
