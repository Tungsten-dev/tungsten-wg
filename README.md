# Tungsten Worldgen Compiler

A compiler and code generator for Minecraft world generation. It takes density functions in an optimized intermediate representation (IR) and turns them into fast Rust, CUDA or Java code.

This repository only contains the backend and code generation side. The end-to-end Minecraft compiler lives in tungsten-mc-compile.

## Features

- Several targets from one source. You create the backends you want, hand them to compile as a list, and each backend keeps its own output.
  - Rust (tungsten-rust): CPU code, enabled by default
  - CUDA C++ (tungsten-cuda): GPU kernels, behind the cuda feature
  - Java (tungsten-java): behind the java feature
  - Inspect (tungsten-inspect): doesn't generate code, but renders the program and its schedule for debugging (IR text, DOT/SVG graphs, diffs). Behind the inspect feature.
- Built for Minecraft's vanilla worldgen (1.21.1).
- Optimization is up to the caller. Passes like inlining are run explicitly before compiling, never by a backend.

## Workspace layout

The project is a Cargo workspace of small crates. Each one owns a single part of the compiler:

<!-- TODO: add dependency diagram (draw.io) -->

- tungsten-ir: the OptimizedProgram model (compute units, functions, statements, arenas).
- tungsten-optimizer: passes over the IR. Right now that is inlining.
- tungsten-interface: the contract between the compiler and the backends (Interface, NamingRules, the codegen traits and the Backend trait).
- tungsten-codegen: language-agnostic building blocks, like the pretty printer.
- tungsten-rust, tungsten-cuda, tungsten-java: one backend per target language.
- tungsten-inspect: the debugging backend.
- tungsten-legacy: the old SPMT model and builder, plus the conversion that lowers it to the IR. It is kept isolated so it can be deleted later.
- tungsten-wg: the top-level crate. It re-exports everything, switches backends on and off through features, and provides compile.

Backends never depend on each other or on the optimizer. For the full rules and the reasoning behind them, see [docs/redesign.md](docs/redesign.md).

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
use tungsten_wg::{RustBackend, RustConfig, RustOutput};

// Build an OptimizedProgram, or lower a legacy SPMT program to one
let mut program = /* ... */;

// Optimization is explicit and happens before compile
tungsten_wg::optimizer::perform_inline(&mut program);

// Each backend is created on its own, with its own config
let mut rust = RustBackend::new(RustConfig::default());

// Pass as many backends as you like, e.g. &mut [&mut rust, &mut cuda]
tungsten_wg::compile(&program, &mut [&mut rust])?;

// Read the result from the backend itself
let out: &RustOutput = rust.output().unwrap();
```

For every backend, compile builds an Interface using that backend's naming rules and then calls generate. If a backend fails, you get a CompileError::Backend with the backend's name and the underlying error. Compilation stops at the first failure, and backends that already ran keep their output.

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
