# Testing policy

This page explains how Tungsten is tested and why. It's written to be read from top to bottom. The
exact rules are in [TESTING.md](../TESTING.md), which wins if the two ever disagree.

## The short version

Tungsten compiles one program to Rust, Java and CUDA. Those three outputs have to compute the same
thing, so the main policy is this:

> Every backend must produce exactly the same result as a reference interpreter, bit for bit, on
> every test case. The only exception is NaN: any NaN equals any NaN.

A test is a small text file holding a program and some inputs. The interpreter runs it and says what
the answer should be. Each backend then compiles and runs the same file, and has to match.

A known bug doesn't get hidden or loosened away. It's written down in the test file with a reason,
and the test fails as soon as the bug is fixed, so that the note gets removed.

## Where things stand

**The comparison is built; the proofs and CI are not.** The Rust, Java and CUDA backends are each
compared with the interpreter today, on a hand-written corpus. "Verified interpreter" is still a goal:
the interpreter is proven not to crash or hang, but not yet proven to compute the right answer.

| Piece | Status |
| --- | --- |
| Text format, parser and printer (`tungsten-parse`) | Working, tested |
| Lowering and registry (`tungsten-check`) | Working, tested |
| Checker and interpreter (`tungsten-core`) | Working, tested |
| Proofs that the checker and interpreter can't go out of bounds, overflow or loop forever | Run by hand with `scripts/verify.sh` |
| Proofs that the interpreter is correct, and that accepted programs can't get stuck | Not started |
| Test harness, test corpus and backend comparison (`tungsten-testsuite`) | Working: Rust, Java and CUDA are compared with the interpreter on the corpus (see [SPEC.md](../SPEC.md)) |
| Random program testing against backends | Out of scope for now |
| Backends following the required semantics | Mostly not yet: Rust and Java carry `@knownfail` notes for the gaps (see [PAPER.md](../PAPER.md)). CUDA has one |
| CI | Deferred; a workflow was written and then removed on purpose. Tests and proofs only run locally |

The corpus is hand-written. Backends are still also checked by compiling and running their output in
`tungsten-mc`, which is where real noise correctness is tested.

The rest of this page describes the policy. Anything not built yet is marked *(planned)*.

## Why test this way

**Backends are tested without a frontend.** A test doesn't need the Minecraft compiler, so a failure
points at a small program rather than at a whole world generator. Real noise correctness stays with
`tungsten-mc`, and tests here use a simple deterministic stand-in for noise tables.

**Agreement is exact, with no tolerance.** A tiny rounding difference may look harmless, but a
comparison or a `floor` can turn it into a large one. Exact matching needs no threshold, so nobody has
to argue about what "close enough" means.

**One implementation is the reference.** With three backends and no reference, a disagreement doesn't
tell you which one is wrong. The interpreter is written to be as plain as possible and to be provable,
so it's the one to trust when results differ.

## What's trusted, and why

| Part | How it earns trust |
| --- | --- |
| `tungsten-core` (checker, interpreter) | Verus proofs, plus tests. The proofs are only partial so far (see above) |
| `prim.rs` (primitive int, float and cast operations) | Trusted, and kept small on purpose. It's the only file in `tungsten-core` allowed to skip verification, and `verify.sh` refuses gaps anywhere else |
| `tungsten-check` (lowering) | Tested, not proven. It only resolves names and normalizes shapes, and leaves type checking to the checker |
| `tungsten-parse`, the harness and test runtimes | Tested, not proven |
| Hand-computed `@expect` values | Checks on the interpreter itself, by a human doing the maths |

The dividing rule is that everything inside `tungsten-core` is meant to be proven, and everything
outside it is tested.

## What backends must do

These rules apply to every backend, and the interpreter defines them. They follow Java, the language
Minecraft itself runs on, even where the target language normally does something else.

- **Integers wrap** on overflow. Shift amounts are masked to the width of the type.
- **Integer division by zero is an error**, and `MIN / -1` is `MIN`.
- **Float to int casts saturate**, and NaN becomes 0.
- **Floats are strict IEEE**, with each operation rounded to its own type. Compilers may not merge
  operations into a fused multiply-add. `-0.0` and `0.0` count as different.
- **`And` and `Or` short-circuit.** Everything else evaluates left to right.
- **Constants and inputs are evaluated eagerly**, even if nothing reads them. If one fails, the backend
  has to fail that case.

`sin`, `cos`, `exp` and friends aren't primitives. They're written in the test language itself, in a
prelude, so every backend compiles the same additions and multiplications and no result depends on a
platform's math library.

## What a test looks like

This is a complete test file:

```text
// Integer division truncates toward zero, and dividing by zero is an error.
program {
    host divisor: i32;

    fn divide() -> i32 @hosts(divisor) {
        return (7 / host(divisor));
    }

    unit top {
        main divide;
        hosts [divisor];
    }

    entry top @dims(1, 1, 1) @origin_scale(1.0, 1.0, 1.0) @position_scale(1.0, 1.0, 1.0);
}

@case "two"  origin(0, 0, 0) seed(1) host(divisor = 2)
@case "zero" origin(0, 0, 0) seed(1) host(divisor = 0)

@expect "two" top[0, 0, 0] = 3
@expect "zero" error(DivByZero)
```

- `@case` gives one set of inputs.
- `@expect` is optional. It states an answer worked out by hand, which checks the interpreter. Without
  it, the interpreter's answer is simply taken as correct.
- `@expect reject` says the checker must refuse the program. Such a file has no cases.

Keep files small, with one concern per file. A `@knownfail` hides everything else that fails in the
same scope, so a big file can hide a new bug behind an old one.

The full syntax is in [`crates/tungsten-parse/SPEC.md`](../crates/tungsten-parse/SPEC.md).

## How a test file is judged

Each file goes through these checks, and each one shows up as its own named test in `cargo test`:

1. **It loads and round-trips.** Printing the program and parsing it back gives the same program.
2. **It lowers properly.** The lowered program is compared with a stored snapshot.
3. **The interpreter runs it.** The checker has to accept it, and the results have to match any
   `@expect`.
4. **Each backend matches.** The backend compiles the program, runs it in a separate process, and is
   compared with the interpreter.
5. **Inlining changes nothing.** The file is inlined, then the interpreter and each backend run again
   and must give the same answers.

Backends are scored one case at a time:

| The interpreter says | The backend does | Result |
| --- | --- | --- |
| Values | Returns equal values | Pass |
| Values | Anything else, including a crash or timeout | Fail |
| An error | Fails in any way | Pass; which error it was isn't compared |
| An error | Returns values | Fail |
| Ran out of fuel | Not run, since backends have no fuel | Not scored |
| An error, on CUDA | Not run, since CUDA can't trap errors reliably | Not scored |
| Anything | Doesn't compile | Fail, always |

The last row matters most. Even when a case is supposed to fail, a compile error doesn't count as that
failure, or a real compiler bug could pass unnoticed in a test that only has error cases.

## Known failures

Backends aren't conformant yet, and the test suite still needs to land green. So a known gap goes in
the test file, with a reason:

```text
@knownfail(java, "min/max doesn't propagate NaN yet", case="nan", stage=inlined)
```

`case` and `stage` are optional. Use them to cover as little as possible.

- A failure the note covers is expected, and it's printed with its reason.
- A failure no note covers fails the test.
- **A covered case that passes also fails the test**, with a message to remove the note. This stops
  notes from going stale.
- `@knownfail` only applies to backends. There's no way to excuse the interpreter or inliner checks,
  because a bug there undermines the reference itself.

Add a known failure only for a real backend gap, and log the bug in [PAPER.md](../PAPER.md). That
makes the collection of notes a record of the bugs found.

## Kinds of test

| Level | What it checks | Status |
| --- | --- | --- |
| L0 | Unit tests inside each crate | Exists |
| L1 | Text round-trip | Exists, over the corpus (`tests/roundtrip.rs`) and in `tungsten-parse` |
| L2 | Lowering snapshots and invariants | Exists (`tests/lowering.rs`, snapshots in `tests/snapshots/`) |
| L3 | Interpreter against hand-computed answers | Exists (`tests/corpus.rs`) |
| L4 | Each backend against the interpreter | Exists for Rust, Java and CUDA |
| L5 | The inlined program against the original | Exists |
| L6 | Randomly generated well-typed programs | Out of scope for now |

L6 is out of scope for now; this describes the intended design. It is deliberately gentle at first. Problems on the interpreter side always fail the run, but a backend
mismatch is only reported until that backend conforms and is added to a strict list. Otherwise random
programs would keep tripping over bugs that are already known.

When a random program does fail, it's shrunk to a small example and saved in `corpus/regressions/`.
Review these before committing them, because each one fails until it's fixed or annotated.

The harness also has tests of its own, or a green run wouldn't mean anything. Its selftests run it
against deliberately broken stub backends that produce wrong values, crash or hang, to show that it
catches them.

## Running the tests

```bash
cargo test --workspace   # all tests, including the backend comparison
scripts/verify.sh        # Verus proofs for tungsten-core, downloads Verus on first use
```

If Verus's bundled Z3 won't start on your machine, `verify.sh` tells you to set `VERUS_Z3_PATH`.

You can filter by test name, and choose backends with environment variables:

| Variable | What it does |
| --- | --- |
| `TW_BACKENDS=rust,java` | Picks the backends to run. Rust and Java are the default, plus CUDA if `nvcc` and a GPU are found |
| `TW_REQUIRE=rust,java` | Fails the run if one of these toolchains is missing, rather than skipping it with a notice |
| `TW_LARGE=1` | Also runs the slow `@tier(large)` tests |
| `TW_TIMEOUT_SECS=<n>` | Per-case watchdog for a backend process (default 10) |

*(Planned)* `TW_FUZZ=<n>` will run n random programs for longer bug hunting, once L6 exists.

There is no CI yet. When it comes, it's meant to run Rust and Java with both toolchains required. CUDA
and the Verus proofs stay local, so they won't gate CI.

## What to do when

**You change a backend.** Run `cargo test --workspace` and check the output in `tungsten-mc`. A test
that now passes will fail on its stale `@knownfail`; remove the note.

**You find a backend bug.** Write the smallest test file that shows it. Fix the bug, or add a
`@knownfail` with a clear reason. Either way, log it in `PAPER.md`.

**A backend and the interpreter disagree.** Assume the backend is wrong, but check. If the interpreter
turns out to be wrong, that's the more serious bug: add an `@expect` with a hand-computed value so it
can't come back.

**You want to change the semantics.** That's a design change, not a test fix. Update `TESTING.md`, the
interpreter and the backends together, rather than adjusting a test to fit one of them.

**You add an IR feature.** Teach the checker and interpreter about it first, then add corpus files
covering its edge cases, not just the normal path.

## What this doesn't cover

- Whether generated terrain looks right, or matches vanilla noise. That's `tungsten-mc`'s job.
- `Interface` scheduling properties, apart from one regression test for merged scale keys.
- Performance.

## Further reading

| Document | Covers |
| --- | --- |
| [TESTING.md](../TESTING.md) | The normative rules: semantics, typing, harness and test levels |
| [SPEC.md](../SPEC.md) | The detailed design of `tungsten-testsuite` |
| [`crates/tungsten-core/SEMANTICS.md`](../crates/tungsten-core/SEMANTICS.md) | What a core program means |
| [`crates/tungsten-core/VERUS.md`](../crates/tungsten-core/VERUS.md) | What's proven today, and how to run it |
| [`crates/tungsten-check/README.md`](../crates/tungsten-check/README.md) | The lowering, and what it deliberately doesn't do |
| [PAPER.md](../PAPER.md) | The running log of bugs and results |
