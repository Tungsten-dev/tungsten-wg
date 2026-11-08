# PAPER.md: notes toward a paper on the testing and verification infrastructure

Status: working notes. The SPMT IR and the Minecraft lowering are covered by a paper that is already
submitted. This file is about the _new_ work: the backends, the core IR / checker / interpreter, and
the differential testing plan. It is the running log for a second paper.

Claude sessions: see [Maintaining this file](#maintaining-this-file) at the bottom.

## State of the work (as of 2026-11-01)

- `TESTING.md` is a partly implemented spec. `tungsten-parse` (parser, elaborator, canonical
  printer, `.tw` test-file envelope) is implemented, and its tests pass (round-trip, floats, depth,
  fuzz, permute). `tungsten-testsuite` (harness, corpus, test runtimes) exists: milestones 1, 2, 3
  and 5 are done, so the Rust, Java and CUDA backends are compared bit for bit against the interpreter
  (original and inlined). The random generator is not built yet.
- `Expression::Ambient` is implemented in the Rust, Java and CUDA backends, and the old legacy conversion
  emitted it instead of the four `Named` variables (`tungsten-legacy` is deleted in v0.3). `derive_binop_type` has not been changed to the
  §5.3 typing.
- `tungsten-core` has a checker and an interpreter. There are **no functional proofs yet**.
  `scripts/verify.sh` proves bounds, absence of overflow and termination only. The spec/refinement
  proof, type soundness of the checker, fuel monotonicity and the layout lemma are all "later".
- Without those proofs, "verified interpreter as ground truth" is aspirational, not a result.
- Backends exist: Rust (reference), Java, CUDA, plus an inspection backend.

## Candidate contributions

1. **Differential testing of a multi-backend compiler against a verified reference.**
   - Rust, Java and CUDA backends are compared bit-exactly with one interpreter. Only exception: any
     NaN equals any NaN.
   - Design choices that make bit-exact agreement possible:
     - Java integer/float edge semantics are normative for every backend (wrapping arithmetic,
       masked shifts, saturating float-to-int casts with NaN to 0).
     - Transcendentals are IR prelude functions, not primitives, so every backend compiles the same
       `+ - * /`.
     - CUDA is built with strict IEEE flags.
     - Float comparison is bit-exact, including `-0.0` vs `0.0`. min/max follow Java `Math.min/max`.
   - Most publishable part: cross-backend float/integer agreement on GPU code is a known pain point.
2. **Two-IR architecture for verification.**
   - A rich IR (`OptimizedProgram`) for compiling, and a plain core IR (dense ids, resolved types,
     no names, floats as bits) for proving. An unverified, tested lowering connects them.
   - Everything inside `tungsten-core` is proven, everything outside is tested.
   - The trust boundary is `prim.rs`, the only `external_body` file. `verify.sh` refuses
     `admit`/`assume`/`external_body`/`external` elsewhere.
   - Good experience-report material for Verus users.
3. **Termination and evaluation model.**
   - Every function, const, input and unit has a rank (depth in the dependency DAG). Calls and reads
     must decrease rank. Termination measure is `(rank, layer, structure)`.
   - Fuel is per `While` execution. `Repeat` is free. Recursion and cyclic consts/inputs are rejected.
   - Evaluation is eager with a canonical error choice (lowest const, lowest input, first cell, left
     to right), so backends can schedule freely and still agree on which error is reported.
4. **Bugs found.** Concrete bugs in backends, optimizer or interface found by the suite make the
   paper convincing. Known so far: the scale-key fixed-point collision (planned `@knownfail`).
   Log every new one below.

Human: Additional one: intrinsic float to integer conversion cases. floor_int.

## Venue options

| Framing                                            | Needs                                                      | Risk  |
| -------------------------------------------------- | ---------------------------------------------------------- | ----- |
| Verification (CPP, ITP, VSTTE)                     | Refinement and type-soundness proofs actually done         | High  |
| Testing / systems (ISSTA, ASE, CGO, PLDI-adjacent) | Working differential harness plus a bug-finding evaluation | Lower |
| Short tool / experience paper                      | Current state plus a few results                           | Thin  |

## To do before writing

1. Implement `tungsten-testsuite` and run Rust, Java and CUDA differentially.
2. Random well-typed program generation, to get bug counts.
3. Mutation testing (e.g. `cargo-mutants`, or deliberately broken backends) to show the suite catches
   real faults.
4. Prove checker type soundness ("check OK implies never stuck"), ideally also interpreter-to-spec
   refinement.
5. Decide the framing relative to the submitted paper: cite it for SPMT and the Minecraft lowering,
   and keep this paper about correctness infrastructure.

## Evaluation ideas

- Bugs found per backend, by category (arithmetic edge case, scheduling, naming, optimizer).
- Bugs caught by hand-written corpus vs random programs vs optimizer-equivalence tests.
- Mutation score of the suite.
- Proof effort: lines of spec / proof / exec, verification time.
- Cost of the verified boundary: size of the trusted `prim.rs` and of the unverified lowering.

## Findings log

Append entries here, newest last. Format:
`- YYYY-MM-DD [kind] short description. Evidence: file, test, commit.`
Kinds: `bug` (backend/optimizer/interface bug), `design` (decision with a rationale worth citing),
`proof` (proof difficulty or trick), `eval` (a number usable in the evaluation), `limitation`.

- 2026-11-01 [design] Verus cannot compare `Stmt` and `Expr` heights in one recursion group, so the
  termination measure uses an explicit layer ordering (expr 0 < stmt 1 < call/const 2 < cell 3 <
  unit 4 < input 5). Evidence: `crates/tungsten-core/README.md`, `VERUS.md`.
- 2026-11-01 [limitation] Outside `tungsten-check` and `tungsten-core`, test coverage is thin; real
  validation is currently downstream (compiling and running generated code in `tungsten-mc`).
- 2026-11-01 [design] Idea (not implemented, unvalidated): bit-exact agreement (contribution 1) is
  stricter than terrain generation needs, and a pointwise bound |f-g| <= eps is the wrong relaxation
  because `select`/`floor`/comparisons make rounding errors jump. Candidate approximate-equivalence
  criterion: (delta, eps)-closeness, i.e. every output g(x) is within eps of f(x') for some x' within
  delta of x (Hausdorff distance between graphs; discontinuities may shift by delta). Complements:
  (a) predicate agreement outside an eps-band around the thresholds consumers use (sign of density,
  biome thresholds); (b) mismatch rate and locality (mismatches only in a thin shell around the
  isosurface); (c) distribution distance (KS/Wasserstein) to catch bias such as a dropped octave;
  (d) gradient/spectral closeness. eps/delta could be derived by forward error analysis over the core
  IR (value error through continuous ops, position shift through discontinuous ones). Sampling gives
  evidence only; the analytic bound would be the guarantee. Evidence: none yet, discussion only.
- 2026-11-01 [bug] Backends initialized any local named `origin`, `pos3`, `origin_scale` or
  `position_scale` by matching the name, while the checker and interpreter treat it as an ordinary
  uninitialized local. A legacy program reading them could not pass the flow check. Fixed by
  lowering to `Ambient` and removing the name binding. Evidence: `default_variables_are_ambients`
  in `crates/tungsten-check/src/tests/legacy.rs` (deleted with the legacy crate).
- 2026-11-01 [bug] `aquifer_random` in `tungsten-mc-compile` creates `origin` as `Pos3` (it is a
  `Vec3`) and only worked because undeclared variables resolved by name. Conversion now resolves it
  to the ambient, types the field from the ambient and records a warning, but `convert` discards
  warnings. Evidence: `mistyped_origin_passes_the_checker`, `aquifer.rs:673` in that repo.
- 2026-11-01 [design] Ambients are read-only in the IR, but `water_levels` in `tungsten-mc-compile`
  reassigns `pos3` mid-body and reads it through a second variable of that name. A grep of the
  frontend missed this; only running real generation found it. Conversion gives such a function a
  local initialized from the ambient. Evidence: `assigned_pos3_starts_as_the_ambient`.
- 2026-11-01 [eval] Vanilla overworld Rust output before and after the ambient change: 221 functions,
  0 differing lines once ambient reads, bindings and `inlined_density_N` numbering are normalized
  (one-off script, not in the repo). Rust only; CUDA and Java output were generated but not diffed.
- 2026-11-01 [eval] First Rust differential (testsuite milestone 2): 59 non-reject corpus files, each
  run on the original and the inlined program. 19 files carry a `@knownfail(rust, ...)`, all for the
  four bugs below. Of the other 40, 38 agree with the interpreter bit for bit on both stages; the 2 left
  send nothing to the backend (`errors/out_of_fuel`, interpreter-only by design) or are `@tier(large)` and
  skipped unless `TW_LARGE=1`. Cold run of
  the whole suite (including the Rust builds) takes about 4.5 s, a warm one about 1.5 s. Evidence:
  `cargo test -p tungsten-testsuite --test corpus`, `grep -rl @knownfail crates/tungsten-testsuite/corpus`.
- 2026-11-01 [bug] The Rust backend emits plain `+ - * / <<` for integers. With overflow checks on, `i32`,
  `i64` and `i8` add, subtract, multiply and `MIN / -1` panic where the IR (and Java) wrap, and a shift
  by the type width or more panics where the amount should be masked. Fifteen files, one per type and
  operation. Evidence: `corpus/ops/{i8,i32,i64}_{add_wraps,sub_wraps,mul_wraps,div_min_by_minus_one,shift_masked}.tw`.
  Unverified: whether a release build without overflow checks would hide it for add/sub/mul (it would
  still not mask shifts or wrap `MIN / -1`).
- 2026-11-01 [bug] The Rust backend emits `>>>` (logical shift right) as `>>`, which is an arithmetic shift
  on signed types: `-8 >>> 28` gives `-1` instead of `15`. Evidence: `corpus/ops/i32_logical_shift.tw`,
  `corpus/ops/i64_logical_shift.tw`; `convert_binary_op` in `tungsten-rust/src/types.rs` maps both
  `ShiftRight` and `LogicalShiftRight` to `RightShift`.
- 2026-11-01 [bug] The Rust backend casts a float to `i8` with `as i8`, which saturates (`300.0 as i8` is 127),
  while the IR goes through `i32` and truncates like Java (44). Evidence: `corpus/casts/to_i8_from_float.tw`.
- 2026-11-01 [bug] The Rust backend emits constants as `const` items, so a failing constant
  (`const bad = 1 / 0`) is a build error (E0080) instead of a runtime failure of the case. The "unread
  failing const must still fail the case" contract is therefore untestable on Rust until this is fixed.
  Evidence: `corpus/errors/unread_failing_const.tw`.
- 2026-11-01 [design] The harness scores a build failure as a failure even for cases where the interpreter
  errors, so a genuine compile bug cannot hide in an error-only test; the cost is that a failing `const`
  needs a `@knownfail` (previous entry). A `@knownfail` also covers every case in its scope, so files are
  kept to one concern each. Evidence: `SPEC.md` §6.3, §6.4.
- 2026-11-01 [limitation] The verified runtime in the sister project (`tungsten-frontends/tungsten-mc`,
  `src/utilsf64.rs`) implements `make_boxed` with `Box::new_uninit().assume_init()`, so `InternalBuffers`
  and `AutoConsts` start with uninitialised contents (undefined behaviour for `bool` cells, and
  nondeterministic otherwise). The test runtime zero-initialises instead
  (`crates/tungsten-testsuite/runtime/rust/api.rs`). Unverified: whether any generated program reads a
  buffer cell before writing it.
- 2026-11-01 [design] Host values are bound by name when a case is sent to a backend, so the same case works
  on the inlined program, where host arena order can differ. Evidence: `TestIo::encode_request` in
  `crates/tungsten-testsuite/src/harness/protocol.rs`.
- 2026-11-01 [eval] First Java differential (testsuite milestone 3): the same corpus on the original and
  the inlined program. 8 files carry a `@knownfail(java, ...)`, for the two bugs below; every other
  file agrees with the interpreter bit for bit on both stages. Unlike Rust, integer arithmetic, shifts and
  failing constants all match. Cold Java run of the corpus (javac batches plus JVM start-ups): about 25 s
  on this machine. Mutation check: swapping the arguments of `copySign` in the Java runtime turns the
  `externs/math_*` trials red.
- 2026-11-01 [bug] The Java backend panics ("cannot derive the element type of an array literal") when an
  array literal starts with a shift expression: `derive_binop_type` (shared, in `tungsten-rust/src/types.rs`)
  returns `None` for shift and bitwise operators. Evidence: `corpus/ops/{i8,i64,i32_shift_masked,...}.tw`
  (7 files, every one a shift in a one-element array).
- 2026-11-01 [bug] The Java backend clamps float to `i8` casts to -128..127 (`Math.min`/`Math.max` around the
  `int` cast) as if it were Rust's saturating `as i8`, but the IR goes through `i32` and truncates (44 for
  `300.0`). The comment in `expression.rs` that justifies it is wrong. Evidence: `corpus/casts/to_i8_from_float.tw`.
- 2026-11-01 [bug] The CUDA backend casts a float to `i8` through `tw::f2i8`, which its README defines as
  clamping to -128..127 ("where Java `(byte)` would wrap"), but the IR goes through `i32` and truncates (44
  for `300.0`). Same defect as Rust and Java. Evidence: `corpus/casts/to_i8_from_float.tw`.
- 2026-11-01 [design] First CUDA differential run (GTX 1650 Ti, nvcc 13.4, exact profile): 58 of 59 corpus
  files agree bit for bit on both stages with no backend change; integer wrapping, masked shifts and `>>>`
  that Rust gets wrong are right in CUDA. Cases where the interpreter errors are not run on CUDA.

## Maintaining this file

Instructions for future Claude sessions working in this repo:

- Whenever you come across something that could matter for this paper, add an entry to the
  **Findings log** in the same session, without being asked. That includes:
  - a bug in a backend, the optimizer, the interface or the lowering, especially one found by a test,
    the checker or the interpreter, and a backend disagreeing with the interpreter;
  - a design decision with a non-obvious rationale, or a semantic edge case that had to be pinned
    down for backends to agree;
  - a proof that was hard, a Verus limitation or workaround, or a rule that `verify.sh` enforces;
  - numbers usable in the evaluation (test counts, bugs per backend, proof size, verification time,
    mutation score, performance);
  - limitations or negative results.
- Keep entries short, dated, tagged with a kind, and pointing at evidence (file, test or commit).
- Update **State of the work** and **To do before writing** when something is implemented or
  decided, so they don't go stale.
- Do not invent results. Record only what was observed, and say when a claim is unverified.
- Don't duplicate content that already lives in the READMEs. Link to it instead.
