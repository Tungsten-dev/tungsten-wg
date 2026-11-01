# TODO

State at the end of the third session: milestones 1 (Foundation), 2 (Rust differential) and 3 (Java
differential and CI) are done and green. See `SPEC.md` at the repo root (§9.4 for the milestones, §14 for
implementation notes) and `README.md` here. Milestone 3 is not committed yet.

Random program testing (L6, milestone 4) is out of scope for now. CUDA (milestone 5) is next.

## Before starting

- Commit milestone 3.
- Run `TW_BACKENDS=rust,java TW_REQUIRE=rust,java INSTA_UPDATE=no cargo test --workspace` to confirm the
  baseline is still green. => Done, is green
- `nvcc` (`/usr/local/cuda-13/bin/nvcc`) and `nvidia-smi` are installed on this machine. Check that
  `nvidia-smi -L` lists a GPU before relying on a local CUDA run.

## Next: milestone 5, CUDA

Follows `SPEC.md` §9.4 item 5 and the CUDA lines of §3.2, §5.2, §7.1 to §7.3 and §9.3. It must fit the
existing protocol and `Lang` split without redesign.

- [ ] `Lang::Cuda` in `harness/batch.rs`: cache under `tw-cache/cuda/`, `Harness::new_cuda` with the probe
      (`nvcc --version` and `nvidia-smi -L` with at least one GPU), `$NVCC` override like `$JAVAC`.
- [ ] `tungsten-cuda` as a dependency of the testsuite (it is already a workspace member).
- [ ] CUDA symbol names through `entry_method_names` (SPEC §2: "CUDA's symbol names get the same
      treatment in milestone 5"). Generated CUDA output must not change; diff it before and after, as was
      done for Java.
- [ ] `runtime/cuda/tw/`: the types and helpers the generated code expects (`Vec3`, `Pos3`, the
      samplers, toy tables ported from `runtime/rust/utilsf64.rs`), the registry externs, and a C++ protocol
      reader and writer. Records become CUDA aggregates generated from `registry.toml`.
- [ ] `harness/cuda.rs`: `records_source`, host driver (`main` that serves requests), `generate_test`.
      Settle the host-driver layout here (SPEC §13).
- [ ] Build with `nvcc --fmad=false --prec-div=true --prec-sqrt=true --ftz=false`
      (`Orchestration::required_flags`), no `--use_fast_math`. The flags are part of the cache key.
- [ ] Error-case skipping: cases whose interpreter result is an error are not run or scored on CUDA
      (SPEC §6.3). `oracle::skipped_for_backends` skips `OutOfFuel` only, so this needs a per-backend
      predicate.
- [ ] GPU serialization: a trap error is sticky, so any failure ends the process and the harness resumes
      in a new one (already how `process.rs` works); GPU jobs run under one global lock (SPEC §13: pick the
      job limit).
- [ ] Trials `<file>::cuda::original|inlined` in `tests/corpus.rs`, ignored unless `cuda` is selected
      (default adds it when `nvcc` and a GPU are detected). `TW_REQUIRE=cuda` fails when missing.
- [ ] Validation (SPEC §12): CUDA half of the golden bytes, toy-table goldens, `runtime_coverage`,
      stub-driver selftests (crash, hang, truncated response, build error in one test of a batch), a mutation
      check on the real path.
- [ ] Run the corpus, mark gaps as `@knownfail(cuda, ...)` and log each in `PAPER.md`.
- [ ] Docs: milestone 5 notes in `SPEC.md` §14, README. CI stays Rust + Java; CUDA and Verus run locally.

## Out of scope for now

- [ ] **4. L6 random programs** (`src/gen.rs`, `tests/random.rs`, batch sampling, manual shrinking,
      `L6_STRICT`, regressions). The design stays in `SPEC.md` §8 and `TESTING.md` §10 for later;
      `proptest` is already a dev-dependency. `corpus/regressions/` stays empty until then.

## Validation (SPEC §12)

- [x] Java halves: golden bytes and toy-table goldens in Java (`selftest::java::*`).
- [x] Generated Java before and after the `entry_method_names` extraction: 118 outputs (59 files, both
      stages), byte-identical.
- [x] `runtime_coverage` (SPEC §5.2): Rust is `runtime_externs_registered`, Java is
      `selftest::java::runtime_coverage`.
- [x] Mutation check on the real generated path, done for Java (`copySign` arguments swapped,
      `externs/math_*` went red).
- [ ] The same mutation check for Rust (edit `runtime/rust/externs.rs`, see the corpus go red).
- [ ] The TESTING.md §8.2 example end to end on Rust and Java (SPEC §12). Not checked separately from
      the corpus.

## Backend gaps found (all `@knownfail`, logged in `PAPER.md`)

Rust:

- [ ] Integer arithmetic does not wrap (`ops/{i8,i32,i64}_{add,sub,mul}_wraps`, `_div_min_by_minus_one`,
      `_shift_masked`). Fix the backend, then delete the `@knownfail` lines (the trials go red until you do).
- [ ] `>>>` is emitted as `>>` (`ops/{i32,i64}_logical_shift`).
- [ ] Float to `i8` casts saturate (`casts/to_i8_from_float`).
- [ ] `const` items make a failing constant a build error (`errors/unread_failing_const`). Once it
      builds, check that the "unread" part of the contract holds too.

Java:

- [ ] The backend panics on an array literal that starts with a shift (`derive_binop_type` has no type for
      shift and bitwise operators): 7 files in `ops/` (`i8`, `i8_shift_masked`, `i32_shift_masked`,
      `i32_logical_shift`, `i64`, `i64_shift_masked`, `i64_logical_shift`).
- [ ] Float to `i8` casts clamp to -128..127 instead of truncating through `i32`
      (`casts/to_i8_from_float`).

## Known gaps in the corpus

- [ ] Expected values in the corpus were written by hand. A few were corrected after the interpreter
      disagreed (casts, shifts, i8). Worth a second look at `casts/` and `ops/i8*.tw`, to be sure the
      interpreter is right and not the expectation. (The Rust backend now disagrees with them in the way
      Rust semantics predict, which is some evidence the expectations follow Java.)
- [ ] There is no corpus file for `@cells` that fails (a sampled-out cell that differs is meant to be
      ignored). The selection function is unit tested.
- [ ] Toolchain probes live in `harness/batch.rs` (`probe_rustc`, `probe_java`); the `nvcc` probe comes with
      milestone 5. `selection.rs` only validates names.

## Open questions from SPEC §13

- Does every density input of an entry receive the entry's `origin` in tungsten-mc (TESTING.md §13)?
- Compile time: a whole Rust batch is about 1 to 2 seconds per directory with the current corpus; Java
  takes about 25 s cold for the whole corpus (javac batches and JVM start-ups). Split a corpus directory if
  it gets slow.
- Inlining has no `@knownfail` escape hatch by design. If a long-lived inliner bug appears, consider
  a `stage=inlined` skip.
- A `@knownfail` without `case=` fails when one of the cases it covers passes (SPEC §6.4 read literally).
  If that proves too strict for multi-case files, relax it to "stale only if none of the covered cases fail".
