# PAPER.md: notes toward a paper on the testing and verification infrastructure

Status: working notes. The SPMT IR and the Minecraft lowering are covered by a paper that is already
submitted. This file is about the _new_ work: the backends, the core IR / checker / interpreter, and
the differential testing plan. It is the running log for a second paper.

Claude sessions: see [Maintaining this file](#maintaining-this-file) at the bottom.

## State of the work (as of 2026-11-01)

- `TESTING.md` is a draft spec, **not implemented**. `tungsten-testsuite` (harness, corpus, test
  runtimes) does not exist yet. `tungsten-parse` is specified but was not in the workspace tree.
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
