# Redesign of crate structure

I am redesigning the structure of this crate to go from a single crate to multiple crates, each responsible for a specific part of the functionality. This will help in better organization, modularity, and maintainability of the codebase.

The new structure will likely include separate crates for core logic, code generation, and any platform-specific implementations (e.g., CUDA). Each crate will have its own `Cargo.toml` and will be developed independently, allowing for clearer boundaries and easier testing.

Crates I envision include:

- `tungsten-ir` - for the intermediate representation and core logic. Based on spmt_optimze model
- `tungsten-optimizer` - for optimization passes and transformations on the intermediate representation.
- `tungsten-interface` - for defining the public interface and API for interacting with the intermediate representation and other components.
- `tungsten-codegen` - for generic code generation constructs and APIs.
- `tungsten-cuda` - for CUDA-specific code generation and platform-specific implementations.
- `tungsten-rust` - for Rust-specific code generation and platform-specific implementations.
- `tungsten-java` - for Java-specific code generation and platform-specific implementations. TODO
- `tungsten-wg` - the top-level crate that ties together the other crates and provides a unified interface for users.

The codegen will be modular. The user of the library can initialize backends for different target platforms (e.g., CUDA, Rust, Java) independently, and gives it as a list of backends to the library, which will then coordinate code generation across the specified backends. Outputs are kept in the structs for each backend, such that the results for each platform can be defined and accessed separately, allowing for flexible integration and further processing of the generated code.

# Guide

This guide turns the vision above into a concrete plan, based on how the code is wired today. It is ordered so that the workspace builds after every step.

## 1. Where we are

Today everything lives in one crate with two pipelines (see [CLAUDE.md](./CLAUDE.md)):

- **Legacy**: `spmt` → `orchestrate` → `transform_rcl` / `transform_cuda` + `transform_orchestration_*` → `rcl` / `cuda` codegen.
- **New**: `spmt` → `spmt_optimize::conversion::convert` → `OptimizedProgram` -> optimizations -> `OptimizedProgram` (after optimizations) → `interface` (`Interface`, `DeviceCodegen`, `OrchestrationCodegen`) → `transform_rcl_new`.

The redesign is built on the **new** pipeline only. The legacy pipeline is not carried over (see §6).

The actual cross-module dependencies, which determine the crate boundaries:

| Module                          | Depends on                                                                   |
| ------------------------------- | ---------------------------------------------------------------------------- |
| `spmt_optimize::model`          | `spmt::{Name, VariableType}`                                                 |
| `spmt_optimize::conversion`     | `spmt` (the old IR, as input) and `spmt_optimize::model`                     |
| `spmt_optimize::inline`         | `spmt_optimize::model`                                                       |
| `interface`                     | `spmt_optimize::model` only                                                  |
| `rcl::model`                    | `spmt::model::Interned`                                                      |
| `rcl::codegen`, `cuda::codegen` | `spmt::pretty::Printer`                                                      |
| `transform_rcl_new`             | `interface`, `spmt_optimize::model`, `rcl`, `spmt::{VariableType, Interned}` |

So the new IR is almost self-contained. The only things it still borrows from `spmt` are `Name`, `VariableType` (IR types, move into `tungsten-ir`), `Interned` (an arena reference used by the target ASTs, goes to `tungsten-codegen`), and `Printer` (a text printer, goes to `tungsten-codegen`).

The old SPMT will be phased out in the future, make it such that this is `legacy` and can be removed without affecting the new pipeline.

## 2. Target structure

```
tungsten-wg/                  (repo root = the `tungsten-wg` facade crate + [workspace])
├── Cargo.toml                (package `tungsten-wg` + [workspace] members = ["crates/*"])
├── src/lib.rs                (re-exports + `compile`/`Compiler`)
└── crates/
    ├── tungsten-ir/
    ├── tungsten-optimizer/
    ├── tungsten-interface/
    ├── tungsten-codegen/
    ├── tungsten-rust/
    ├── tungsten-cuda/
    └── tungsten-java/        (later)
```

Keep the root as the `tungsten-wg` package (not a virtual workspace) so the published name, the `tungsten_wg` lib name and `tungsten-mc-compile`'s dependency keep working. Put shared metadata in `[workspace.package]` and shared dependency versions in `[workspace.dependencies]`.

### Dependency graph

Arrows point to dependencies. There must be no cycles and no sideways dependencies between backends.

```
tungsten-wg ──► tungsten-rust ──┐
     │      ──► tungsten-cuda ──┤──► tungsten-interface ──► tungsten-ir
     │      ──► tungsten-java ──┘            ▲
     │              │  └─────────────► tungsten-codegen   (no tungsten deps)
     └────► tungsten-optimizer ───────────────┴──────────► tungsten-ir
```

Rules:

- `tungsten-ir` and `tungsten-codegen` are leaves.
- Backends depend on `ir`, `interface` and `codegen`, never on each other or on `optimizer`. Optimization is the caller's decision, not a backend's.
- Only `tungsten-wg` depends on everything.

## 3. What goes where

### `tungsten-ir`

The `OptimizedProgram` model and nothing else.

- From `src/spmt_optimize/model.rs`: `OptimizedProgram`, `ComputeUnit`, `Function`, `Statement`, `Arena`, `Ref`, `OwnerTag`, `EntryPoint`, `DensityInput`, `HostInput`, `PermutationTableInput`, `Constant`.
- From `src/spmt/model.rs` (only what the optimized model uses): `Name` and `VariableType`. Check whether the optimized model needs the whole `Expression`/`BinaryOperator` family; if so, move those too, rather than depending on the legacy model.
- Type derivation helpers (`try_derive_type`, `try_derive_binop_type` from `src/spmt/mod.rs`) if backends still need them. Prefer porting them to operate on the new `Expression`.
- Depends on: `thiserror` at most. No `bumpalo`.
- Should be clonable.

### `tungsten-optimizer`

- `src/spmt_optimize/inline.rs` (`Inliner`) and `fuse.rs` (still a stub; keep it behind a `fuse` module that is not exported until it works).
- Public API: `pub fn inline(program: &mut OptimizedProgram) -> InlineStats`, and later a `Pass` trait plus a `Pipeline` so that passes can be composed and ordered by the caller.
- Replace the `println!("performed {} inlines")` in `perform_inline` with the returned stats.
- Depends on: `tungsten-ir`.

### `tungsten-interface`

The backend-agnostic contract: `Instance`, `Interface`, `naming`, and the traits.

- From `src/interface/`: everything.
- Add the `Backend` trait (see §4) here, next to `DeviceCodegen` and `OrchestrationCodegen`.
- Depends on: `tungsten-ir`.

### `tungsten-codegen`

Generic, language-agnostic building blocks.

- `Printer` (from `src/spmt/pretty.rs`; check what in that file is SPMT-specific and leave that behind).
- `Interned<'m, T>` (pointer-equality arena reference) and the `bumpalo` dependency.
- Shared helpers that are currently duplicated across `rcl` and `cuda`, e.g. indentation, identifier sanitising and C-like expression/operator printing, when they turn up while porting.
- Depends on: `bumpalo`. No tungsten crates.

### `tungsten-rust` (replaces `rcl` + `transform_rcl_new`)

- `rcl::model` and `rcl::codegen` become `tungsten_rust::ast` and `tungsten_rust::printer`.
- `transform_rcl_new/*` becomes `tungsten_rust::device` (`RCLCodegen`) and `tungsten_rust::orchestration` (`TransformRclOrchestration`).
- The runtime assumptions listed in the `transform_rcl_new` module docs (`Pos3`, `Config`, `iter_3d`, samplers, …) should get their own document here, `crates/tungsten-rust/RUNTIME.md`. The current docs reference an `INSTRUCTIONS.md` that does not exist, so write it as part of this move.
- Public type: `RustBackend` (see §4).
- Depends on: `ir`, `interface`, `codegen`.

### `tungsten-cuda`

- `cuda::model` and `cuda::codegen` move here as the AST and printer.
- The CUDA transform and orchestration must be rewritten against `interface` (this is already the first item in [TODO.md](./TODO.md)). Do not port `transform_cuda` and `transform_orchestration_cuda`: they consume the legacy SPMT/wave DAG.
- Until then the crate can contain only the AST/printer and a `CudaBackend` that returns `BackendError::Unsupported`. Alternatively, do not create it until the rework starts.
- Validate against `tungsten-rust` output (see §7).
- Depends on: `ir`, `interface`, `codegen`.

### `tungsten-java`

Not created yet. Start by copying the layout of `tungsten-rust`.

### `tungsten-wg` (facade)

- Re-exports: `pub use tungsten_ir as ir; pub use tungsten_optimizer as optimizer; …`, plus the backends behind Cargo features (`rust` default, `cuda`, `java`) so downstream users can opt out of backends.
- The `compile` entry point (see §4).
- Depends on: all of the above, with backends optional.

## 4. Public API and backend coordination

Goals from the vision: the user initializes backends independently, hands the library a list, and reads each backend's output from its own struct.

### The trait

Put in `tungsten-interface`:

```rust
pub trait Backend {
    /// Short stable identifier, e.g. "rust" or "cuda". Used in errors.
    fn name(&self) -> &str;

    /// Generate everything for `program`. Results are stored in `self`.
    fn generate(
        &mut self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<(), BackendError>;
}
```

Design points:

- `Interface::build(&program)` runs **once** in the coordinator and is shared by all backends. That is what guarantees that names and wave order agree across targets, and it is the reason `Interface` exists.
- `generate` takes `&mut self` and returns nothing. The output lives in the backend struct, as intended. This also resolves the lifetime problem: `OrchestrationCodegen<'a>` ties `convert_program` to the program's lifetime, and today `compile()` creates the `Bump` arena as a local. Make each backend create its arena **inside** `generate`, convert, and store only the finished `String`s. Nothing borrowed outlives the call.
- Each backend has its own concrete output accessors, not a shared output enum:

```rust
let mut rust = RustBackend::new(RustConfig::default());
let mut cuda = CudaBackend::new(CudaConfig::default());

tungsten_wg::compile(&program, &mut [&mut rust, &mut cuda])?;

let rs: &RustOutput = rust.output().unwrap();   // density_module, orchestration_module, full source
let cu: &CudaOutput = cuda.output().unwrap();   // device source, host source
```

- Per-backend configuration lives with the backend (`RustConfig { density_module_name, orchestration_module_name }`), replacing the growing flag list in today's `CompilerConfig`. Adding a backend no longer touches a central config, output struct or `compile()`.

### The coordinator

In `tungsten-wg`:

```rust
pub fn compile(
    program: &OptimizedProgram,
    backends: &mut [&mut dyn Backend],
) -> Result<(), CompileError>
```

It builds the `Interface`, loops over the backends, and wraps failures in `CompileError::Backend { name, source }`. Running the backends in parallel (`std::thread::scope`) is possible later because they only share `&OptimizedProgram` and `&Interface`; that requires `Backend: Send` and the IR to be `Sync`. Not needed now.

Optimization is explicit and happens before `compile`:

```rust
let mut program = tungsten_wg::lower(&spmt)?;      // only if the legacy SPMT builder is used, see §5
tungsten_wg::optimizer::inline(&mut program);
tungsten_wg::compile(&program, &mut [&mut rust])?;
```

### Errors

Each crate gets its own `thiserror` enum (`IrError`, `OptimizeError`, `BackendError`); `tungsten-wg::CompileError` wraps them. While moving code, replace `unwrap()`/`panic!` on user-reachable paths (e.g. `convert(program).unwrap()` in the current `compile()`) with these.

## 5. The SPMT builder and `tungsten-mc-compile`

`tungsten-mc-compile` builds the **legacy** `spmt::SPMT` through `spmt::builder`, and `conversion::convert` lowers it to the new IR. That input has to go somewhere. Options:

1. **Recommended: separate crate `tungsten-spmt`** (not in the list above) containing `spmt/*` (model, builder, normalize, dag) and `spmt_optimize::conversion`. Depends on `tungsten-ir` and `tungsten-codegen`. It has one public function, `lower(&SPMT) -> Result<OptimizedProgram>`. The facade re-exports it as `tungsten_wg::spmt` so frontends keep working. When `tungsten-mc-compile` is switched to build `tungsten-ir` directly, delete this crate.

The `Printer` and `Interned` types used by `spmt` should be imported from `tungsten-codegen` in all cases.

## 6. The legacy pipeline

Not migrated: `orchestrate`, `transform_rcl`, `transform_cuda`, `transform_orchestration_{rcl,cuda,gpu}`, and `transform_naga`, `transform_spmt_to_opencl`, `opencl` (which already do not compile into the crate).

- Before starting, tag the current commit (`git tag pre-workspace`) so that the legacy code stays reachable.
- The root `tungsten-wg` 0.1.x API (`compile(&SPMT, &CompilerConfig)`) is removed. Release the new structure as **0.2.0**.
- Wave orchestration is replaced by `Interface::waves()`/`waves_for()`. The DOT/wave-graph output (`orchestrate::dot`, `generate_wave_orchestration_graph`) is the one legacy feature without a replacement: port it as `tungsten_interface::debug::waves_dot(&Interface) -> String`, or drop it deliberately.
- `src/rcl/tests/examples.rs` is not compiled today. Either revive it in `tungsten-rust` or delete it.

## 7. Migration steps

Each step should end with `cargo build --workspace` passing. Use `git mv` so history follows the files.

1. **Tag and branch.** `git tag pre-workspace`. Work on a `workspace` branch.
2. **Create the workspace.** Add `[workspace]` with `members = ["crates/*"]` and `[workspace.dependencies]` (`bumpalo`, `thiserror`, `md5`) to the root `Cargo.toml`. Keep the existing `src/` untouched. Build.
3. **`tungsten-codegen`.** Move `Printer` and `Interned`. In the old modules, replace the imports with the new crate (`pub use` shims at the old paths keep the diff small). Build.
4. **`tungsten-ir`.** Move `spmt_optimize/model.rs` and the needed `Name` and `VariableType` definitions. Leave a `pub use` in the old locations (`spmt::model::{Name, VariableType}` re-export from `tungsten-ir`) so `spmt` and its builder keep compiling. Build.
5. **`tungsten-interface`.** Move `interface/`. It only needs `tungsten-ir`. Add the `Backend` trait and `BackendError`. Build.
6. **`tungsten-optimizer`.** Move `inline.rs` and `fuse.rs`. Return stats instead of printing. Build.
7. **`tungsten-rust`.** Move `rcl/` and `transform_rcl_new/`. Implement `Backend for RustBackend` by wrapping the existing `RCLCodegen::new` + `TransformRclOrchestration::new` + `convert_program` + `output` sequence from `compile()`. Add `RustConfig` and `RustOutput`. Build.
8. **`tungsten-spmt`** (or your choice from §5). Move `spmt/` and `conversion.rs`. Build.
9. **Facade.** Rewrite `src/lib.rs` as re-exports plus `compile(&OptimizedProgram, &mut [&mut dyn Backend])`. Delete the legacy modules (§6). Build.
10. **Tests.** See below.
11. **`tungsten-cuda`**, then **`tungsten-java`**, one at a time, per [TODO.md](./TODO.md).

Do steps 3–6 as separate commits so that each can be reviewed or reverted on its own.

## 8. Testing strategy

Today the crate has effectively no tests (`cargo test` runs 0). The split makes testing cheaper, so add some before the move is finished.

- **`tungsten-ir`**: arena/`Ref` tests, including that using a `Ref` on the wrong arena panics.
- **`tungsten-optimizer`**: small hand-built `OptimizedProgram`s before/after `inline`, asserting structure.
- **`tungsten-interface`**: for a hand-built program, assert deterministic naming and wave order (build twice, compare), and that `waves_for(target)` is a subset of `waves()`.
- **`tungsten-rust`**: snapshot tests of the generated source for a few small programs (`cargo insta` or plain golden files under `tests/golden/`). Take the golden files from the current `rcl_new` output **before** moving code in step 7, so the move is proven to be behavior-preserving.
- **Cross-backend**: later, a test crate (or `tests/` in the facade) that compiles the same program to Rust and CUDA and runs both on fixed positions, comparing results. This is the validation the TODO asks for.

Also remove `#![allow(warnings)]` as each crate is created (new crates start with warnings on) and fix what shows up there rather than carrying the suppression across.

## 9. Open questions

- Which §5 option for the legacy SPMT builder? It decides whether `tungsten-spmt` exists.
- Should backends be allowed to run optimizer passes themselves (e.g. CUDA wants a different inlining policy than Rust; the existing code notes inlining is slower on CPU)? The current plan says no, but a per-backend `recommended_passes()` hint on `Backend` would be a cheap way to express it.
- Should the `Backend` trait be object-safe only (as above), or also offer a generic `compile<B: Backend>` for callers who want a typed output without calling `.output()` afterwards?
- Is `tungsten-codegen` going to hold a shared C-like AST (CUDA, Java and OpenCL are all C-family)? If so, put the effort in when the second C-like backend is written, not before.
