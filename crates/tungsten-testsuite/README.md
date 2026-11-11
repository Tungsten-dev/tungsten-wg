# tungsten-testsuite

A test harness for tungsten. It runs small `.tw` programs through the parser, the lowering, the
interpreter and the inliner, and checks the results against hand-written expectations.

The crate is unpublished (`publish = false`). The design is below, and
the testing policy is in `TESTING.md`.

## Status

Built. The suite tests the parser, the lowering, the interpreter and the
inliner, and it compiles every program with the Rust, Java and CUDA backends, runs it out of process and compares
every value with the interpreter, on the original and on the inlined program. CUDA runs locally only
(it needs `nvcc` and a GPU). Not built: the random program generator, decision traces and a CI workflow
(see "Deferred work and open questions" below).

## What it checks

Every file under `corpus/` gets these tests. The test names look like `ops/i32::interp`, so you can
filter with `cargo test`.

| Test               | What it checks                                                                                                                                                                         |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `::roundtrip`      | Printing a program and parsing it back gives the same program, and printing again gives the same text.                                                                                 |
| `::lowering`       | Lowering keeps the counts of functions, constants, hosts and so on, and gives the same result twice. It also compares the lowered program with a saved snapshot in `tests/snapshots/`. |
| `::interp`         | Every `@case` runs on the interpreter and matches its `@expect` lines. It runs with memoization on and off, and both must agree.                                                       |
| `::inline`         | After inlining, the interpreter gives the same cell values as before.                                                                                                                  |
| `::rust::original` | The program is compiled by the Rust backend, run on every case, and every value is compared bit for bit with the interpreter (any NaN equals any NaN).                                 |
| `::rust::inlined`  | The same for the inlined program. The expected values are still the interpreter's on the original program.                                                                             |
| `::load`           | Only appears if the file doesn't load or a directive is invalid. The file then gets no other tests.                                                                                    |

Files that expect a rejection (`@expect reject`) only get `roundtrip`, `lowering` (if lowering
works) and `interp`.

## Running it

```
cargo test -p tungsten-testsuite                       # everything
cargo test -p tungsten-testsuite --test corpus         # interpreter and inliner only
cargo test -p tungsten-testsuite -- ops/               # one corpus directory
```

If you change the lowering on purpose, the snapshots will differ. Review and accept the new ones
with `INSTA_UPDATE=always cargo test -p tungsten-testsuite --test lowering`, then look at the diff
in `tests/snapshots/`. CI should set `INSTA_UPDATE=no`.

Environment variables. `cuda` is added by default when `nvcc` (`$NVCC`, else `PATH`, else
`/usr/local/cuda/bin/nvcc`) and a GPU in `nvidia-smi -L` are found. Toolchain probes run once: `rustc`
(`$RUSTC`), `javac` and `java` (`$JAVAC`, `$JAVA`; both must be 21 or newer) and `nvcc`:

- `TW_BACKENDS`: comma list from `rust`, `java`, `cuda`. Default: `rust,java`, plus `cuda` when detected.
  An unknown name fails the run.
- `TW_REQUIRE`: backends whose missing toolchain should fail the run (exit code 2). Without it, the
  backend trials are ignored, with the reason shown.
- `TW_LARGE=1`: also run files marked `@tier(large)`.
- `TW_TIMEOUT_SECS`: watchdog per case, default 10 (ten times that for `@tier(large)`).

Builds are cached under `target/tmp/tw-cache/<backend>/<hash>/` (the hash covers generated sources,
runtime files, compiler flags and the compiler version; for CUDA also the GPU list and arch), so a second
run builds nothing. Builds go to a temp directory and are renamed into place, so concurrent test binaries
are safe. A failed build leaves its sources in `<hash>.failed/`.

| Backend | Build                                                                                                                                                                                                                     |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust    | `rustc --edition 2024 -C opt-level=3 -C overflow-checks=on -C panic=unwind --cap-lints allow` (direct `rustc`: no `Cargo.toml` and no nested cargo locks; `--cap-lints allow` so a literal `1 / 0` still reaches runtime) |
| Java    | `javac --release 21`, run as `java -XX:+UseSerialGC -cp <classes> t_<hash>.Driver`                                                                                                                                        |
| CUDA    | `nvcc -std=c++20 -O3 -w --fmad=false --prec-div=true --prec-sqrt=true --ftz=false -arch=sm_XY` (arch of GPU 0; never `--use_fast_math`)                                                                                   |

A cold run of the whole corpus on both stages with CUDA takes about 45 s on a GTX 1650 Ti, a warm run about 25 s.

Failures print the case, the entry and cell, and both values in decimal and hex. A test that fails on
purpose is marked `@knownfail(rust, "reason")` (optionally `case="..."`, `stage=original|inlined`). It is
printed as an expected failure, and it fails the run as soon as it starts passing ("stale knownfail").

## Adding a test

Drop a `.tw` file into a directory under `corpus/`. No code changes are needed. A file holds a
`program { ... }`, some `@case` lines that give inputs, and `@expect` lines that give the expected
outputs. The format is described in `crates/tungsten-parse/SPEC.md`. For example:

```
program {
    fn run() -> i32 { return (2147483647 + 1); }
    unit u { main run; }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
@case "c" origin(0, 0, 0) param(seed = 1)
@expect "c" u[0, 0, 0] = -2147483648
```

Guidelines:

- Keep files small and focused, one concern each. A `@knownfail` hides everything else that fails in
  the same scope, so a file that mixes a known bug with working checks would stop guarding the latter.
- Compute expected values by hand, not by running the interpreter.
- A case that runs out of fuel must say `@expect "case" error(OutOfFuel)`. `OutOfFuel` is only
  produced by the interpreter. Backends have no fuel and are never sent such a case.
- Expectations compare floats bit for bit, except that any NaN matches any NaN.
- Files in `corpus/regressions/` are for shrunk failures found by the future random tester. They
  don't get snapshots, and nothing writes to the directory yet.
- Required coverage (`TESTING.md` §11): every operator and type, overflow and shift masks, every cast edge
  case, `-0.0`, short-circuit with an erroring right side, break and return in nested loops, `Repeat 0`,
  consts referencing consts, density inputs at `(1,1,1)`, equal and different dims, explicit-index grid-edge
  reads and the scale-key collision. Each extern has hand-computed `@expect` lines.

## Layout

```
registry.toml        externs and records the test programs may use
prelude/prelude.tw   sin, cos, exp, log, pow, lerp, smoothstep, as_index, corner helpers
runtime/rust/        what generated Rust code links against: math and iteration (vendored from
                     tungsten-mc, see the header of mathf64.rs), the registry externs, toy tables, and
                     the driver side of the protocol. The interpreter shares externs.rs and utilsf64.rs.
corpus/<dir>/*.tw    the test programs: ops, casts, floats, control, density, externs, prelude, errors,
                     records, entries, cells, large, reject, regressions
src/                 registry loader, TestEnv, extern adapter, value conversion, oracle, scoring
                     (compare.rs), knownfail reconciliation, corpus loading
src/harness/         protocol, Rust driver generator, batches and cache, child processes, backend trials
tests/               roundtrip.rs, lowering.rs, corpus.rs (all use libtest-mimic), selftest/ (the harness
                     checking itself with stub drivers), golden/ (protocol bytes) and snapshots/
```

## Things worth knowing

- **Externs.** Test programs call extern functions such as `sqrt`, `min` or `test_domain`, which are
  listed in `registry.toml`. They are written once in `runtime/rust/`. `min` and `max` follow Java
  (`-0.0 < 0.0`, NaN wins). `test_domain` fails for negative input so the extern-domain error can be
  tested.
- **Toy tables and initializers.** The testsuite acts as a small frontend. Real Perlin and Base3D noise are
  not tested here: the permutation table and sampler are simple functions of integer initializer
  arguments, and the value initializers (`make_salt`, `make_bits`, …) cover the other argument kinds and
  scalar types (see `SPEC.md` §5.3). They are exact in every language, so backends are compared bit for
  bit. `tests/isolation.rs` fails if `perlin`, `base3d` or `md5` appear in the library crates.
- **The prelude.** The transcendentals are series approximations built from `+ - * /`, so every backend
  compiles the same arithmetic. The tests check exact points and agreement with libm to 1e-9.
- **Known backend gaps.** Integer overflow, `>>>`, float to `i8` casts and failing constants on the
  Rust backend, and a panic and a cast bug on the Java backend, are marked `@knownfail` (`grep -rl @knownfail corpus`); `PAPER.md` has the log.
- **CUDA.** The runtime is header-only in `runtime/cuda/tw/` (core types follow `../cuda-density`, the
  samplers and externs are the registry's toy versions). Each test is one `nvcc` run over a `t_<hash>.cu`
  that includes the four generated files and a generated host driver; objects are cached per test and
  linked into one binary. Cases where the interpreter errors are not run on CUDA, and GPU jobs run one
  at a time. A reported failure ends the process, since device errors are sticky.
- **Vendored runtime.** `runtime/rust/mathf64.rs` is vendored from `tungsten-frontends/tungsten-mc` (commit
  88b88ae of that repo; adds `Neg for Vec3`), except `min`/`max`/`floor`/`ceil`/`pow`, whose registry versions
  have Java semantics, and `Vec3` and `Pos3` also derive `Default`. Generated code no longer calls `make_boxed`, so the
  runtime has none. The Java
  `Vec3`, `Pos3`, `Config` and `PositionIterator` come from `tungsten-diff-harness/java_wrapper/runtime`; its
  `Helpers` and samplers are real Minecraft noise, so the suite has its own. The CUDA vector operators, `Array`,
  `Config`, `PositionIterator` and saturating casts follow `../cuda-density/src/tw/`; `tw::f2i8` clamps.
  The pure toy-table functions in `utilsf64.rs` are named `perm_lookup` and `sample_state`, so glob imports of
  generated code can't see two `perm_get`/`sampler_sample`.
- **Naming outside the crate.** The CUDA driver calls `enqueue_<base>`, whose names come from
  `tungsten_interface::naming::{entry_method_names, entry_base_names}`, shared with the backend. The Rust and
  Java drivers create one `Orchestration` per case and one worker per entry, with the names from
  `tungsten_rust::orchestration::rust_worker_names` and `tungsten_java::orchestration::java_worker_names`, and bind
  only that entry's hosts (`specs/workers.md`). `tests/rust_workers.rs` and `tests/java_workers.rs` cover what the
  corpus can't: threads (`std::thread` and an `ExecutorService`), the bounds modes, marched slices and worker
  isolation, and on Rust the `Send + Sync` assertion and records without `Default`.
- **What is not covered yet.** The random program generator and decision traces (see below).

## Rules and contracts

Semantics are in `TESTING.md` §5, §6 and §9. The harness-specific rules:

**Trials.** Each check is one `libtest-mimic` trial named `<path>::<check>` (`<path>` relative to `corpus/`),
so `cargo test` filters work. A trial is ignored with a reason if its backend is unselected or its toolchain is
missing, or if the file is `@tier(large)` without `TW_LARGE=1`. `::load` appears only if loading fails or a
`@knownfail` directive is invalid (unknown backend, case or stage); the file then gets no other trials.

**Interpreter stage.** `EvalFailure::Stuck` is always a failure (a checker or interpreter bug). A rejected
program fails unless the file says `@expect reject`. `@knownfail` never applies to interpreter trials, so an
inliner bug keeps `::inline` red until fixed. Hosts are bound by name, fuel comes from `@fuel`, else
`DEFAULT_FUEL`. `::inline` compares entries only, since const and input ids change under inlining.

**`@expect`.** `Cell`, `All` and `Grid` check only the named cells, which must be `Ok`. `error(Kind)` needs at
least one failing cell, and the first failing one (lowest entry index, then flat order) must have that kind:
`DivByZero`, `OutOfBounds`, `OutOfFuel` or `ExternDomain` (`Stuck` can't be expected). `reject(Kind)` matches
a lowering or checker error by variant name, any rejection without a `Kind`. An entry returning `PermTable` or
`Sampler` can't be encoded and is a test error on every backend trial.

**`OutOfFuel` is interpreter-only.** Backends have no fuel. A case with `OutOfFuel` fails `::interp` unless it
says `@expect "<case>" error(OutOfFuel)`, and is never sent to a backend. A watchdog timeout on any other case
is an ordinary backend failure.

**Eager failures are strict.** A failing const or input fails the whole case, read or not, and each backend must
fail it. Backends that only emit used constants are `@knownfail` until they emit all listed consts.

**Scoring per case:**

| Situation                                                       | Result                                          |
| --------------------------------------------------------------- | ----------------------------------------------- |
| Interpreter `OutOfFuel` anywhere                                | Not run, not scored                             |
| Interpreter error on CUDA                                       | Not run, not scored (also when the build fails) |
| Interpreter clean, backend returns equal values                 | Pass                                            |
| Interpreter clean, backend differs, fails, crashes or times out | Fail                                            |
| Interpreter error, backend fails, crashes or times out          | Pass; the kind is shown, never compared         |
| Interpreter error, backend returns                              | Fail                                            |
| `generate` panics or errors, or the build fails                 | Fail, for every case that would have run        |

A build failure always fails, even where the interpreter predicts an error, so "compiled means comparable" and
a compile bug can't hide in an error-only test. `generate` runs inside `catch_unwind`, because backends panic on
underivable types. Values are equal if integers and `Bool` match, and floats match by bits (any NaN equals any NaN
of the same width). With `@cells(sample, seed)` the harness compares the `sample` cells with the smallest
`splitmix64(seed ^ entry << 40 ^ cell)`; all cells still run.

**Knownfail reconciliation**, per `(case, stage)`: covered and failing is expected and printed with its reason;
covered and passing fails the trial ("stale knownfail"); uncovered and failing fails the trial. A `@knownfail`
without `case=` therefore fails when any case it covers passes. Knownfails for an ignored or unscored backend
are not evaluated.

**Batches.** A batch is `(backend, top-level corpus dir, stage)`: the files that load, aren't reject files, aren't
`@tier(large)` (unless `TW_LARGE=1`) and have a case the backend can run. A filter that selects any test in a batch
builds the whole batch. If a batch fails to build, each of its tests is built alone (parallel, separately cached);
tests that compile alone run from their own artifact, the others fail for all their cases, and the batch failure
itself is not reported. Each test lives in its own namespace `t_<hash>` (Rust `pub mod`, Java package, CUDA
namespace).

**Processes.** One process per `(test, stage)`, with a fresh `Orchestration` per case so state can't leak.
Cases are request/response with a watchdog (plus a startup allowance on the first case for the JVM), which avoids
pipe deadlocks. A crash or timeout fails that case, kills the process and resumes from the next case in a new
one. Rust maps panics to protocol kinds by message (`divide by zero` 0, `out of bounds` 1, marker
`tw:ExternDomain` from `test_domain` 2, anything else such as an overflow panic 3). Java catches `Throwable` per
case and reports `ExceptionInInitializerError` (failing constants) and the `NoClassDefFoundError` after it by
the original cause. CUDA errors are sticky, so a reported failure ends the process and the next case starts
clean; one global lock serializes the GPU phase, while builds are CPU-only and run in parallel (up to 8 `nvcc`).

**Protocol.** Little-endian on stdin/stdout, flushed per message, driver logs to stderr only.

```
request  := origin:[u64;3] (f64 bits)  param* host*  steps:u32  step*      (host arena order); EOF ends the process
step     := refinement:u32  offset:[i32;3]  host*                          (every host, resolved by the harness)
response := 0x00 value* (entry order, flat cell order) step_value*         (the step entry's buffer per step)
          | 0x01 kind:u8 len:u32 utf8[len]
kind     := 0 DivByZero | 1 OutOfBounds | 2 ExternDomain | 3 other
```

`steps` is always present and 0 for a program without refinements. The size of an `Ok` response depends on
the case's steps, so `run_cases_lens` takes one per request. The Rust driver reads the `steps` count, then
`STEP_LEN` bytes per step (`serve(request_len, step_len, run)`), and so does the Java driver
(`Proto.serve(requestLen, stepLen, run)`). The CUDA driver only reads the fixed part and ignores `steps`, because
the backend's guard refuses refinement programs until its own phase (`specs/combined-rollout.md` §3.3).
`tests/golden/protocol.txt` also holds a `step_request` and `step_response`, which only the host side checks (the
Rust and Java drivers are exercised by the `refine/` corpus).

Values are positional and typed by the program, with no tags: `Bool` and `I8` 1 byte, `I32` and `F32` 4,
`I64` and `F64` 8 (raw bits), `Vec3`, `Pos3`, arrays and records are their elements in order. A response of the
wrong length is a harness error, not a mismatch. A driver writes only after the whole case succeeds.

**Toy tables.** `splitmix64(x)` is the pure mix of `x + 0x9E3779B97F4A7C15`; `fold` is
`acc = splitmix64(acc ^ a)` starting from the first argument; `seed` is reinterpreted between `u64` and `i64`.

```
Perlin  state = fold over (seed, id_lo, id_hi, subident_index, sub_lo, sub_hi)
Base3D  state = splitmix64(seed)
perm    = Fisher-Yates over 0..256, draw k = splitmix64(state + k*gamma) mod the range
perm_get(t, i)       = perm[i & 255]
sampler_sample(s, v) = (h >>> 11) as f64 * 2^-53, h = fold over (state, f2i(floor v.x), f2i(floor v.y), f2i(floor v.z))
```

The interpreter derives the same inputs from `map.table_idents` via `tungsten_rust::random::xoroshiro_seed`,
inside a per-case `Externs` value, since the core `Case` has no seed. Records (`Pair`, `Wrap`) are generated per
language from `registry.toml`; extern functions stay hand-written; extern names are unique, so `f32` variants
take a `_f32` suffix.

**Lowering checks.** Over every non-reject, non-regression file: types and arity are preserved, every ref is
resolved, `lower` is deterministic (density-input vars are not core locals and are excluded from the locals
count), plus the `insta` snapshot of `{:#?}` of the lowered program. Regressions are excluded because their
contents are generated.

**The harness checks itself** (`tests/selftest/`, `selftest::`): stub drivers go through the real build, process
and scoring code for a wrong value, a sign-of-zero difference, differing NaN payloads (must pass), a crash, a
hang, a truncated response, a build error in one test of a batch, a stale knownfail, a missing case resumed
after a crash and, on CUDA, a real `__trap`. Golden protocol bytes (`tests/golden/protocol.txt`) are checked
from Rust, Java and CUDA, toy-table goldens in all languages (CUDA on host and device), and `runtime_coverage`
compiles a call to every registered extern per backend. Mutating a runtime extern (`copySign`, `copysign`)
turns `externs/math_*` red.

## Design decisions

| Topic                        | Decision                                                                            | Rejected, and why                                                                                                                                                                                                |
| ---------------------------- | ----------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Scope                        | Rust, then Java, then CUDA, with the design fitting all of them                     | All at once: the largest block of work precedes anything running. Excluding CUDA: risks protocol rework.                                                                                                         |
| Build isolation              | Batch per `(backend, dir, stage)` with per-test fallback                            | Always per test: too slow. Parsing compiler diagnostics: tied to compiler formats, and one error can hide others. Whole corpus as one batch: any edit rebuilds everything. Hash shards: batches with no meaning. |
| Build failure on error cases | Always a failure                                                                    | Accepting when all cases error: real compile bugs pass silently. Matching E0080: ties the harness to rustc codes.                                                                                                |
| Test runner                  | `libtest-mimic`, one trial per check                                                | One `#[test]` per level: no per-file filtering. `build.rs` generation: more machinery for the same result.                                                                                                       |
| `OutOfFuel`                  | Interpreter-only                                                                    | Requiring a hang: costs a timeout per case and can't tell slow from infinite. Banning it: fuel couldn't be covered by the corpus.                                                                                |
| Unread failing const         | Strict; backends must evaluate all listed consts, `@knownfail` until they do        | Don't-care: weaker contract, though it would spare production output. Checker rejection: changes verified code and could trip on legitimate IR.                                                                  |
| Processes                    | One per `(test, stage)`, resume after a crash                                       | Long-lived per batch: state leaks and restart logic. Per case: JVM start per case.                                                                                                                               |
| Lowering checks              | Invariants plus `insta` snapshots                                                   | Invariants only: misses silent lowering drift. Hand-rolled bless: same churn plus custom tooling.                                                                                                                |
| Backend conformance          | Out of scope; gaps stay `@knownfail`                                                | Fixing them here: bigger, and loses a clean record of bugs found.                                                                                                                                                |
| Toolchains                   | Auto-detect; `TW_REQUIRE` makes a missing one fail                                  | CUDA opt-in: risks going stale. All explicit: Java regressions only seen in CI.                                                                                                                                  |
| Extern implementation        | One shared Rust source for interpreter and Rust backend                             | Two copies: drift gives false alarms for one-line functions.                                                                                                                                                     |
| Records                      | Generated from `registry.toml`                                                      | Hand-written: three edits per record, and mismatches surface only in tests that use them.                                                                                                                        |
| Toy tables                   | Over the generated arguments (MD5 ids)                                              | Passing strings: changes production backends purely for tests.                                                                                                                                                   |
| Rust `rustc`                 | Direct `rustc` on a batch crate                                                     | Generated `Cargo.toml`: nested cargo locks for a crate with no dependencies.                                                                                                                                     |
| CUDA layout                  | One `nvcc` run per test (no `-rdc`), objects cached by content and linked per batch | Per-batch compile: a fallback would recompile everything.                                                                                                                                                        |
| GPU jobs                     | One global lock around the process phase                                            | Unbounded parallel GPU processes: sticky device errors and contention.                                                                                                                                           |

Small choices, easy to revisit: md5 for cache keys, a 10-second watchdog, `_f32` suffix instead of overloads,
`@cells` comparing after a full run.

## Deferred work and open questions

- **CI.** There is no workflow: one was written and removed on purpose, so tests only run locally. The command
  it would run is `TW_BACKENDS=rust,java TW_REQUIRE=rust,java INSTA_UPDATE=no cargo test --workspace` with
  JDK 21 and stable Rust. CUDA and Verus would stay local.
- **L6, random well-typed programs.** Nothing is built (no `gen.rs`, `tests/random.rs` or `TW_FUZZ`). Design to
  start from: a generator in `src/gen.rs` recording a `Vec<u64>` of choices that proptest generates and shrinks
  (the `tungsten-parse` generator doesn't produce well-typed programs), obeying `TESTING.md` §5.3, grids at most
  4x4x4, `Repeat` at most 8, `While` always a bounded counter, edge-biased literals, at least half of the cases
  clean. A normal run uses a fixed seed and 64 programs, all in one batch. On failure, shrink by hand
  (`simplify`/`complicate`) against only the failing backend and stage within a time budget. Report-only per
  backend until it is listed in a `L6_STRICT` constant; the interpreter-side checks always gate. Failures are
  written to `corpus/regressions/<content-hash>.tw` locally, and logged plus uploaded as an artifact under `CI`,
  never into the source tree. Rejected: a plain proptest runner (one release build per program), no shrinking
  (failures too large to debug), a gap table from an IR scan (churns with every fix).
- **Decision traces.** The first-diverging-decision report of `TESTING.md` §9.4 is not built. The leading
  candidate is an IR instrumentation pass that rewrites each `If`/`While` condition to call a
  `__trace(site, ambient(pos3), cond)` extern on a mismatch and reruns both sides. A hook in `tungsten-core`
  was rejected because it would add state to future refinement proofs.
- **Inliner bugs.** There is no escape hatch for a known inliner bug (`@knownfail` doesn't apply to
  interpreter trials). If one has to wait for a long fix, a `stage=inlined` skip could be added.
- **Compile time** of a whole batch is unmeasured; split a directory if it becomes slow.
- **`origin` in tungsten-mc.** Confirm that every density input of an entry receives the entry's `origin`
  (`TESTING.md` §13).
