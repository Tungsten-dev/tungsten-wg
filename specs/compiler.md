# `Compiler<B>`: one backend per compile, typed output

Status: design agreed. Implemented (see Implementation notes). The READMEs and `docs/redesign.md` are updated. This is a breaking change to the `tungsten-wg` facade and the `Backend` trait in
`tungsten-interface`, so it ships in v0.3. It lands right after milestone 5 of `specs/program-level-inputs.md` and before
`specs/auto-consts.md`. `tungsten-interface` gains `Compiler` and dependencies on the optimizer and the checker. It
revises `docs/redesign.md` (§8).

## 1. Summary

Today a frontend creates several backends and passes them to `compile` together. Each backend stores its result in an
`Option<Output>` that the caller reads back afterwards with `output()`:

```rust
let mut rust = RustBackend::new(RustConfig::default());
tungsten_wg::compile(&program, &mut [&mut rust])?;
let out: &RustOutput = rust.output().unwrap();
```

From v0.3, each compile uses exactly one backend and one registry, so every compile is checked. Compilation goes through
a builder that is generic over the backend, and it returns that backend's own output type. Backends can now make compile choices: a backend proposes which
optimization passes to run, and the frontend can override that. Backend-specific settings are exposed as extension
traits on `Compiler<ThatBackend>`. Each backend crate defines its own extension trait, so built-in and custom backends
work the same way.

```rust
use tungsten_wg::{Compiler, Passes, Inline};
use tungsten_wg::{CudaBackend, CudaConfig, CudaCompile};
use tungsten_wg::cuda::Profile;

let out: CudaOutput = Compiler::new(program, registry, CudaBackend::new(CudaConfig::default()))
    .passes(Passes::default().inline(Inline::All))
    .profile(Profile::Tolerance)        // CudaCompile, only on Compiler<CudaBackend>
    .block_size(256)
    .deny_warnings()
    .compile()?;
```

### Goals

- One backend per compile. The output is typed by `B::Output`, so there's no `Option` and no `unwrap`.
- Backends are stateless values: `generate(&self, ..)` returns its output and stores nothing.
- Backends can influence compilation: they choose the default passes, and the frontend overrides them on the builder.
- Backend-specific settings are type-checked. A CUDA-only setting can't be called on a Rust compile.
- Custom backends, defined in a crate outside this workspace, are as capable as built-in ones. They get the builder,
  passes, checking, `deny_warnings` and their own extension traits, with no change to this repo.
- Invalid backend configuration is reported as an error, never as a panic.
- An invalid program never reaches a backend through `Compiler`. There is no unchecked `Compiler`.
- `.deny_warnings()` works the same way for every backend.
- With default passes (`Inline::None`), `compile()` on a valid program generates exactly what the low-level path (§6)
  generates.

### Non-goals

- Compiling several backends in one call, or sharing a check result between compiles. A frontend that targets four
  backends builds four `Compiler`s (§4.6).
- A shared opt level, profile or precision enum across backends. These stay per backend (§10, D5).
- Budgeted inlining. `Inline` is `#[non_exhaustive]` so a `Budget(n)` can be added later (§11).
- Panic capture. Backends still panic on bugs. The testsuite keeps its own `catch_unwind`.
- Porting `tungsten-mc-compile`. It stays pinned (`specs/program-level-inputs.md` §6.8), and §9 records what its port
  needs.
- Changes to the testsuite beyond following the trait change (§7).

## 2. Where things live

| Item | Crate | Notes |
|---|---|---|
| `Backend` trait, `BackendError`, `Interface` | `tungsten-interface` | The trait changes (§3) |
| `Passes`, `Inline` | `tungsten-interface` | Returned by `Backend::default_passes`. Plain data |
| `Compiler<B>`, `CompileError` | `tungsten-interface` (`compiler.rs`) | Uses `Interface::build`, the optimizer and tungsten-check. Depends on no backend |
| `RustCompile`, `JavaCompile`, `CudaCompile`, `InspectCompile` | each backend crate, next to its config | Thin setters, implemented directly on `Compiler<'_, XBackend>` (§5) |
| Backend structs and configs | each backend crate | Lose `output` and `output()`. Gain `validate`, `default_passes`, and `config()`/`config_mut()` |
| Re-exports | `tungsten-wg` | `Compiler`, `CompileError` and the rest from `tungsten-interface`, plus each backend's extension trait. It adds no code of its own |

`tungsten-interface` used to be only the contract a backend implements. It is now also the pipeline that drives that
contract: validate, check, passes, `Interface::build`, generate. Both are what a backend author needs, and `Compiler`
uses every other item in the crate, so they stay together (D6).

### 2.1 Dependency graph

```text
tungsten-ir ← tungsten-optimizer ─┐
tungsten-core ← tungsten-check ───┼─ tungsten-interface ← tungsten-{rust,java,cuda,inspect} ← tungsten-wg
                                  │          ↑
tungsten-ir ──────────────────────┘   custom backends
```

No cycle: `tungsten-optimizer` depends only on `tungsten-ir`, and `tungsten-check` only on `tungsten-core` and
`tungsten-ir`. Neither may ever depend on `tungsten-interface`. A CI check (§7) guards this, since adding that edge
would close a cycle. Every backend crate already depends on `tungsten-interface`, so it can implement its own (local)
extension trait for `Compiler<'_, ItsBackend>` under the orphan rule. A custom backend does exactly the same from
outside the workspace, depending on `tungsten-interface` directly or through `tungsten-wg`.

### 2.2 The new dependency rule

`docs/redesign.md` said "backends never depend on the optimizer". That rule changes to **backends never run passes:
`Compiler` runs them, and a backend only proposes them through `default_passes`**. Every backend now depends on the
optimizer transitively, through `tungsten-interface`, but must not call it. (`tungsten-inspect` already depends on
`tungsten-optimizer` directly, for `InlineStats` in diffs, and keeps doing so.)

### 2.3 Features

There is no `check` feature. `tungsten-check` is an ordinary dependency of `tungsten-interface`, and `tungsten-wg`
always re-exports it as `check`, along with `registry_from`. Every build therefore compiles `tungsten-core` and `vstd`.

### 2.4 Facade

```rust
// tungsten-wg/src/lib.rs
pub use tungsten_interface::{Backend, BackendError, Compiler, CompileError, Passes, Inline};
pub use tungsten_check as check;
pub use tungsten_interface as interface;

#[cfg(feature = "cuda")]
pub use tungsten_cuda as cuda;
#[cfg(feature = "cuda")]
pub use tungsten_cuda::backend::{CudaBackend, CudaConfig, CudaOutput, CudaCompile};
// … the same for rust, java, inspect
```

## 3. The `Backend` trait

```rust
// tungsten-interface
pub trait Backend {
    /// What a successful compile returns, e.g. `RustOutput`.
    type Output;

    /// Short stable identifier, e.g. "rust" or "cuda". Used in errors.
    fn name(&self) -> &str;

    /// Naming rules for the current config. Computed on call, so config edits made after `new` are respected.
    fn naming_rules(&self) -> Cow<'_, NamingRules>;

    /// Checks the current config. `Compiler::compile` calls it before anything else.
    fn validate(&self) -> Result<(), BackendError> { Ok(()) }

    /// The passes this backend wants by default, given its current config.
    fn default_passes(&self) -> Passes { Passes::default() }

    /// Generates everything for `program`. Stores nothing in `self`.
    fn generate(&self, program: &OptimizedProgram, interface: &Interface)
        -> Result<Self::Output, BackendError>;

    /// Non-fatal diagnostics in an output. Read by `deny_warnings`.
    fn warnings(_output: &Self::Output) -> &[String] { &[] }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    #[error("code generation failed: {0}")]
    Codegen(String),
    #[error("invalid backend configuration: {0}")]
    InvalidConfig(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Passes {
    pub inline: Inline,
}

impl Passes {
    /// Sets the inlining pass. `Passes` is `#[non_exhaustive]`, so other crates can't use a struct expression.
    pub fn inline(self, inline: Inline) -> Self;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Inline {
    #[default]
    None,
    All,
}
```

The trait is no longer object safe as `dyn Backend` without naming `Output`. Nothing in the workspace needs `dyn`
after this change, because the multi-backend slice was its only user.

### 3.1 Per-backend changes

| Backend | `Output` | `naming_rules` | `validate` | `warnings` |
|---|---|---|---|---|
| Rust | `RustOutput` | `Borrowed(NamingRules::rust())`, held in a `static`/`LazyLock` or in the struct | none | `&[]` |
| Java | `JavaOutput` | `Owned`: `java()` with the sanitized `class_name` reserved | none (sanitizing stays a warning) | `&out.warnings` |
| CUDA | `CudaOutput` | `Owned`: `cuda()` with the namespace and kernel-prefix reservations that `new` computes today | today's `CudaConfig::validate` asserts, returned as `InvalidConfig` | `&out.warnings` |
| Inspect | `InspectOutput` | `Borrowed(inspect())` | none | `&[]` |

- `XBackend::new(config)` stays infallible and no longer validates. It just stores the config.
- Every backend gets `pub fn config(&self) -> &XConfig` and `pub fn config_mut(&mut self) -> &mut XConfig`. The
  extension traits use these.
- The `output: Option<_>` fields and the `output()` accessors are removed.
- `XConfig::to_backend` stays.
- `tungsten_inspect::inspect(&program, &config)` stays as the standalone shortcut.
- `default_passes` is `Passes::default()` (no inlining) for all four backends in v0.3 (D9).

## 4. `Compiler<B>`

```rust
// tungsten-interface/compiler.rs
pub struct Compiler<B: Backend> {
    program: OptimizedProgram,
    registry: Arc<check::Registry>,
    backend: B,
    passes: Option<Passes>,          // None = backend.default_passes()
    deny_warnings: bool,
}

impl<B: Backend> Compiler<B> {
    pub fn new(program: OptimizedProgram, registry: impl Into<Arc<check::Registry>>, backend: B) -> Self;

    pub fn passes(self, passes: Passes) -> Self;   // replaces the backend's defaults entirely
    pub fn deny_warnings(self) -> Self;

    pub fn backend(&self) -> &B;
    pub fn backend_mut(&mut self) -> &mut B;       // public API: extension traits, built-in and custom, use it

    pub fn compile(self) -> Result<B::Output, CompileError>;
}
```

### 4.1 Pipeline of `compile`

1. `backend.validate()`. On failure: `CompileError::Backend { name, source: InvalidConfig(..) }`. Nothing else runs.
2. `check::prepare(&program, &registry)`. On failure: `CompileError::Invalid`.
3. `passes = self.passes.unwrap_or_else(|| backend.default_passes())`. Then run them in a fixed order. Today the only
   pass is: `Inline::All` → `optimizer::perform_inline(&mut program)`, and `Inline::None` does nothing.
4. If at least one pass changed the program: `check::prepare` again. On failure:
   `CompileError::InvalidAfterPasses`. That means a bug in a pass, not in the frontend.
5. `Interface::build(&program, &backend.naming_rules())`.
6. `backend.generate(&program, &interface)`. On failure: `CompileError::Backend { name, source }`.
7. If `deny_warnings` is set and `B::warnings(&output)` isn't empty: `CompileError::Warnings { name, warnings }`. The
   output is dropped.
8. Return the output.

"Changed" in step 4 is `InlineStats::inlined > 0` for the inliner. Every later pass reports the same kind of
yes/no answer.

### 4.2 `CompileError`

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CompileError {
    #[error("backend `{name}` failed")]
    Backend { name: String, #[source] source: BackendError },
    #[error("program is invalid")]
    Invalid(#[source] check::Invalid),
    #[error("program became invalid after optimization passes (compiler bug)")]
    InvalidAfterPasses(#[source] check::Invalid),
    #[error("backend `{name}` produced {} warning(s) and warnings are denied", warnings.len())]
    Warnings { name: String, warnings: Vec<String> },
}
```

`CompileError` stays non-generic, so frontends can write `Result<_, CompileError>` without naming a backend.

### 4.3 Removed

- `tungsten_wg::compile(&program, &mut [&mut dyn Backend])`.
- `tungsten_wg::compile_checked`.
- There's no free-function shorthand. The builder is the only entry point (D4).

### 4.4 Program by value

`Compiler::new` takes `OptimizedProgram` by value because passes mutate it. The program isn't handed back. A frontend
that compiles one program for several backends clones it once per extra backend. That's acceptable: a clone is cheap
next to a check or codegen run, and it rules out one backend seeing another's passes.

### 4.5 Edge cases

| Case | Behaviour |
|---|---|
| `.passes()` called twice | Last call wins |
| `.passes(Passes::default())` on a backend whose default inlines | No inlining. An explicit setting always wins over the backend default |
| Ext setter makes the config invalid, e.g. `.block_size(7)` | `compile` returns `Backend { InvalidConfig }` from step 1, with no panic |
| Ext setter changes a naming field, e.g. `.namespace("x")` | Picked up, since `naming_rules()` is computed in step 5 |
| `deny_warnings` and Java sanitizes the class name | `Warnings`. That's correct, since sanitizing is a warning today |
| Passes change nothing | Checked once (step 4 is skipped) |
| Program with an extern or record the registry doesn't declare | `CompileError::Invalid`. Every extern and record a program uses must be declared |
| Backend panics in `generate` | Unwinds into the caller, as today |

### 4.6 Several targets

```rust
let rust = Compiler::new(program.clone(), registry.clone(), RustBackend::new(RustConfig::default())).compile()?;
let cuda = Compiler::new(program, registry, CudaBackend::new(cfg)).profile(Profile::Tolerance).compile()?;
```

Each target is checked on its own, so a frontend pays for one check per target. The cost on vanilla-sized programs is
unmeasured (§11).

## 5. Extension traits

Each backend crate defines its extension trait next to its config, and implements it directly and only for
`Compiler<'_, XBackend>`. Each setter writes `self.backend_mut().config_mut().field`. The setters are sugar: everything
they set can also be set through `XConfig` before `new`. Validation happens in `compile` (§4.1 step 1), not in the
setter.

```rust
// tungsten-cuda/src/backend.rs
use tungsten_interface::Compiler;

pub trait CudaCompile: Sized {
    fn block_size(self, n: u32) -> Self;
    // …
}

impl CudaCompile for Compiler<'_, CudaBackend> {
    fn block_size(mut self, n: u32) -> Self {
        self.backend_mut().config_mut().block_size = n;
        self
    }
    // …
}
```

The full sets for v0.3:

```rust
pub trait CudaCompile: Sized {
    fn profile(self, p: Profile) -> Self;
    fn block_size(self, n: u32) -> Self;
    fn host_consts_ring(self, n: u32) -> Self;
    fn debug_checks(self, on: bool) -> Self;
    fn inline_policy(self, p: InlinePolicy) -> Self;   // emitted __forceinline__, not the IR inliner
    fn namespace(self, ns: impl Into<String>) -> Self;
    fn buffer_bounds(self, b: BufferBounds) -> Self;   // added by specs/workers.md
}
pub trait JavaCompile: Sized {
    fn package(self, p: impl Into<String>) -> Self;
    fn class_name(self, c: impl Into<String>) -> Self;
    fn runtime_package(self, p: impl Into<String>) -> Self;
    fn buffer_bounds(self, b: BufferBounds) -> Self;   // added by specs/workers.md
}
pub trait RustCompile: Sized {
    fn density_module_name(self, n: impl Into<String>) -> Self;
    fn orchestration_module_name(self, n: impl Into<String>) -> Self;
    fn buffer_bounds(self, b: BufferBounds) -> Self;   // added by specs/workers.md
}
pub trait InspectCompile: Sized {
    fn views(self, v: Views) -> Self;
}
```

A new backend setting (target hardware, a CUDA SM arch, a Java opt level) touches only that backend's crate: a field on
`XConfig`, any `validate` rule, a setter on the extension trait, and, if it should affect passes, `default_passes`.
Nothing in `tungsten-interface` or `tungsten-wg` changes.

Name clashes:

- Setter names must not collide with `Compiler`'s inherent methods (`new`, `passes`, `deny_warnings`, `backend`,
  `backend_mut`, `compile`). Rust silently picks the inherent method, so each backend crate has a test
  that calls every setter and checks that the field changed (§7). Adding a new inherent method to `Compiler` is
  therefore a potentially breaking change for custom backends, and it's noted in the changelog.
- Two extension traits that share a method name (e.g. a `namespace` on both CUDA and a custom backend) don't
  conflict, because each is implemented for a different `Compiler<'_, B>`.

### 5.1 Custom backends

A crate outside the workspace writes the same thing:

```rust
// crate `my-backend`, depending on tungsten-wg (or tungsten-interface directly)
use tungsten_wg::{Backend, BackendError, Compiler, interface::{Interface, naming::NamingRules}};

pub struct MyBackend { config: MyConfig }
impl Backend for MyBackend { type Output = MyOutput; /* name, naming_rules, generate, … */ }

pub trait MyCompile: Sized { fn level(self, n: u8) -> Self; }
impl MyCompile for Compiler<'_, MyBackend> {   // allowed: MyCompile is local to this crate
    fn level(mut self, n: u8) -> Self { self.backend_mut().config.level = n; self }
}
```

The public API that custom backends rely on is: the `Backend` trait with its defaults, `BackendError`, `Interface`
and its accessors, `NamingRules`, `Passes`/`Inline`, and `Compiler::backend`/`backend_mut`. A crate can't implement a
foreign trait for `Compiler`, so the trait and its impl must live in the same crate, which is the backend's own.

## 6. Low-level path

`Interface::build(&p, &b.naming_rules())` plus `b.generate(&p, &interface)` stays public and supported. Callers that
need the `Interface` after generation use it, e.g. the testsuite driver, which reads buffer and kernel names. It skips
validation, passes, the check and `deny_warnings`. Callers that want validation call `b.validate()` themselves.

This is the only unchecked way to generate code, and an invalid program behaves as it did under the old `compile`
(for example, a cycle panics in `arrange_waves`). It's for tools and tests, not frontends.
`tungsten_inspect::inspect(&program, &config)` likewise doesn't check, so a frontend can still inspect a program the
checker rejects.

## 7. Testing

- **`Compiler` unit tests** in `tungsten-interface` (replacing `check_tests` in `tungsten-wg/src/lib.rs`), with a test `Counting`
  backend whose `validate`, `default_passes` and `warnings` can be configured:
  - an invalid program runs no `generate` (the test that exists today, ported);
  - a valid program runs `generate` exactly once and returns its output;
  - `validate` failing stops before the check and before `generate`;
  - `.passes()` overrides `default_passes`, including an override to `Inline::None`;
  - `deny_warnings`: warnings → `Warnings`, no warnings → `Ok`, and without the flag warnings → `Ok`;
  - the re-check runs only when a pass changed the program. A test pass that corrupts the program produces
    `InvalidAfterPasses` (behind `cfg(test)`, through an injectable pass hook, or by checking with a program the
    inliner provably changes plus a registry that only the inlined form violates. Pick whichever is less invasive
    when implementing).
- **Default equivalence**: for each backend, `Compiler::new(p, registry, b).compile()` gives output byte-identical to the
  low-level path on the existing backend test programs. Those programs get a registry that declares what they use.
- **Ext traits**: in each backend crate, one test calls every setter and checks the config field (this catches the
  inherent-method shadowing from §5). CUDA: `.block_size(7)` → `InvalidConfig` rather than a panic. Java:
  `.class_name("x y")` is reflected in the reserved naming rules.
- **Custom backend**: an integration test in `tungsten-wg/tests/custom_backend.rs`. Cargo builds it as a separate
  crate, so it is a real downstream crate. It defines a backend and a local extension trait implemented for
  `Compiler<'_, ItsBackend>`, then compiles with a setter, `.passes()` and `.deny_warnings()`. If this stops
  compiling, the custom-backend goal is broken.
- **Dependency rule**: a test in `tungsten-wg/tests/dependency_rules.rs` reads the workspace sources and manifests. It
  checks that no backend crate's source mentions `tungsten_optimizer::`, except `tungsten-inspect`'s `InlineStats`
  import, and that `tungsten-optimizer` and `tungsten-check` never list `tungsten-interface` as a dependency (§2.1).
  The repo has no CI, so this runs with `cargo test`.
- **Backend crates**: existing `tests.rs` follow the trait change (`let out = backend.generate(..)?`). Rewrite the CUDA
  tests that use `#[should_panic]` on invalid configs to assert `validate()` errors.
- **Testsuite**: the harnesses (`harness/{rust,java,cuda}.rs`) stay on the low-level path. They change to use the
  returned output and `backend.naming_rules()` instead of a hard-coded `NamingRules::rust()` etc. The corpus must stay
  green with no new `@knownfail`.
- `cargo test --workspace --all-features` and `cargo doc` without warnings. Update the README and docs examples so they
  compile (doctest where possible).

## 8. Documentation changes

- **`README.md`**: rewrite the Features bullet ("hand them to compile as a list") and the Usage section for `Compiler`.
  Mention the extension traits and that passes may now run inside compile.
- **`docs/redesign.md`**:
  - "Backend output" row: output is returned from `generate`/`compile` as `B::Output`, not stored.
  - "Optimization" row: replace "Explicit, before compile. Backends never run passes" with "Backends propose passes
    (`default_passes`), the frontend overrides them on `Compiler`, and `Compiler` runs them."
  - Crate table: the `tungsten-interface` row also lists `Compiler`, `Passes` and `CompileError`. Change "Backends never depend on each other or on the optimizer" to the
    §2.2 rule. The README's workspace layout section changes the same way.
  - The open question at line 75 (generic `compile<B>`): resolved by this spec.
  - The open question at line 77 (parallel backends): obsolete, since one backend runs per compile. A caller can still
    run several `Compiler`s on threads if `B: Send`.
  - The usage example at lines 39–50.
- **Facade crate docs** (`src/lib.rs`): the module comment.

## 9. For a later `tungsten-mc-compile` port

It stays on its pin, and nothing in this repo builds it. When it moves to v0.3:

- It must pass the checker first, since there is no unchecked `Compiler`. Vanilla currently fails `check::prepare` with
  1,719 errors, none about inputs (`specs/program-level-inputs.md`, milestone 3 notes). A port has to fix those.
- `compile_java` becomes `Compiler::new(optimized, registry, JavaBackend::new(cfg))
  .passes(Passes::default().inline(Inline::All)).compile()`, which removes its manual `perform_inline` and the `.output().cloned().ok_or(..)` dance.
- `do_compile` (four backends) becomes four `Compiler`s over clones of the converted program, with the CUDA one using
  `.profile(Profile::Tolerance)`.
- Its `CompileError::Internal(format!(.."{:?}"))` wrapping can match `tungsten_wg::CompileError` variants instead.

## 10. Decisions

| # | Decision | Rationale | Rejected |
|---|---|---|---|
| D1 | One backend per compile, output is `B::Output` | Typed result, no `Option`/`unwrap`, no output stored in backends | Multi-backend slice (today): untyped readback; consuming multi-compile `Compiler`: the user wants exactly one backend |
| D2 | Backend is a config-holding value with `generate(&self)` | Keeps `new(config)` and the config types. Stateless, reusable, `Send`-friendly | `impl Backend for XConfig` (deletes the backend structs); `compile::<B>(cfg)` with associated `Config` (turbofish, backend never a value) |
| D3 | Shared `BackendError`, plus a new `InvalidConfig` | No backend has a richer error. Keeps `CompileError` non-generic for frontends | Associated `type Error`: generic errors leak into frontend signatures |
| D4 | A full, consuming builder, and no free functions | One entry point, room for compile options. Consuming matches one backend per compile | Free `compile`/`compile_checked` (too little room for options); check-once handle; `&self` reuse with a cached check |
| D5 | Backend settings stay in `XConfig`, exposed via extension traits on `Compiler<XBackend>` | One place per setting; type-checked per backend; adding a setting touches only that backend's files | Separate `B::Options` type (two overlapping structs per backend); untyped option bag; shared `OptLevel`/profile (precision is backend-specific, and Rust and Java have no non-exact mode) |
| D6 | `Compiler` lives in `tungsten-interface`, which every backend already depends on. Each backend crate defines its ext trait and impls it on `Compiler<'_, ItsBackend>`. `tungsten-wg` only re-exports | One pattern for built-in and custom backends; a setting lives in one crate; no cycle (§2.1); no new crate; `Compiler` sits next to `Interface::build`, `Backend` and `Passes`, all of which it uses. The cost is that every backend depends on the optimizer transitively, so the rule becomes "backends never run passes" | A separate `tungsten-compiler` crate (its only advantage is a lighter dependency for custom backends with no setters, which is rare); `Compiler` in `tungsten-wg` with the ext traits there too (a cycle stops backends from writing their own impls, settings get split over two crates, and custom backends would follow a different pattern); traits in the backend crate with an accessor method plus a per-backend impl in `tungsten-wg` (works, but two patterns); a shared `BackendHandle` trait with blanket impls (an extra public trait only to get around the cycle) |
| D15 | The registry is a required argument of `Compiler::new`, and `tungsten-check` is always built | One checked path. Mapping and refinement validation can't be skipped by accident, and invalid programs no longer reach backends. Costs one check per target, and a frontend must pass the checker before it can compile | Optional `.registry()` behind a feature: the default path skips validation, so strict rules only hold for callers who opt in |
| D14 | Custom backends are first-class | Frontends may target their own languages without forking this repo | Closed set of backends |
| D7 | Program taken by value | Passes mutate. No hidden clone | `&` plus clone-on-pass (hidden cost); `&mut` (one compile's passes leak into the next) |
| D8 | Passes are a closed struct of knobs. Backend default, frontend override wins | Only one pass exists. A `#[non_exhaustive]` struct and enum can grow without breaking | `Vec<Box<dyn Pass>>` pipeline (overkill); a bare `.inline()` (no room to grow) |
| D9 | Every default is `Inline::None` in v0.3 | No behaviour change. Tuned defaults (CUDA `All`, Java budget) need benchmarks first | Tuned now; `All` everywhere to match mc-compile |
| D10 | Check before passes, and again after if they changed the program | Frontend errors point at frontend code. Inliner bugs show up as their own error | Before only (trusts the inliner); after only (errors in code the frontend never wrote) |
| D11 | Validation in `compile`, as an error | Ext setters change the config after `new`, so validating in `new` is too early. Panics aren't acceptable for user input | Setters only for tuning fields (validation still has to move); rebuilding the backend in each setter (keeps the panics) |
| D12 | `deny_warnings` reads `Backend::warnings(&Output)` | Output types don't change. Rust and Inspect get the empty default | `Output: Diagnostics` bound; structured `Warning` kinds (more churn, no current need) |
| D13 | Testsuite stays on the low-level path; mc-compile stays pinned | The harness needs the `Interface`; mc-compile is pinned anyway | Porting both now |

## 11. Open and deferred

- **`Inline::Budget(n)`**: budgeted inlining in `tungsten-optimizer`, aimed at Java's method-size limits and JIT
  thresholds. It needs a size metric and its own tests. With it, Java's `default_passes` can change.
- **Tuned `default_passes`** per backend (CUDA likely `All`, Rust `None`), once benchmarks exist. Changing a default
  changes generated output, so note it in the changelog.
- **Target-hardware settings** (Rust target CPU, CUDA SM arch, Java version): the framework supports them (§5). Which
  fields to add, and what they change in codegen, is per-backend work.
- **An inspect dump of the compiled program** (after passes), now that passes run inside `compile`. Today a caller
  can't see the inlined program without running the inliner itself.
- **Panic capture** as a builder option, if frontends besides the testsuite need it.
- **Unmeasured:** the cost of the mandatory check on vanilla-sized programs.
- **A testsuite mode** that compiles the corpus through `Compiler` with `Inline::All`, for end-to-end differential
  testing of backend × inliner.

## Implementation notes

- **Dependency-rule test.** §7 says no backend source may mention `tungsten_optimizer::` except `tungsten-inspect`'s
  `InlineStats` import. Inspect reads more of the optimizer than that (`explain_inline`, `GridInfo`, `InlineVerdict`,
  and others), so the test (`tests/dependency_rules.rs`) is `backends_never_run_passes`: Rust, Java and CUDA sources
  (outside `tests.rs`) must not mention `tungsten_optimizer` at all, and Inspect's must not mention `perform_inline`.
  The manifest check also covers `tungsten-core` and `tungsten-ir`.
- **`InvalidAfterPasses` test.** Uses a `#[cfg(test)]` field on `Compiler` that runs one extra pass after the real ones.
  There is no test that proves the re-check is skipped when nothing changed; it only checks that an unchanged program
  still compiles.
- **Setters.** `buffer_bounds` is not added, since it comes with `specs/workers.md`. `Passes::inline` and
  `CudaConfig::validate` are public.
- **Naming rules** are computed per call for Java and CUDA (`Cow::Owned`), and held in the struct for Rust and Inspect.
- **Testsuite.** The Rust harness now takes its rules from `backend.naming_rules()`. The CUDA harness calls
  `backend.validate()` itself, as §6 says a low-level caller should.
- **Equivalence tests** run on `BYTES` for Rust, Java and CUDA, because the other test programs use externs that an
  empty registry rejects. The backend crates gained a `tungsten-check` dev-dependency for the registry.
- **Removed.** The `check` feature of `tungsten-wg`, and its `thiserror` dependency.
- **Registry ownership.** `Compiler` owns its registry as an `Arc<Registry>`, taken as `impl Into<Arc<Registry>>`. A frontend
  passes an owned `Registry`, or shares one `Arc` between the `Compiler`s of several targets (§4.6) with `.clone()`.
  The spec's borrowed `&'r Registry` and the `Compiler<'r, B>` lifetime are gone.
