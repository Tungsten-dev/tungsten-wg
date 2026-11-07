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
├── tungsten-legacy ──► tungsten-ir, tungsten-codegen
└── tungsten-codegen   (no tungsten deps)
```

| Crate | Responsibility |
| --- | --- |
| `tungsten-ir` | The `OptimizedProgram` model (compute units, functions, statements, arenas). Leaf crate. |
| `tungsten-builder` | Builds `OptimizedProgram`s from Rust for frontends: `ProgramBuilder`, `FnBuilder`, typed `Expr` with collected errors, derived unit lists, `body!`/`build!`, `extern_fns!`/`record_decls!`. The replacement for the legacy builder. |
| `tungsten-optimizer` | Passes over the IR. Currently inlining (`perform_inline` returns `InlineStats`); `fuse` is a stub and not exported. |
| `tungsten-interface` | The backend-agnostic contract: `Interface` (names and wave schedule), `NamingRules`, `DeviceCodegen`, `OrchestrationCodegen`, `Backend`. |
| `tungsten-codegen` | Language-agnostic building blocks (`Printer`, `Interned`). Leaf crate. |
| `tungsten-rust`, `tungsten-cuda`, `tungsten-java` | One backend per target language. |
| `tungsten-inspect` | A backend that renders the program and schedule for debugging (IR text, DOT/SVG graphs, diffs) instead of generating code. |
| `tungsten-legacy` | The old SPMT model and builder, plus `conversion` that lowers it to the IR. Isolated so it can be deleted. |
| `tungsten-wg` | The facade: re-exports the crates, gates backends behind features (`rust` is the default; `cuda`, `java`, `inspect` are opt-in), and provides `compile`. |

## Rules

- `tungsten-ir` and `tungsten-codegen` are leaves. No cycles.
- Backends depend on `ir`, `interface` and `codegen`. They never depend on `optimizer`: optimization is the caller's decision.
- Only the facade depends on everything.
- `tungsten-builder` depends only on `ir`, and only the facade depends on the builder. Backends and passes never see it.
- Nothing outside `tungsten-legacy` may depend on it. The new pipeline must keep building when it is removed.
- Known exception: `tungsten-cuda` and `tungsten-java` depend on `tungsten-rust` for shared helpers (type derivation, constant dependencies, auto-const specs). Moving those into a neutral crate would remove the exception.

## Using it

Backends are created independently and passed to `compile` as a list. Each keeps its own output, read through its own accessor.

```rust
let mut program = /* build or lower an OptimizedProgram */;
tungsten_wg::optimizer::perform_inline(&mut program);   // optional, explicit

let mut rust = RustBackend::new(RustConfig::default());
tungsten_wg::compile(&program, &mut [&mut rust])?;
let out: &RustOutput = rust.output().unwrap();
```

`compile` builds one `Interface` per backend (using that backend's `NamingRules`), calls `generate`, and wraps failures as `CompileError::Backend { name, source }`. It stops at the first failure; earlier backends keep their output.

## Design decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Backend output | Each backend stores its own concrete output struct and exposes `output()`. | A shared output enum or one central output struct: adding a backend would touch central code, and callers would have to match on variants. |
| Backend config | Per-backend config struct (`RustConfig`, `CudaConfig`, ...). | One global `CompilerConfig` with a flag per backend: it grows with every backend. |
| Arena lifetimes | `generate` takes `&mut self` and returns `()`. A backend creates its `Bump` arena inside `generate` and stores only finished `String`s. | Returning codegen objects: they borrow the program, so nothing could outlive the call. |
| Naming | Names and wave order live in `Interface`, built from `NamingRules` supplied by each backend. | Letting each backend invent names: cross-target comparison (e.g. Rust vs CUDA validation) becomes impossible. |
| Optimization | Explicit, before `compile`. Backends never run passes. | Backends running their own passes: hidden behavior. Note that inlining is slower on CPU, so a `recommended_passes()` hint on `Backend` is the cheap way to express per-target policy if it is needed. |
| Facade | Root stays the `tungsten-wg` package (not a virtual workspace). | A virtual workspace: it would break the published name, the `tungsten_wg` lib name and `tungsten-mc-compile`'s dependency. |
| Legacy SPMT | Lives in its own crate and lowers to the IR through `conversion`. The facade re-exports it as `tungsten_wg::spmt`. | Keeping it in the facade or in the IR crate: the IR would be coupled to a model that is being phased out. Delete the crate once `tungsten-mc-compile` builds the IR directly. |
| Legacy pipeline | The old wave orchestration and `transform_*` codegen were not migrated. `Interface::waves()` replaced wave orchestration. | Porting them: they consumed the legacy SPMT/wave DAG. The old `compile(&SPMT, &CompilerConfig)` API is removed. |
| Program construction | `tungsten-builder`: an owned `FnBuilder` per function plus `&mut ProgramBuilder`, typed `Expr` with poison-and-collect errors, lists always derived, equal inputs merged. See `specs/builder.md` §11. | `Rc<RefCell>` builders (borrow panics, `!Send`), panicking on type errors (one error per run), hand-written lists (transitive helpers become frontend bookkeeping). |
| Errors | One `thiserror` enum per crate (`BackendError`, ...); the facade's `CompileError` wraps them. User-reachable paths return errors instead of `unwrap`/`panic!`. | |

## Conventions

- New crates start with warnings enabled. Do not add `#![allow(warnings)]`; fix what shows up. (Generated Rust output does carry `#[allow(warnings)]`; that is intentional.)
- Shared dependency versions live in `[workspace.dependencies]`.
- Move files with `git mv` so history follows them.

## Open questions

- Should backends be allowed to run optimizer passes (see the `recommended_passes()` idea above)?
- Should `Backend` also offer a generic `compile<B: Backend>` for callers who want a typed output without calling `.output()`?
- Should `tungsten-codegen` hold a shared C-like AST? CUDA, Java and OpenCL are all C-family. Do this when the second C-like backend is being written, not before.
- Running backends in parallel (`std::thread::scope`) is possible, since they share only `&OptimizedProgram` and `&Interface`, but needs `Backend: Send` and a `Sync` IR.
- The wave/DOT debug output of the legacy pipeline has no direct replacement outside `tungsten-inspect`. Decide whether `inspect` is enough or `Interface` needs its own `waves_dot`.

## Not done yet

- Tests are thin. Planned: IR arena/`Ref` misuse, optimizer before/after structure, `Interface` determinism (build twice, compare; `waves_for(t)` is a subset of `waves()`), golden-file tests for Rust output, and a cross-backend test running Rust and CUDA on fixed positions.
- `tungsten-rust` has no runtime document. Its `lib.rs` points at an `INSTRUCTIONS.md` that does not exist; the runtime assumptions (`Pos3`, `Config`, `iter_3d`, samplers, ...) still need writing down.
- No `pre-workspace` tag exists, so the pre-split code is reachable only through history.
- See [TODO.md](../TODO.md) for backend work (CUDA validation, OpenCL).
