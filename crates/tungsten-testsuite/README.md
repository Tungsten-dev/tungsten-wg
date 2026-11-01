# tungsten-testsuite

A test harness for tungsten. It runs small `.tw` programs through the parser, the lowering, the
interpreter and the inliner, and checks the results against hand-written expectations.

The crate is unpublished (`publish = false`). The design lives in `SPEC.md` at the repo root, and
the testing policy in `TESTING.md`.

## Status

Milestones 1 and 2 are built. The suite tests the parser, the lowering, the interpreter and the
inliner, and it compiles every program with the Rust backend, runs it out of process and compares
every value with the interpreter, on the original and on the inlined program. Java, CUDA and the
random program generator are later milestones (`SPEC.md` §9.4).

## What it checks

Every file under `corpus/` gets these tests. The test names look like `ops/i32::interp`, so you can
filter with `cargo test`.

| Test | What it checks |
| --- | --- |
| `::roundtrip` | Printing a program and parsing it back gives the same program, and printing again gives the same text. |
| `::lowering` | Lowering keeps the counts of functions, constants, hosts and so on, and gives the same result twice. It also compares the lowered program with a saved snapshot in `tests/snapshots/`. |
| `::interp` | Every `@case` runs on the interpreter and matches its `@expect` lines. It runs with memoization on and off, and both must agree. |
| `::inline` | After inlining, the interpreter gives the same cell values as before. |
| `::rust::original` | The program is compiled by the Rust backend, run on every case, and every value is compared bit for bit with the interpreter (any NaN equals any NaN). |
| `::rust::inlined` | The same for the inlined program. The expected values are still the interpreter's on the original program. |
| `::load` | Only appears if the file doesn't load or a directive is invalid. The file then gets no other tests. |

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

Environment variables (see `SPEC.md` §3.2). Only `rust` runs so far; `java` and `cuda` are accepted
names but have no trials yet:

- `TW_BACKENDS`: comma list from `rust`, `java`, `cuda`. An unknown name fails the run.
- `TW_REQUIRE`: backends whose missing toolchain should fail the run. Without it, a missing `rustc`
  (`$RUSTC` or `rustc` on the path) makes the backend trials ignored, with the reason shown.
- `TW_LARGE=1`: also run files marked `@tier(large)`.
- `TW_TIMEOUT_SECS`: watchdog per case, default 10 (ten times that for `@tier(large)`).

Builds are cached under `target/tmp/tw-cache/rust/<hash>/` (the hash covers generated sources, runtime
files, compiler flags and the `rustc` version), so a second run builds nothing. A directory under
`corpus/` is one batch, built as one crate; if that fails to compile, each file is built alone so that
one broken file doesn't hide the others. A failed build leaves its sources in `<hash>.failed/`.

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
@case "c" origin(0, 0, 0) seed(1)
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
  don't get snapshots.

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
- **Toy tables.** Real Perlin and Base3D noise are not tested here. Instead the permutation table and
  sampler are simple functions of the case's seed and the table's name (see `SPEC.md` §5.3). They are
  exact in every language, so backends can later be compared bit for bit.
- **The prelude.** The transcendentals are series approximations built from `+ - * /`, so every backend
  compiles the same arithmetic. The tests check exact points and agreement with libm to 1e-9.
- **Known backend gaps.** Integer overflow, `>>>`, float to `i8` casts and failing constants on the
  Rust backend are marked `@knownfail` (`grep -rl @knownfail corpus`); `PAPER.md` has the log.
- **What is not covered yet.** Java, the random program generator, CI and CUDA are later milestones in
  `SPEC.md` §9.4.
