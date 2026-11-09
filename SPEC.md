# `tungsten-testsuite`: differential test harness, corpus and runtimes

Status: approved design; milestones 1, 2, 3 and 5 are implemented (§14). This implements `TESTING.md` §9–§12 and the
`tungsten-testsuite` entry in §3. `TESTING.md` remains the normative source for semantics (§5, §6)
and the text format (§8, plus `crates/tungsten-parse/SPEC.md`). The amendments in §10 below have
already been applied to it.

## 1. Summary

A new unpublished crate compiles small `.tw` programs with each backend, runs the result out of
process on the file's cases, and compares every value bit for bit with the `tungsten-core`
interpreter (any NaN equals any NaN). It also tests the interpreter against hand-written
expectations, the text round-trip, the lowering and the inliner.

### Goals

- Rust and Java differential testing in `cargo test --workspace` and CI, with CUDA as a later
  milestone that needs no redesign.
- One failing or uncompilable test never hides the result of another.
- The suite lands green while backends are still non-conformant: every known gap is a concrete,
  self-expiring `@knownfail`, which doubles as the bug log for `PAPER.md`.
- Editing one corpus file gives fast feedback.

### Non-goals

- Fixing backend conformance (`TESTING.md` §4.3–§4.5). That is follow-up work.
- Interface property tests, other than the scale-key regression.
- Verus proofs, or any change to `tungsten-core`.
- Real noise correctness, which stays with tungsten-mc.
- Decision traces (§9.1 here) in the first version.
- Random program testing (L6, §8). Out of scope for now: no generator, no `tests/random.rs`, no
  `corpus/regressions/` writer. §8 stays as the design for when it is picked up again.

## 2. Crate layout

```
crates/tungsten-testsuite/        # publish = false
  Cargo.toml
  registry.toml                   # externs and records (§5)
  prelude/prelude.tw              # sin, cos, exp, log, pow, lerp, smoothstep, as_index, corner*
  corpus/<dir>/**/*.tw            # hand-written; each top-level dir is one batch
  corpus/regressions/             # shrunk L6 failures (deferred, §8)
  runtime/rust/{mathf64,utilsf64,externs}.rs
  runtime/java/tungsten/runtime/*.java
  runtime/cuda/tw/*               # milestone 5
  src/
    lib.rs
    env.rs                        # TestEnv: tungsten_parse::Env + tungsten_check::Registry
    registry.rs                   # registry.toml loader
    externs.rs                    # tungsten_check::Externs adapter over runtime/rust
    values.rs                     # tungsten_parse::Value <-> core Value <-> protocol bytes
    corpus.rs                     # discovery and per-file loaded state
    oracle.rs                     # interpreter runs, @expect matching
    compare.rs                    # scoring (§6.3) and reports
    knownfail.rs                  # reconciliation (§6.4)
    selection.rs                  # TW_BACKENDS, TW_REQUIRE, toolchain probes
    gen.rs                        # L6 generator (§8, deferred)
    harness/
      batch.rs                    # batching, content-hash cache, fallback (§7.2)
      protocol.rs                 # wire format (§7.4)
      process.rs                  # child lifecycle, watchdog, resume (§7.3)
      rust.rs  java.rs  cuda.rs   # driver generator and build command per backend
  tests/
    roundtrip.rs   lowering.rs   corpus.rs   random.rs   # all harness = false; random.rs is deferred (§8)
    snapshots/                    # insta
.github/workflows/test.yml        # new: the repo has no CI yet
```

Dependencies: `tungsten-parse`, `tungsten-check`, `tungsten-ir`, `tungsten-interface`,
`tungsten-optimizer`, `tungsten-rust`, `tungsten-java`, `tungsten-cuda`, `serde`, `toml`,
`proptest`, `libtest-mimic`, `insta`. New workspace dependencies: `toml`, `libtest-mimic`, `insta`.

### Required change outside the crate

The driver must call `orchestrate_<entry>`. Rust and Java each compute that name inline
(`tungsten-rust/src/orchestration.rs:407`, `tungsten-java/src/orchestration.rs:208`), via `unique`
over `unit_base_name`. Extract it to a public
`tungsten_interface::naming::entry_method_names(program, rules) -> Vec<String>` used by both
backends and the driver generators. Generated output must not change. CUDA's symbol names get the
same treatment in milestone 5. No other production code changes are in scope.

## 3. Test model

### 3.1 Trials

`libtest-mimic` makes each check a separate trial, so `cargo test` names and filters work:

| Trial name | Level | What it does |
| --- | --- | --- |
| `<path>::load` | | Only if `load` fails or a `@knownfail` directive is invalid; the file gets no other trials |
| `<path>::roundtrip` | L1 | `parse(print(m)) ≅ compact(m)` and `print` fixpoint (§8.1 of `TESTING.md`) |
| `<path>::lowering` | L2 | Invariants and insta snapshot (§3.4) |
| `<path>::interp` | L3 | `prepare`, run every case, match `@expect` or `@expect reject`, memo on == off |
| `<path>::inline` | L5 | `perform_inline`, `prepare` again, interpreter result equals `::interp` |
| `<path>::<backend>::original` | L4 | Backend vs interpreter |
| `<path>::<backend>::inlined` | L5 | Backend on the inlined program vs interpreter |

`<path>` is relative to `corpus/`, without the extension. Files are discovered at runtime, so adding
one needs no code change. A reject file has only `roundtrip`, `lowering` (when lowering succeeds)
and `interp`.

A trial is **ignored with a reason** if the backend is unselected or its toolchain is missing, or if
the file is `@tier(large)` without `TW_LARGE=1`.

### 3.2 Environment variables

| Variable | Meaning | Default |
| --- | --- | --- |
| `TW_BACKENDS` | Comma list of backends to run | `rust,java`, plus `cuda` when `nvcc` and a GPU are detected and the backend's milestone has landed |
| `TW_REQUIRE` | A listed backend with a missing toolchain fails the run instead of being ignored | empty |
| `TW_LARGE` | `1` runs `@tier(large)` | off |
| `TW_FUZZ` | (deferred with L6) L6 program count; also enables shrinking and writing for non-strict backends | unset |
| `TW_SEED` | (deferred with L6) L6 base seed | fixed constant; with `TW_FUZZ`, random and printed |
| `TW_SHRINK_SECS` | Time budget per shrink | 120 |
| `TW_SHRINK_COUNT` | Maximum failures shrunk per backend per run | 5 |
| `TW_TIMEOUT_SECS` | Watchdog per case | 10 (×10 for `@tier(large)`) |
| `CI` | (deferred with L6) L6 failures go to the log and an artifact, not the source tree | set by Actions |

An unknown name in `TW_BACKENDS` or `TW_REQUIRE` fails the run.

Toolchain probes run once: `rustc --version`, `javac -version` and `java -version` (must be 21 or
newer), `nvcc --version` plus `nvidia-smi -L` with at least one GPU.

### 3.3 Interpreter stage

- `load` uses `TestEnv` and reads `prelude.tw` through `Env::library`.
- Hosts are bound by name (`NamedCase`), so inlining can't invalidate them. Fuel comes from `@fuel`,
  else `DEFAULT_FUEL`.
- A rejected program is a failure, unless the file says `@expect reject`.
- `EvalFailure::Stuck` is always a failure (interpreter or checker bug).
- `@knownfail` never applies to interpreter trials. An inliner bug keeps `::inline` red until fixed.

`@expect` semantics, on the original stage:

- `Cell`, `All`, `Grid`: the named cells must be `Ok` and match, floats by bits, with any NaN
  matching any NaN. Cells that aren't named are not checked.
- `error(Kind)`: the case must have at least one failing cell, and the first failing cell (lowest
  entry index, then flat order) must have that kind. Valid kinds are `DivByZero`, `OutOfBounds`,
  `OutOfFuel` and `ExternDomain`; anything else is a test error. `Stuck` can't be expected.
- `reject(Kind)`: some lowering or checker error has that enum variant name. Without a `Kind`, any
  rejection matches.
- An entry whose return type holds `PermTable` or `Sampler` can't be encoded, so it is a test error
  on every backend trial.

### 3.4 L2 lowering checks

Over every non-reject, non-regression corpus file, original stage:

- **Invariants**: types and arity are preserved, every ref is resolved, and `lower` is deterministic.
- **Snapshot**: `insta` snapshot of `{:#?}` of the lowered core `Program`, named after the corpus
  path, in `tests/snapshots/`. CI sets `INSTA_UPDATE=no`.

Regressions are excluded because their contents are generated and would only add churn.

## 4. Semantics of outcomes

### 4.1 `OutOfFuel` is interpreter-only

Backends have no fuel: a `While` running `fuel + 1` iterations correctly returns a value on a
backend, and a truly infinite one would only burn a watchdog.

- `OutOfFuel` in any cell of a case fails `::interp`, unless the file says
  `@expect "<case>" error(OutOfFuel)`.
- Either way, that case is **never sent to a backend**, on either stage.
- A watchdog timeout on any other case is an ordinary backend failure.

### 4.2 Eager failures are strict

`TESTING.md` §2 and §9.4 stand as written: a failing const or input fails the whole case, read or
not, and each backend must fail that case. Backends that only emit used constants return values for
an unread failing const, which is a mismatch tracked as `@knownfail` until they emit all listed
consts. Rust's `const` emission turns the same program into a build failure (§6.3).

## 5. Registry, runtimes and toy tables

### 5.1 `registry.toml`

```toml
[[record]]
name = "Pair"
fields = [{ name = "a", type = "f64" }, { name = "b", type = "i32" }]

[[extern]]
name = "sqrt"
params = ["f64"]
ret = "f64"
domain_error = false
```

Type spellings are those of `tungsten-parse` §5.4. The loader fills `RegistryBuilder` and backs
`Env::extern_fn` and `Env::extern_type`. Names are unique, with no overloads, so `f32` variants take
a `_f32` suffix.

Primitive set: `sqrt`, `floor`, `ceil`, `trunc`, `abs`, `min`, `max` (Java semantics), `copysign`,
`fma`, each with a `_f32` variant; `float_to_bits(f32) -> i32` and `double_to_bits(f64) -> i64`
(canonical NaN); `bits_to_float`, `bits_to_double`; `perm_get`; `sampler_sample`; and
`test_domain(i32) -> i32`, which fails for negative input so `ExternDomain` is reachable.

### 5.2 Implementations

- **One shared Rust source.** `runtime/rust/` holds the extern functions. `src/externs.rs` includes
  them (`#[path]`) behind the `Externs` adapter for the interpreter, and they are copied into
  generated builds. Generated code hardcodes `use crate::mathf64::*; use crate::utilsf64::*;`, so
  these are crate-root modules in the batch crate.
- A shared bug can't appear as interpreter vs Rust. The Java comparison and hand-computed `@expect`
  corpus files for each extern cover that.
- **Records are generated** per batch from `registry.toml`: Rust `#[repr(C)]` `Copy` struct, Java
  class, CUDA aggregate. Extern functions stay hand-written.
- `runtime_coverage` (in `tests/corpus.rs`) compiles one call to every registered extern, per
  backend, so a missing implementation has one obvious failure.
- The test runtime supplies everything the backends assume: `Vec3` (f64), `Pos3`, `Config`,
  `iter_3d`, `index`, `make_boxed`, samplers, and for Java the `*Ref` holders and `Helpers`.

### 5.3 Toy tables

Defined over the arguments generated code already passes:

```
state  = fold splitmix64 over (seed, id_lo, id_hi, subident_index, sub_lo, sub_hi)   # Perlin
state  = splitmix64(seed)                                                            # Base3D
perm   = Fisher-Yates over 0..256, drawing from splitmix64(state)
perm_get(t, i)        = perm[i & 255]                                  # no domain error
sampler_sample(s, v)  = (h >>> 11) as f64 * 2^-53, where
                        h = fold splitmix64 over (state, f2i(floor v.x), f2i(floor v.y), f2i(floor v.z))
```

`seed` is reinterpreted between `u64` and `i64` (bits). The interpreter computes the same inputs from
`LowerMap::resolved_auto_consts` (the toy initializers take the ids as literal `i64` arguments), inside a
per-case `Externs` value, since the core `Case` has no params. Every step is integer or exactly representable, so it's identical
across languages. All Base3D tables of one seed are identical; tests must not rely on them differing.

## 6. Differential comparison

### 6.1 Flow for one backend trial

1. Get the oracle for the file and stage (memoized per file).
2. Get the batch for `(backend, corpus dir, stage)`, building it on first use (§7.2).
3. Start one process for this test, send each eligible case in order, and collect results (§7.3).
4. Score per case (§6.3), reconcile (§6.4), and report.

### 6.2 Value equality

Recursive: integers and `Bool` by value, floats by bits except that any NaN equals any NaN of the
same width. Arrays, records, `Vec3` and `Pos3` go element by element. With `@cells(sample, seed)`, the
harness picks a deterministic subset of cells per entry and compares only those; all cells still run
and are transferred, because the interpreter and generated code have no partial mode.

### 6.3 Scoring

| Situation | Result for the case |
| --- | --- |
| Interpreter `OutOfFuel` anywhere | Not run, not scored |
| Interpreter clean (no cell error, no `failures`), backend returns equal values | Pass |
| Interpreter clean, backend differs, fails, crashes or times out | Fail |
| Interpreter error, backend fails, crashes or times out | Pass; the kind is shown, never compared |
| Interpreter error, backend returns | Fail |
| Interpreter error on CUDA | Not run, not scored (`TESTING.md` §9.3) |
| `generate` panics or errors, or the build fails | **Fail, for every case**, including cases where the interpreter errors |

A build failure is always a failure, even where a "fail" is what the interpreter predicts. That keeps
"compiled means comparable", so a genuine compile bug can't pass unnoticed in an error-only test.
Consequences:

- Rust's `const X: T = 1 / 0` (E0080) is a backend failure, to be fixed in the backend or marked
  `@knownfail`.
- Literal `1 / 0` in a function body must still reach runtime, so builds pass `--cap-lints allow`.
- `generate` runs inside `catch_unwind`, because backends panic on underivable types.

### 6.4 Knownfail reconciliation

For each `(case, stage)` of a backend trial, `@knownfail` with matching `backend` and optional `case`
and `stage` marks it as covered.

- Covered and failing: expected, printed with its reason.
- Covered and passing: **trial fails**, "stale knownfail, remove it".
- Uncovered and failing: trial fails.
- Unknown backend, case or stage names in a directive produce a failing `::load` trial (§3.1).
- Knownfails for a backend that is ignored or not scored for that case are not evaluated.

Since one `@knownfail` hides everything else that fails in the same scope, corpus files should stay
small and focused, one concern per file, with `case=` narrowing where a file has several behaviours.

### 6.5 Mismatch report

Trial output gives the file and case name, backend and stage, entry label, cell `(x, y, z)` and flat
index, both values in hex and decimal, the first differing cell plus a count of the rest, and any
const or input `failures`. For crashes, it gives the exit status and the tail of stderr. For build
failures, it gives the compiler output and the generated-source path in the cache.

## 7. Drivers

### 7.1 Generation

`Interface::build` with the backend's naming rules gives host field names. `entry_method_names` gives
entry methods. A generated `Driver` per test, per language, is type-directed from the program's host
and entry types: it decodes hosts, creates a **fresh `Orchestration` per case** (so stale buffers and
seed state can't leak between cases), calls every entry in entry order, and encodes each result.

Each test lives in its own namespace, `t_<hash>`:

- **Rust**: `pub mod t_<hash> { pub mod density_function; pub mod orchestration; pub mod driver; }`.
  Generated imports use `super::` for sibling modules, so nesting works. The binary's `main` dispatches
  on the test id.
- **Java**: package `t_<hash>` through `JavaConfig`, with runtime package `tungsten.runtime`, run as
  `java -cp <dir> t_<hash>.Driver`.

### 7.2 Batches, cache and fallback

- A batch is `(backend, top-level corpus dir, stage)`. L6 has its own batch per run. A filter that
  selects any test in a batch builds the whole batch, so filtering never changes its hash.
- Location: `$CARGO_TARGET_TMPDIR/tw-cache/<backend>/<hash>/`. The hash (a small FNV-based one in `src/hash.rs`) covers generated sources,
  runtime files, flags and the compiler's version string. Builds go to a temp directory and are
  renamed into place, so concurrent test binaries are safe.
- Each batch sits behind a `OnceLock`, so the first trial that needs it builds it.

Fallback: if a batch build fails, each of its tests is built alone (parallel, separately cached).
Tests that compile alone run from their own artifact, and tests that don't get a build failure for
all their cases. The batch failure itself is not reported, only the attributed ones.

Build commands:

| Backend | Command |
| --- | --- |
| Rust | `rustc` (`$RUSTC` if set) `--edition 2024 -C opt-level=3 -C overflow-checks=on -C panic=unwind --cap-lints allow`. Direct `rustc`, since the crate has no dependencies, which avoids generating a `Cargo.toml` and nested cargo locks. |
| Java | `javac --release 21 -d <dir>` |
| CUDA | `nvcc --fmad=false --prec-div=true --prec-sqrt=true --ftz=false` (`Orchestration::required_flags`) |

### 7.3 Process model

One process per `(test, stage)`. Trials run in parallel under libtest-mimic.

- Cases are **request/response**: the harness writes one case, then waits for its result under the
  watchdog (plus a fixed startup allowance on the first case, for the JVM). This prevents pipe deadlocks.
- A crash or timeout on a case scores that case as failed, kills the process, and **resumes from the
  next case in a new process**, so results for later cases stay accurate (including for `case=`
  knownfails).
- Rust: a panic hook silences the message into the failure text, and each case runs in
  `catch_unwind`.
- Java: each case catches `Throwable`, including `ExceptionInInitializerError` from failing constants
  and the `NoClassDefFoundError` that follows.
- CUDA: trap errors are sticky, so any failure ends the process and resumes. GPU jobs run serially.

### 7.4 Protocol

Little-endian on stdin and stdout, flushed after each message. Driver logs go to stderr only.

```
request  := origin:[u64;3] (f64 bits)  seed:u64  host*        (host arena order)
            EOF ends the process
response := 0x00 value* (entry order, flat cell order)
          | 0x01 kind:u8 len:u32 utf8[len]
kind     := 0 DivByZero | 1 OutOfBounds | 2 ExternDomain | 3 other
```

Value encoding is positional and typed by the program, with no tags: `Bool` and `I8` take 1 byte,
`I32` and `F32` take 4 bytes, `I64` and `F64` take 8 bytes (raw float bits), and `Vec3`, `Pos3`, arrays
and records are their elements in order. A response shorter or longer than the types predict is a
harness error, reported as such rather than as a mismatch. A driver writes a response only after the
whole case succeeds, so partial results can't be observed.

## 8. L6: random well-typed programs

> **Out of scope for now.** Nothing in this section is implemented or scheduled. The next milestone is
> CUDA (§9.4 item 5). The text below is the design to start from later.

### 8.1 Generator

`src/gen.rs` is new, because the `tungsten-parse` test generator doesn't produce well-typed programs.
It reuses that generator's technique of recording a `Vec<u64>` of choices that proptest generates and
shrinks, replayed through a small `Rng`.

- Output is a `TestFile`, so a failure can be printed to `.tw` directly.
- It obeys `TESTING.md` §5.3: all operators and types, the `Vec3`/`Pos3` rule, casts, explicit unit
  lists, an acyclic call graph, hosts, consts, helpers, density inputs with equal and different dims,
  explicit-index reads, externs, records and arrays.
- Grids are at most 4×4×4. `Repeat` counts are at most 8. `While` is always a bounded counter shape,
  so `OutOfFuel` doesn't arise.
- Literals, hosts and origins are biased toward edge values (NaN, ±inf, `-0.0`, MIN/MAX).
- Errors are allowed but biased against. The run asserts that at least half of the cases are clean, so
  the differential can't become vacuous.

### 8.2 Execution

The default is a fixed seed and 64 programs in the normal suite. `TW_FUZZ=<n>` runs more, with a
random, printed seed.

Draw N value trees with `Strategy::new_tree`, build them all in one batch, then compare. On failure,
drive `simplify`/`complicate` by hand against **only the failing backend and stage**, one build per
candidate, until `TW_SHRINK_SECS` runs out, keeping the smallest failing program found.

### 8.3 Gating

| Check | Normal run | With `TW_FUZZ` |
| --- | --- | --- |
| Checker accepts, no `Stuck`, round-trip, memo on == off, inlined interpreter == original | **Gates**; shrinks and writes | Same |
| Backend listed in `L6_STRICT` | **Gates**; shrinks and writes | Same |
| Backend not listed | Printed count and first reasons; passes; no shrink or write | Passes; shrinks and writes |

`L6_STRICT: &[&str]` is a constant in `tests/random.rs`, empty at first. A backend is added once it's
conformant.

### 8.4 Regressions

- Locally, a failure is written to `corpus/regressions/<content-hash>.tw` via `print_test_file`, with
  a comment holding the seed and the failing backend and stage. It has no `@knownfail`, so it fails
  until it's fixed or annotated. Review these before committing, especially after `TW_FUZZ` runs while
  gaps remain open.
- When `CI` is set, the text goes to the log and to `$CARGO_TARGET_TMPDIR/tw-regressions/`, which the
  workflow uploads on failure. The source tree isn't touched.
- Regressions are hand-reviewed once committed, then ordinary corpus files.

## 9. Corpus, CI and milestones

### 9.1 Decision traces: future work

`TESTING.md` §9.4's first-diverging-decision report is deferred. The first version reports what §6.5
lists. When it's built, the leading candidate is an IR instrumentation pass that, on a mismatch,
rewrites each `If`/`While` condition to call a `__trace(site, ambient(pos3), cond)` extern and reruns
both sides. That leaves verified code untouched and gives every backend a trace mode. The alternative,
a hook in `tungsten-core`, would add state to future refinement proofs.

### 9.2 Corpus

Required coverage is `TESTING.md` §11, at least one file each: every operator × type, overflow and
shift masks, every cast edge case, `-0.0`, short-circuit with an erroring right side, break and return
in nested loops, `Repeat 0`, consts referencing consts, density inputs at `(1,1,1)` / equal / different
dims, explicit-index grid-edge reads, the scale-key collision. Additionally: each extern with
hand-computed `@expect`, `@expect error(OutOfFuel)`, an unread failing const, and a failing input that
nothing reads. Top-level dirs: `ops/`, `casts/`, `floats/`, `control/`, `density/`, `externs/`,
`prelude/`, `errors/`, `reject/`, `regressions/`.

### 9.3 CI

New `.github/workflows/test.yml`, on push and pull request: JDK 21, stable Rust, cache `target/`, and

```
TW_BACKENDS=rust,java TW_REQUIRE=rust,java INSTA_UPDATE=no cargo test --workspace
```

Uploads `tw-regressions/` on failure. CUDA and Verus stay local.

### 9.4 Milestones

Each one lands green.

1. **Foundation.** Crate, `registry.toml`, `TestEnv`, shared externs, prelude, trials `roundtrip`,
   `lowering`, `interp`, `inline`, and a first corpus. No backend runs. The interpreter is exercised for
   real for the first time.
2. **Rust differential.** `entry_method_names`, Rust runtime, driver, protocol, batches, fallback,
   watchdog, scoring, knownfail reconciliation. Gaps become `@knownfail`, logged in `PAPER.md`.
3. **Java differential and CI.** Java runtime and driver, full corpus, workflow.
4. **L6 (out of scope for now, deferred).** Generator, batch sampling, manual shrinking, regressions,
   report-only gating. CUDA (5) is done first.
5. **CUDA.** `tw/` runtime, exact profile, C++ host driver, error-case skipping, GPU serialization.

## 10. Amendments to `TESTING.md`

Applied to `TESTING.md`:

| Section | Change |
| --- | --- |
| §2 Termination, §9.2 | `OutOfFuel` is interpreter-only; backends are never run on such cases, and a timeout is no longer its counterpart (§4.1) |
| §2 Execution, §9.2 | Batch per corpus dir with per-test fallback; one process per `(test, stage)`, request/response protocol; direct `rustc`; cache under `CARGO_TARGET_TMPDIR` |
| §2 Execution | `TW_BACKENDS` and `TW_REQUIRE` |
| §3 | Layout per §2 here; `registry.toml` loader in `src/registry.rs` |
| §9.3, §9.4 | Build failure always fails; `--cap-lints allow`; scoring table |
| §9.4 | First-diverging-decision report is future work |
| §10 | Primitive externs live in the shared Rust runtime rather than `prim.rs`; extern names; `test_domain`; toy tables per §5.3; records generated from the registry |
| §11 L2 | Invariants and insta snapshot, excluding regressions |
| §11 L6 | Report-only for backends until listed in `L6_STRICT`; shrinking and writing rules |
| §13 | `Vec3` is f64 for the Rust backend: generated code imports `crate::mathf64`. The `origin` question stays open |

## 11. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Scope | Phased: Rust, then Java, then CUDA (L6 deferred), with the design fitting all of them | All at once: the largest block of work precedes anything running. Excluding CUDA: risks protocol rework. |
| Build isolation | Batch with per-test fallback | Always per test: too slow across stages and L6. Parsing diagnostics: tied to compiler formats, and one error can hide others. |
| Build failure on error cases | Always a failure | Accepting when all cases error: real compile bugs pass silently. Matching E0080: ties the harness to rustc codes. |
| Test runner | `libtest-mimic`, one trial per check | One `#[test]` per level: no per-file filtering. `build.rs` generation: more machinery for the same result. |
| `OutOfFuel` | Interpreter-only | Requiring a hang: costs a timeout per case and can't tell slow from infinite. Banning it: fuel couldn't be covered by the corpus. |
| Unread failing const | Strict, as §9.4 says; backends must evaluate all listed consts, `@knownfail` until they do | Don't-care: weaker contract, though it would spare production output. Checker rejection: changes verified code and could trip on legitimate IR. |
| Decision trace | Future work | Core hook: enlarges proof surface. Dropping it entirely: it is still wanted. |
| Processes | One per `(test, stage)`, resume after a crash | Long-lived per batch: state leaks and restart logic. Per case: JVM start per case. |
| L6 and backend gaps | Report-only until a backend is listed in `L6_STRICT`; interpreter-side checks always gate | Gap table by IR scan: bookkeeping that churns with every fix. Generator flags: all backends lose coverage. |
| L6 shrinking | Batch sample, manual shrink, time budget, failing backend only | Plain proptest runner: one release build per program. No shrinking: random failures are too large to debug. |
| Regressions | Write locally, log and artifact in CI | `TW_FUZZ`-only writes: manual copying. Under `target/`: lost on `cargo clean`. |
| L2 | Invariants plus `insta` snapshots | Invariants only: misses silent lowering drift. Hand-rolled bless: same churn plus custom tooling. |
| Backend conformance | Out of scope; gaps stay `@knownfail` | Fixing §4.3 or all of §4 here: bigger, and loses a clean record of bugs found. |
| Batch shape | One per top-level corpus directory, per backend and stage | Whole corpus: any edit rebuilds everything. Hash shards: batches with no meaning. |
| Toolchains | Auto-detect; CI sets `TW_REQUIRE` | CUDA opt-in: risks going stale. All explicit: Java regressions only seen in CI. |
| Extern impl | One shared Rust source | Two copies: drift gives false alarms for one-line functions. |
| Records | Generated from `registry.toml` | Hand-written: three edits per record, and mismatches surface only in tests that use them. |
| Toy tables | Over the initializer's integer arguments | Passing strings: changes production backends purely for tests. |

Decided without an interview question, and easy to revisit: a fresh `Orchestration` per case, direct
`rustc`, `_f32` suffix instead of overloads, `@cells` comparing after a full run, a small stable hash for cache keys,
a 10-second watchdog, the default L6 count of 64, and `TW_SHRINK_COUNT`.

## 12. Validation plan

The harness itself must be shown to detect failures, or a green run means nothing:

- **Mutation self-tests** (`tests/corpus.rs`, under `selftest::`): deliberately wrong generated code or
  a stub driver must produce the expected verdict. One each for a wrong value, a sign-of-zero difference,
  differing NaN payloads (must **pass**), a crash, a hang, a truncated response, a build error in one
  test of a batch (others unaffected), a stale knownfail, and a missing case resumed after a crash.
- **Protocol**: encode/decode round-trip proptest over all value types; golden bytes shared by Rust
  and Java drivers.
- **Cross-language externs**: `runtime_coverage`, plus per-extern corpus files.
- **Toy tables**: golden values for fixed seeds, in all languages.
- **Cache**: a second run performs no builds, and changing a runtime file invalidates the key.
- **End-to-end**: at milestone 3, the example in `TESTING.md` §8.2 passes through every stage on
  Rust and Java, or has a justified `@knownfail`.
- **L6** (deferred with §8): acceptance rate is 100%, the clean-case floor is asserted, and shrinking is demonstrated
  against a stub backend with a planted bug.

## 13. Open items

- Confirm that every density input of an entry receives the entry's `origin` in tungsten-mc
  (`TESTING.md` §13).
- ~~Rust-generated `HostConsts` and `Orchestration` members must be visible to a sibling `driver` module.~~
  Confirmed in milestone 2: the driver builds `HostConsts { .. }` and calls `Orchestration` methods, and no
  backend change was needed.
- Inlining with `@knownfail` has no interpreter-level escape hatch by design (§3.3). If a known inliner
  bug has to wait for a long fix, a `stage=inlined` skip could be added.
- Rust and Java compile time for a whole batch is unmeasured. If a directory becomes slow, split it.
- ~~CUDA specifics (host-driver layout, GPU job limit)~~ Settled in milestone 5 (§14).

## 14. Implementation notes

### Milestone 1 (Foundation)

Implemented: the crate, `registry.toml` loader, `TestEnv`, shared Rust externs and toy tables,
prelude, the `roundtrip`, `lowering` (invariants plus insta snapshots in `tests/snapshots/`),
`interp` and `inline` trials, and a first corpus of 37 files. Deviations:

- **Runtime files.** `runtime/rust/` holds `externs.rs` and `utilsf64.rs` (toy tables). `mathf64.rs`
  is not created yet: the interpreter doesn't need it, and its contents (`Vec3` math for generated
  code) belong to milestone 2.
- **Toy-table details.** §5.3 leaves the exact folding open. `splitmix64(x)` is the pure mix of
  `x + 0x9E3779B97F4A7C15`; `fold` is `acc = splitmix64(acc ^ a)` starting from the first argument;
  Fisher-Yates draws `splitmix64(state + k·gamma)` and takes it modulo the range. Golden values for
  the language ports belong with milestone 2.
- **Registry.** Besides `Pair`, `registry.toml` has a record `Wrap` (record inside a record) for
  later tests. Nothing in the corpus uses it yet.
- **Lowering invariant.** Density-input vars are not core locals, so the locals-count check excludes
  `density`-typed vars.
- **`@knownfail` case names** are already validated by the parser ("names no case"); the harness
  re-checks backend and stage only as a second line.
- **`::inline` compares entries only.** Const and input ids change under inlining, so
  `CaseResult::failures` is not compared across stages.
- **Not done in milestone 1:** the "entry returns `PermTable`/`Sampler`" test error (it is a backend
  trial error), `entry_method_names`, drivers, protocol, batches, `tests/random.rs`, CI workflow. For
  `entry_method_names`: Rust seeds its `used` set with `"new"` and Java with `rules.used_set()`, so the
  shared function needs a seed parameter to leave generated output unchanged.
- **Prelude accuracy.** Transcendentals are series approximations (`exp` range-reduced, `log` via
  atanh series). Corpus files check exact points and agreement with libm to 1e-9.

### Milestone 2 (Rust differential)

Implemented: `entry_method_names`, the Rust runtime, protocol, driver generator, child processes with
a watchdog and crash resume, batches with the content-hash cache and per-test fallback, scoring,
`@knownfail` reconciliation, the `::rust::original` and `::rust::inlined` trials, the validation of §12
(except the Java half of the golden bytes), and 26 new corpus files. Deviations and details:

- **`entry_method_names`** takes the seed set as a third parameter
  (`entry_method_names(program, rules, used)`), as predicted. Rust output for the whole corpus was diffed
  before and after and is byte-identical. Java is the same loop and its tests pass, but its output was not
  diffed.
- **Runtime files.** `mathf64.rs` is vendored from `tungsten-frontends/tungsten-mc/src/{mathf64,utilsf64}.rs`
  (commit 88b88ae of that repo): `Vec3`, `Pos3`, their operators, `index`, `Iter3D`, `PositionIterator`,
  `iter_3d`, `Config`. It adds `Neg for Vec3`. New files: `api.rs` (the registry externs by plain name, the
  toy samplers, `make_boxed`) and `proto.rs` (driver side of §7.4 and the request loop). Upstream's `make_boxed`
  returns uninitialised memory, so ours is zero-initialised. `min`/`max`/`floor`/`ceil`/`pow` are not taken
  from upstream: the registry versions have Java semantics.
- **Rename.** The pure toy-table functions in `utilsf64.rs` are now `perm_lookup` and `sample_state`, so the
  glob imports of generated code can't see two `perm_get` / `sampler_sample`.
- **Index type.** `index(p)` returns `usize` upstream (not `i32`), and the iterator type is `Iter3D`, not
  `PositionIter`. The generated code is `buffers.x[index(p) as usize]`.
- **Module layout.** Generated modules start with `use super::*;`, not `use crate::mathf64::*;`. Each
  `t_<hash>.rs` therefore begins with those two imports, then holds the backend's `density_function` and
  `orchestration` modules and a generated `driver` module. Records are `records.rs`, re-exported through
  `mathf64`.
- **Panic mapping.** Rust's own panics are mapped to protocol kinds by message (`divide by zero` is 0,
  `out of bounds` is 1, the marker `tw:ExternDomain` raised by `test_domain` is 2, anything else, such as
  `attempt to add with overflow`, is 3). A kind-3 failure still counts as "the backend failed".
- **Batch contents.** A batch holds the files of a directory that load, aren't reject files, aren't
  `@tier(large)` (unless `TW_LARGE=1`) and have at least one case the backend can run. Both stages are
  separate batches. `CARGO_TARGET_TMPDIR` reaches the library as a parameter of `Harness::new`, since
  only integration tests have it.
- **Knownfail scope.** Reconciliation is exactly per `(case, stage)` as in §6.4, so a `@knownfail` without
  `case=` fails the trial when any of the cases it covers passes. All 19 current knownfails have a
  single-case file.
- **Corpus split.** `ops/i8`, `ops/i32`, `ops/i64` and `casts/to_i8` were split into one file per
  concern, because a single panic hides the rest of the array. The old files keep the operations that
  already agree. Their snapshots in `tests/snapshots/` were regenerated and the new ones added.
- **`CellSample` rule.** `@cells(sample, seed)` compares the `sample` cells with the smallest
  `splitmix64(seed ^ entry << 40 ^ cell)`.
- **Selftests** drive stub drivers through the real build, process and scoring code (14 of them,
  `tests/selftest/mod.rs`). Golden bytes are `tests/golden/protocol.txt`, checked from the Rust side
  (host encoder and driver-side `Writer`/`Reader`); the Java driver reuses the file in milestone 3.
  Toy-table goldens (`src/externs.rs`) were cross-checked against an independent Python implementation.
- **Not done:** a Java side for the goldens, decision traces, and any fix of the gaps below.

Gaps found (all `@knownfail(rust, ...)`, logged in `PAPER.md`): integer overflow does not wrap (15 files),
`>>>` is emitted as `>>` (2 files), float to `i8` casts saturate (1 file), and a failing `const` is a
build error (1 file).

### Milestone 3 (Java differential and CI)

Implemented: the Java runtime (`runtime/java/tungsten/runtime/`), `harness/java.rs` (records, `Driver`,
`generate_test`), a `Lang` in `Harness` (`Harness::new` is Rust, `Harness::new_java` is Java; each has its
own cache under `tw-cache/<lang>/`), `process::run_cases_cmd`, the `::java::original` and `::java::inlined`
trials, the Java selftests, and `.github/workflows/test.yml`. Details:

- **Runtime.** `Vec3`, `Pos3`, `Config` and `PositionIterator` are vendored unchanged from
  `tungsten-frontends/tungsten-diff-harness/java_wrapper/runtime`. Its `Helpers` and samplers implement the
  real Minecraft noise, not the registry externs and toy tables the interpreter uses (§5.3), so they are not
  used. New: `Helpers` (registry externs and toy table constructors), the toy `PerlinNoiseSampler` and
  `InterpolatedNoiseSampler`, the six `*Ref` holders, and `Proto` (Reader, Writer, request loop).
- **Build.** One `javac --release 21` run per `(directory, stage)` batch over a source tree; files of a test
  are `t_<hash>/Gen.java` (backend output) and `t_<hash>/Driver.java`. Source blobs separate files with
  `//@@FILE <path>` lines. Run as `java -XX:+UseSerialGC -cp <classes> t_<hash>.Driver`, with
  `STARTUP_ALLOWANCE` added to the first case of each process.
- **Failing constants.** `ExceptionInInitializerError` is reported as its cause, and the
  `NoClassDefFoundError` of later cases as the first cause, so all of them score by the original kind.
- **Probe.** `javac -version` and `java -version` must report 21 or newer (`$JAVAC` and `$JAVA` override the
  commands). With `TW_REQUIRE=java` a missing toolchain exits with code 2.
- **Validation of §12.** Java golden bytes (`selftest::java::golden_bytes`), toy-table goldens
  (`toy_table_goldens`), `runtime_coverage` (generated from the registry; a missing extern fails to compile),
  stub-driver selftests, and a mutation check on the real path (a changed `copySign` turns `externs/math_*` red).
- **Gaps.** 8 corpus files carry `@knownfail(java, ...)` (see `PAPER.md`). The before/after diff of the
  generated Java across the `entry_method_names` extraction was done: 118 outputs (59 files, both stages; 14 are the backend panics logged in `PAPER.md`), byte-identical.


### Milestone 5 (CUDA)

Implemented: `entry_base_names`, the header-only runtime (`runtime/cuda/tw/`), `harness/cuda.rs` (records,
host driver, `main.cu`), `Lang::Cuda` with `Harness::new_cuda` and `probe_cuda`, error-case skipping, GPU
serialization, the `::cuda::original` and `::cuda::inlined` trials, and the CUDA selftests
(`tests/selftest/cuda.rs`). Details and decisions:

- **`entry_base_names(program, rules, used)`** in `tungsten_interface::naming` replaces the inline
  `unique(unit_base_name)` loop of `tungsten-cuda/src/orchestration.rs`; the driver uses it too. The generated
  CUDA for the whole corpus is unchanged (cache keys of the batches did not move, so nothing was rebuilt).
- **Runtime.** Vector operators, `Array`, `Config`, `PositionIterator` and the saturating casts follow the
  production runtime in `../cuda-density/src/tw/`. Its Perlin and Base3D samplers and Minecraft externs are
  not used: the interpreter compares against the toy tables (§5.3), so `tungsten_runtime.cuh` has the toy
  `PerlinNoiseSampler { perm[256] }` and `InterpolatedNoiseSampler { state }` and `profile_exact.cuh` the
  registry externs (Java `min`/`max`, canonical-NaN bit casts). Everything is `__host__ __device__`, so the
  selftests check goldens on both sides. `tw::f2i8` clamps, as the CUDA README says.
- **Host-driver layout (settled).** One `nvcc` run per test builds `t_<hash>.cu`, which includes
  `density.cu`, `orchestration.cu` and a generated driver, in the namespace `t_<hash>` (also the
  `CudaConfig::namespace`, so kernel symbols are `t_<hash>_...`). No `-rdc`. A generated `main.cu` dispatches
  on the test id. The objects are cached by content under `tw-cache/cuda/obj/` and linked per batch, so the
  per-test fallback of §7.2 costs a link, not a recompile. The driver creates a fresh `Orchestration` per
  case, enqueues every entry, syncs once and encodes the pinned results. Records are `tw/records.cuh`,
  generated from `registry.toml`.
- **Flags.** `-std=c++20 -O3 -w --fmad=false --prec-div=true --prec-sqrt=true --ftz=false -arch=sm_XY`
  (compute capability of GPU 0; the nvcc version, GPU list and arch are in the cache key). No
  `--use_fast_math`. `debug_checks` stay off because erroring cases are not run.
- **Error-case skipping.** `oracle::skipped_for_backend(result, backend)`: `OutOfFuel` everywhere, and on
  CUDA any case with an interpreter cell error or const/input failure. Such cases are `Skipped` even when the
  build fails (§6.3 says a build failure fails every case that would have run).
- **Sticky errors.** The driver answers a CUDA error with a kind-3 failure and exits. `process::run_cases_opts`
  has `restart_after_failure`, which kills the process after a reported failure so the next case starts
  clean (selftest `device_trap_ends_process_and_resumes` uses a real `__trap`).
- **GPU job limit (settled).** One global lock around the child-process phase of a CUDA trial; builds
  are CPU-only and run in parallel (up to 8 `nvcc` at once).
- **Cost.** A cold run of the whole corpus on both stages takes about 45 s on a GTX 1650 Ti (16 cores);
  warm, about 25 s, most of it CUDA context creation per process.
- **Validation.** CUDA golden bytes, toy-table goldens (host and device), `runtime_coverage` (every extern
  called in a kernel), stub-driver selftests (wrong value, sign of zero, NaN payloads, crash, hang, truncated
  response, resume after crash, build error in one test, cache), and a mutation check on the real path
  (`copysign` arguments swapped in `profile_exact.cuh`: `externs/math_f64` went red).
- **Gaps.** One corpus file carries `@knownfail(cuda, ...)`: `casts/to_i8_from_float` (see `PAPER.md`). The
  other 58 files agree bit for bit on both stages; CUDA is the only backend with correct integer wrapping,
  masked shifts and `>>>` so far.
