# Tungsten Worldgen Compiler

A compiler and code generator for Minecraft world generation. It takes density functions in an optimized intermediate representation (IR) and turns them into fast Rust, CUDA or Java code.

This repository only contains the backend and code generation side. The end-to-end Minecraft compiler lives in tungsten-mc-compile.

## Features

- Several targets from one source. Each compile uses one backend through `Compiler`, and returns that backend's own output type. A frontend that targets several languages builds one `Compiler` per backend.
  - Rust (tungsten-rust): CPU code, enabled by default
  - CUDA C++ (tungsten-cuda): GPU kernels, behind the cuda feature
  - Java (tungsten-java): behind the java feature
  - Inspect (tungsten-inspect): doesn't generate code, but renders the program and its schedule for debugging (IR text, DOT/SVG graphs, diffs). Behind the inspect feature.
- Built for Minecraft's vanilla worldgen (1.21.1).
- Every compile is checked: the program goes through the core checker against a registry of the externs and records it uses before any backend sees it.
- Passes like inlining run inside `compile`. A backend proposes its default passes, the frontend overrides them with `.passes(..)`, and a backend never runs passes itself.
- Generated code is split into a thread-safe `Orchestration` and one **worker** per entry. A worker belongs to one thread and owns that entry's buffers (see "Running the generated code").
- **Density mappings** (`@expand`, `@lerp`) say how a reader's cell maps into an input of another shape, so reads across grids are explicit. Every backend generates them, and lerp gives the same bits everywhere.
- **Refinement** reruns part of an entry's closure at a moved `Pos3` (marching), so a large volume can be computed slice by slice. Every backend generates refine calls.
- Backend settings are extension traits on `Compiler` (`RustCompile`, `JavaCompile`, `CudaCompile`, `InspectCompile`). A backend defined in another crate gets the same builder, passes, checking and its own extension trait.

## Workspace layout

The project is a Cargo workspace of small crates. Each one owns a single part of the compiler:

<!-- TODO: add dependency diagram (draw.io) -->

- tungsten-ir: the Program model (compute units, functions, statements, arenas). Density inputs live in one program-wide arena, and each unit lists the ones it reads.
- tungsten-builder: builds Programs from Rust, for frontends. Expressions are typed as they are built, every mistake is collected, and unit lists are derived from the bodies.
- tungsten-optimizer: passes over the IR. Right now that is inlining.
- tungsten-interface: the contract between the compiler and the backends (Interface, NamingRules, the codegen traits and the Backend trait), and the pipeline that drives it: Compiler, Passes and CompileError.
- tungsten-check: lowers programs to the core IR against a registry and runs the checker.
- tungsten-codegen: language-agnostic building blocks, like the pretty printer.
- tungsten-rust, tungsten-cuda, tungsten-java: one backend per target language.
- tungsten-inspect: the debugging backend.
- tungsten-wg: the top-level crate. It re-exports everything, switches backends on and off through features, and provides registry_from.

Backends never depend on each other, and they never run passes: Compiler runs them, and a backend only proposes them through default_passes. For the full rules and the reasoning behind them, see [docs/redesign.md](docs/redesign.md).

## Installation

Add this to your Cargo.toml:

```toml
[dependencies]
tungsten-wg = "0.2"
```

To enable more backends:

```toml
[dependencies]
tungsten-wg = { version = "0.2", features = ["cuda"] }
```

Or install from GitHub:

```toml
[dependencies]
tungsten-wg = { git = "https://github.com/Tungsten-dev/tungsten-wg" }
```

## Usage

```rust
use tungsten_wg::builder::prelude::*;
use tungsten_wg::{Compiler, Inline, Passes, RustBackend, RustCompile, RustConfig, RustOutput};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Build an Program (see crates/tungsten-builder/README.md)
let mut pb = ProgramBuilder::new();
let mut f = FnBuilder::new("height", F64);
f.ret(pos3().y().cast(F64) * 0.5);
let unit = pb.finish_unit(f);
pb.entry(unit, Grid::new((16, 1, 16)));
let program = pb.build()?; // every mistake at once, with its file and line

// The registry declares the externs and records the program uses (none here).
// Pass it owned, or as an Arc shared between compiles.
let registry = tungsten_wg::registry_from([])?;

// One backend per compile, and the output is that backend's own type
let out: RustOutput = Compiler::new(program, registry, RustBackend::new(RustConfig::default()))
    .passes(Passes::default().inline(Inline::All)) // optional: the backend proposes its own
    .density_module_name("height_fns")             // RustCompile, only on a Rust compile
    .deny_warnings()
    .compile()?;
# Ok(())
# }
```

compile validates the backend's config, checks the program, runs the passes (and checks again if they changed it), builds an Interface using the backend's naming rules, and calls generate. Failures are a CompileError: Backend (with the backend's name and error, including an invalid config), Invalid (the program failed the checker, so no backend ran), InvalidAfterPasses (a bug in a pass) or Warnings (only with deny_warnings).

To target several languages, build one Compiler per backend and clone the program for all but the last: each compile takes the program by value and is checked on its own.

```rust,ignore
let rust = Compiler::new(program.clone(), registry, RustBackend::new(RustConfig::default())).compile()?;
let cuda = Compiler::new(program, registry, CudaBackend::new(CudaConfig::default()))
    .profile(Profile::Tolerance)
    .compile()?;
```

A custom backend implements the Backend trait (with its Output type) and defines its own extension trait for Compiler<ItsBackend>. tests/custom_backend.rs is a complete example. Tools and tests that need the Interface after generating can still call Interface::build and generate directly. That path skips validation, the check, the passes and deny_warnings.

## Running the generated code

The generated code is one `Orchestration` per program and one worker per entry. The `Orchestration` is created once from the program's params, is `Send + Sync` and never changes. A worker runs exactly one entry on one thread and owns that entry's buffers: for another entry, or another thread, make another worker. Each worker has its own `<Entry>HostConsts`, holding only the hosts that entry reads.

```rust,ignore
let orch = Orchestration::new(seed);                  // shared across threads
let mut w = orch.final_density_worker();              // one per thread, not Clone
let hc = FinalDensityHostConsts { blend: 0.5, ..Default::default() };
let out: &[f64; 98304] = w.run(origin, &hc);          // the entry's whole closure
```

```java
Gen.Orchestration orch = Gen.Orchestration.create(seed);
Gen.FinalDensityWorker w = orch.finalDensityWorker();
double[] out = w.run(x, y, z, hc);                    // the pool; the first FINAL_DENSITY_LEN elements are the entry
```

In CUDA, `Orchestration::create(seed, out)` and `create_final_density_worker(out)` give a worker with its own stream. `enqueue(origin, hc)` launches, and `result()` is valid after `sync()` (see `crates/tungsten-cuda/README.md`).

A program with a `refinement` also gets one refine call per refinement on the entry's worker (`refine_<name>`, or `enqueue_refine_<name>` in CUDA). It takes the offset and reruns only what the refinement lists. Everything else stays frozen:

```rust,ignore
w.run(origin, &hc);                                   // computes the whole closure
w.refine_step(Pos3::new(0, 383, 0), &hc);             // reruns the listed instances at offset y = 383
w.refine_step(Pos3::new(0, 382, 0), &hc);
```

Call refine after `run`, and keep every offset component in `[0, march]`. These are caller preconditions with no generated checks. `docs/redesign.md` has the runtime contract the generated Rust code expects, and `use-guide.md` explains mappings and refinement.

## Development

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test
```

### Generating Documentation

```bash
cargo doc --open
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Repository

https://github.com/NiekAukes/tungsten-wg
