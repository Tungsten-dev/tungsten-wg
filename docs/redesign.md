# Workspace redesign

Tungsten started as one crate with two pipelines. It is now a Cargo workspace of small crates, each owning one part of the compiler. This page explains the layout, the rules that keep it clean, and why it looks the way it does. For exact types and signatures, read the code.

## Overview

```
tungsten-wg (facade, repo root)
├── tungsten-rust ──┐
├── tungsten-cuda ──┼──► tungsten-interface ──► tungsten-ir
├── tungsten-java ──┤            ▲
├── tungsten-inspect┘            │
├── tungsten-optimizer ──────────┴──────────► tungsten-ir
├── tungsten-builder ─────────────────────────► tungsten-ir
└── tungsten-codegen   (no tungsten deps)
```

| Crate | Responsibility |
| --- | --- |
| `tungsten-ir` | The `OptimizedProgram` model (compute units, functions, statements, arenas). Leaf crate. |
| `tungsten-builder` | Builds `OptimizedProgram`s from Rust for frontends: `ProgramBuilder`, `FnBuilder`, typed `Expr` with collected errors, derived unit lists, `body!`/`build!`, `extern_fns!`/`record_decls!`. The replacement for the removed legacy builder. |
| `tungsten-optimizer` | Passes over the IR. Currently inlining (`perform_inline` returns `InlineStats`); `fuse` is a stub and not exported. |
| `tungsten-interface` | The backend-agnostic contract: `Interface` (names and wave schedule), `NamingRules`, `DeviceCodegen`, `OrchestrationCodegen`, `Backend`, and the pipeline that drives it: `Compiler`, `Passes`, `CompileError`. |
| `tungsten-codegen` | Language-agnostic building blocks (`Printer`, `Interned`). Leaf crate. |
| `tungsten-rust`, `tungsten-cuda`, `tungsten-java` | One backend per target language. |
| `tungsten-inspect` | A backend that renders the program and schedule for debugging (IR text, DOT/SVG graphs, diffs) instead of generating code. |
| `tungsten-wg` | The facade: re-exports the crates, gates backends behind features (`rust` is the default; `cuda`, `java`, `inspect` are opt-in), and re-exports `Compiler`, the extension traits and `registry_from`. |

## Rules

- `tungsten-ir` and `tungsten-codegen` are leaves. No cycles.
- Backends depend on `ir`, `interface` and `codegen`. They never run passes: `Compiler` runs them, and a backend only proposes them through `default_passes`. Every backend depends on the optimizer transitively, through `interface`, but must not call it (`tungsten-inspect` also reads its analyses directly). `tungsten-optimizer` and `tungsten-check` never depend on `interface`, which would close a cycle. `tests/dependency_rules.rs` checks both.
- Only the facade depends on everything.
- `tungsten-builder` depends only on `ir`, and only the facade depends on the builder. Backends and passes never see it.
- Known exception: `tungsten-cuda` and `tungsten-java` depend on `tungsten-rust` for shared helpers (type derivation, constant dependencies, auto-const specs). Moving those into a neutral crate would remove the exception.

## Using it

One backend per compile. `Compiler` is generic over the backend and returns that backend's own output type. Backend settings are extension traits on `Compiler<ThatBackend>`, defined in the backend's crate.

```rust
let program = /* build or lower an OptimizedProgram */;
let registry = /* tungsten_wg::registry_from([..]); owned, or an Arc shared between compiles */;

let out: RustOutput = Compiler::new(program, registry, RustBackend::new(RustConfig::default()))
    .passes(Passes::default().inline(Inline::All))   // optional: the backend proposes its own
    .compile()?;
```

`compile` runs: `backend.validate()`, `check::prepare`, the passes (and `check::prepare` again if they changed the program), `Interface::build` with `backend.naming_rules()`, `backend.generate`, then `deny_warnings`. Failures are a `CompileError`. See `specs/compiler.md`.

## Design decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Backend output | `generate` returns the backend's own `Output` type, and `Compiler::compile` passes it on. Backends store nothing. | A shared output enum or one central output struct: adding a backend would touch central code, and callers would have to match on variants. |
| Backend config | Per-backend config struct (`RustConfig`, `CudaConfig`, ...). | One global `CompilerConfig` with a flag per backend: it grows with every backend. |
| Arena lifetimes | `generate` takes `&self`. A backend creates its `Bump` arena inside `generate` and returns only finished `String`s. | Returning codegen objects: they borrow the program, so nothing could outlive the call. |
| Naming | Names and wave order live in `Interface`, built from `NamingRules` supplied by each backend. | Letting each backend invent names: cross-target comparison (e.g. Rust vs CUDA validation) becomes impossible. |
| Optimization | Backends propose passes (`default_passes`), the frontend overrides them on `Compiler`, and `Compiler` runs them. | Backends running their own passes: hidden behavior. Explicit passes only, before `compile`: no way for a backend to express per-target policy. |
| Facade | Root stays the `tungsten-wg` package (not a virtual workspace). | A virtual workspace: it would break the published name, the `tungsten_wg` lib name and `tungsten-mc-compile`'s dependency. |
| Legacy SPMT | Lived in its own crate and lowered to the IR through `conversion`. The crate and the `tungsten_wg::spmt` re-export were deleted in v0.3 (`specs/program-level-inputs.md` §6.8). `tungsten-mc-compile` stays pinned to v0.2 until it is ported to `tungsten-builder`. | Keeping it in the facade or in the IR crate: the IR would be coupled to a model that is being phased out. |
| Density inputs | One `density_inputs` arena on `OptimizedProgram`. A unit lists the inputs it reads, like hosts, tables and consts, so units and shared helpers can share an input. Core has no `owner`, and the checker reports `InputNotListed`. Equal inputs are not merged by the IR. | An arena per unit: no sharing, nested arenas and placeholder variables, and IR inputs wouldn't map 1:1 to core's `inputs`. |
| Legacy pipeline | The old wave orchestration and `transform_*` codegen were not migrated. `Interface::waves()` replaced wave orchestration. | Porting them: they consumed the legacy SPMT/wave DAG. The old `compile(&SPMT, &CompilerConfig)` API is removed. |
| Program construction | `tungsten-builder`: an owned `FnBuilder` per function plus `&mut ProgramBuilder`, typed `Expr` with poison-and-collect errors, lists always derived, equal inputs merged. See `specs/builder.md` §11. | `Rc<RefCell>` builders (borrow panics, `!Send`), panicking on type errors (one error per run), hand-written lists (transitive helpers become frontend bookkeeping). |
| Errors | One `thiserror` enum per crate (`BackendError`, ...); `CompileError` (in `tungsten-interface`) wraps `BackendError` and the checker's `Invalid`. User-reachable paths return errors instead of `unwrap`/`panic!`. | |

## Conventions

- New crates start with warnings enabled. Do not add `#![allow(warnings)]`; fix what shows up. (Generated Rust output does carry `#[allow(warnings)]`; that is intentional.)
- Shared dependency versions live in `[workspace.dependencies]`.
- Move files with `git mv` so history follows them.

## Open questions

- Should `tungsten-codegen` hold a shared C-like AST? CUDA, Java and OpenCL are all C-family. Do this when the second C-like backend is being written, not before.
- The wave/DOT debug output of the legacy pipeline has no direct replacement outside `tungsten-inspect`. Decide whether `inspect` is enough or `Interface` needs its own `waves_dot`.

## Not done yet

- Tests are thin. Planned: IR arena/`Ref` misuse, optimizer before/after structure, `Interface` determinism (build twice, compare; `waves_for(t)` is a subset of `waves()`), golden-file tests for Rust output, and a cross-backend test running Rust and CUDA on fixed positions.
- `tungsten-rust` has no runtime document. Its `lib.rs` points at an `INSTRUCTIONS.md` that does not exist; the runtime assumptions (`Pos3`, `Config`, `iter_3d`, samplers, ...) still need writing down.
- No `pre-workspace` tag exists, so the pre-split code is reachable only through history.
- See [TODO.md](../TODO.md) for backend work (CUDA validation, OpenCL).
