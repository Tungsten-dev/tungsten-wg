# tungsten-cuda: CUDA C++ backend specification

Status: v1 implemented (device side, orchestration, naming); runtime still a contract only. This document is the contract for the `tungsten-cuda` crate:
what it generates, how it maps the IR, and what the (not generated) runtime must provide.

The backend is modelled on `tungsten-rust` (device side + orchestration side on top of
`tungsten-interface`) and is validated against Rust output downstream.

---

## 1. Scope

### In scope (v1)

- A new workspace crate `crates/tungsten-cuda` implementing `Backend`.
- **Device side**: `CudaCodegen` implementing `DeviceCodegen`. Emits CUDA C++ `__device__` density
  functions, helpers, constants, and one kernel per scheduled `Instance`.
- **Orchestration side**: `TransformCudaOrchestration` implementing `OrchestrationCodegen`. Emits the
  shared structs, buffer layout, and a C++ host `Orchestration` class that allocates memory,
  initializes permutation tables on the host, launches waves on a stream and reads results back.
- Two numeric **profiles** (bit-exact and tolerance), selected in `CudaConfig`.
- `NamingRules::cuda()` in `tungsten-interface`.
- The **runtime contract** (§12): names, signatures and layouts the generated code relies on.

### Out of scope (v1), but the design must not block it

- **Runtime implementation.** The runtime headers (`tw::` namespace: vector ops, `Array`, samplers,
  noise, math externs, host RNG init) are specified here as a contract only. They move into this
  crate once the intrinsic set has stabilized.
- **Wave fusion** (§9.4): several instances of one wave in a single kernel launch.
- **Rust host orchestration** driving `CudaCodegen` (e.g. via NVRTC/cudarc). The device side must
  stay independent of the C++ orchestration for that reason (§3.2).
- **A plain C++ (CPU) device codegen**: a separate `DeviceCodegen`, not a mode of this one.
- Shared-memory staging of permutation tables.
- Batching several origins per call, CUDA Graphs, multi-stream execution inside a wave.
- JNI/Panama bindings. Java uses a hand-written wrapper over the public C++ header.
- In-repo GPU tests (§15).

---

## 2. Workspace integration

- Crate: `crates/tungsten-cuda`, sources under `src/` (like `tungsten-rust` / `tungsten-java`).
- Dependencies (all via `[workspace.dependencies]`, `.workspace = true`): `tungsten-ir`,
  `tungsten-interface`, `tungsten-codegen` (text `Printer`), `tungsten-rust` (shared helpers, see
  below). Add `tungsten-cuda = { path = "crates/tungsten-cuda" }` to `[workspace.dependencies]`.
- Root `Cargo.toml`: add to `members`, add feature `cuda = ["dep:tungsten-cuda"]` (off by
  default), and an optional dependency. `src/lib.rs` re-exports it as `cuda` behind the feature.
- `compile` is not touched.
- Reused from `tungsten-rust` (as `tungsten-java` does): `types::derive_binop_type`,
  `types::derive_const_type`, `types::element_type`, `random::xoroshiro_seed`, and the argument
  derivation of `orchestration::permutation_table_specs` (identifier seeds, subident index). The
  CUDA crate maps those to its own AST types; it never emits RCL.
- Update `CLAUDE.md` (workspace table, features) when the crate lands.

### Module layout

| Module           | Role                                                                                       |
| ---------------- | ------------------------------------------------------------------------------------------ |
| `lib.rs`         | `CudaCodegen` (device side), constants for generated/runtime names, uniform parameters.    |
| `backend.rs`     | `CudaBackend`, `CudaConfig`, `CudaOutput`, `Profile`, `InlinePolicy`.                       |
| `types.rs`       | IR type → CUDA type, operator mapping, integer-semantics helpers.                          |
| `converter.rs`   | Per-function state: variable naming, type derivation, lookups (mirrors `tungsten-rust`).   |
| `expression.rs`  | Expression lowering (§7).                                                                  |
| `statement.rs`   | Statement lowering (§8).                                                                   |
| `function.rs`    | Density functions, helpers, constants, kernel bodies and wrappers (§9).                    |
| `launch.rs`      | Block shape / grid computation per instance (§9.3).                                        |
| `orchestration.rs` | Shared structs, buffer layout, host class, file assembly (§10, §11).                     |
| `model/`         | Own C++ AST (`model.rs`) and printer (`codegen.rs`) on `tungsten-codegen::Printer`.        |

The AST is new, designed for this spec (qualifiers, `extern "C"`, `__launch_bounds__`, designated
initializers, `tw::Array`, preprocessor lines). `old/cuda/` is reference only and is not ported.
The AST is owned (no `Bump`), like `tungsten-java`'s.

---

## 3. Public Rust API

### 3.1 Backend, config, output

```rust
pub enum Profile { Exact, Tolerance }            // default: Exact
pub enum InlinePolicy { Auto, Force, Never }     // default: Auto

pub struct CudaConfig {
    /// Namespace of all generated C++ code. Device code goes in `<namespace>::dev`.
    pub namespace: String,                 // "tungsten_gen"
    /// Prefix of every `extern "C"` kernel symbol. None = "<namespace>_".
    pub kernel_prefix: Option<String>,
    /// File names of the four outputs (also used in the generated #includes).
    pub public_header_name: String,        // "orchestration.h"
    pub interface_header_name: String,     // "interface.cuh"
    pub density_source_name: String,      // "density.cu"
    pub orchestration_source_name: String, // "orchestration.cu"
    /// Include path of the runtime entry header.
    pub runtime_include: String,           // "tw/tungsten_runtime.cuh"
    pub profile: Profile,
    /// Max threads per block; power of two, 32..=1024.
    pub block_size: u32,                   // 128
    /// Pinned staging slots for HostConsts uploads (>= 1).
    pub host_consts_ring: u32,             // 2
    /// Emit bounds / division checks (§13).
    pub debug_checks: bool,                // false
    pub inline_policy: InlinePolicy,
}

pub struct CudaOutput {
    pub public_header: String,        // orchestration.h
    pub interface_header: String,     // interface.cuh
    pub density_source: String,       // density.cu
    pub orchestration_source: String, // orchestration.cu
    /// Non-fatal diagnostics (DeviceCodegen::warnings + orchestration warnings).
    pub warnings: Vec<String>,
}
impl CudaOutput { pub fn files(&self) -> Vec<(String, String)>; } // (file name, contents)

pub struct CudaBackend { /* config, output: Option<CudaOutput>, naming_rules */ }
impl CudaBackend {
    pub fn new(config: CudaConfig) -> Self;
    pub fn output(&self) -> Option<&CudaOutput>;
}
impl Backend for CudaBackend { /* name() == "cuda", naming_rules() == NamingRules::cuda()
                                  plus config.namespace and kernel prefix as reserved */ }
impl CudaConfig { pub fn to_backend(self) -> CudaBackend; }
```

`CudaConfig::default()` uses the defaults shown. Invalid config values (`block_size` not a power of
two or out of range, `host_consts_ring == 0`, namespace not a valid identifier) panic in
`CudaBackend::new`.

### 3.2 Device/orchestration split

- `CudaCodegen` implements `DeviceCodegen` and knows nothing about the host class. It produces
  `density.cu` through `render(module_name, imports)`, where `imports` are `#include` targets
  (the interface header). `codegen_language()` returns `"cuda"`.
- `TransformCudaOrchestration` owns a `Box<dyn DeviceCodegen>` and drives it exactly like
  `TransformRclOrchestration`: one `generate_kernel_of(vec![inst.cu])` per scheduled unit in wave
  order, then builds the interface header, public header and host source.
- Everything the device code needs from the orchestration side (struct names, `BufferLayout`
  member names, `Ctx` layout) is either a constant in `lib.rs` or comes from the `Interface`.
  This is what lets a future Rust host drive `CudaCodegen` with its own orchestration.

### 3.3 Errors

As in the Rust and Java backends, IR the backend can't express panics with a message naming the
function or unit (e.g. `HostInput::Dynamic`, `Construct` on an unsupported type, an entry result
whose element type has no public mirror). `generate` returns `Ok(())` otherwise. Converting panics
to `BackendError` is left for a cross-backend cleanup.

---

## 4. Naming

### 4.1 `NamingRules::cuda()`

Added to `tungsten-interface/naming.rs`, with a `CUDA_RESERVED` list and `cuda_sanitize`.

Reserved:

- All C++20 keywords and alternative tokens (`alignas` … `xor_eq`, including `concept`, `requires`,
  `co_await`, `co_return`, `co_yield`, `consteval`, `constinit`, `char8_t`, `char16_t`, `char32_t`,
  `and`, `or`, `not`, `bitand`, ...), plus `NULL`, `std`, `assert`, `printf`, `main`.
- CUDA built-ins referenced unqualified: `threadIdx`, `blockIdx`, `blockDim`, `gridDim`, `warpSize`,
  `int3`, `double3`, `float3`, `make_int3`, `make_double3`, `dim3`, `cudaStream_t`, `cudaError_t`.
- Generated and runtime names: `tw`, `dev`, `Ctx`, `Config`, `PositionIterator`, `HostConsts`,
  `DeviceHostConsts`, `AutoConsts`, `BufferLayout`, `Orchestration`, `Status`, `Vec3`, `Pos3`,
  `Array`, `Impl`, `PerlinNoiseSampler`, `InterpolatedNoiseSampler`.
- Uniform parameters and kernel locals: `p`, `ctx`, `local`, `origin`, `buffers`,
  `auto_constants`, `host_consts`, `r`, `config`.
- The configured namespace and kernel prefix (added by `CudaBackend`).

Identifiers starting with `__`, or `_` + uppercase, are reserved by C++ and are never produced
(handled by `cuda_sanitize`).

### 4.2 `cuda_sanitize`

1. Replace every character that isn't ASCII alphanumeric or `_` with `_`.
2. Collapse runs of `_` into one `_`.
3. If the result is empty, starts with a digit, or starts with `_`, prefix `v`.

Collisions this creates are resolved by `unique()` in `Interface::build`, as for other backends.

### 4.3 Names not from the Interface

- **Kernel symbols** are `extern "C"` (§9.2), so namespaces don't separate them. The symbol is
  `<kernel_prefix><interface.kernel(inst)>`. This is the only derived name. Both sides compute it
  with the same function in `lib.rs` (`kernel_symbol(config, interface, inst)`).
- **Kernel bodies**: `<interface.kernel(inst)>_body`, also via one function in `lib.rs`. It lives
  in the same namespace as kernels, and `Interface` guarantees kernel names are unique, so the
  suffix can't collide with another kernel. It could collide with a helper that is literally
  named `<kernel>_body`. To rule that out, the backend checks the generated set and panics on a
  collision. Proper fix: an `Interface::kernel_body` name later.
- **Entry methods**: `enqueue_<unit_base_name>` / `result_<unit_base_name>` / `<unit_base_name>_len`,
  made unique within the class like `orchestrate_*` in the Rust backend, but computed with the
  CUDA naming rules (the Rust orchestration hard-codes `NamingRules::rust()`; don't copy that).

---

## 5. Generated files

| File               | Compiled by          | Contents                                                                                     |
| ------------------ | -------------------- | -------------------------------------------------------------------------------------------- |
| `orchestration.h`  | any C++17 host compiler | Public API: `Vec3`, `Pos3`, public `HostConsts`, `Status`, `Orchestration` (pimpl). No CUDA headers. |
| `interface.cuh`    | nvcc / NVRTC         | `dev::DeviceHostConsts`, `dev::AutoConsts`, `dev::BufferLayout`, `dev::Ctx`, kernel declarations. |
| `density.cu`       | nvcc / NVRTC         | Constants, forward declarations, density functions, helpers, kernel bodies, kernel wrappers. |
| `orchestration.cu` | nvcc (host code)     | `Orchestration::Impl`, `create`, `enqueue_*`, `sync`, `query`, `result_*`, layout `static_assert`s. |

Include graph:

```
orchestration.h   <- (consumer code, JNI wrapper)
interface.cuh     -> runtime_include (tw/tungsten_runtime.cuh)
density.cu        -> profile define, runtime_include, profile header, interface.cuh
orchestration.cu  -> orchestration.h, interface.cuh, <cuda_runtime.h>, <cstring>, <memory>
```

`density.cu` and `orchestration.cu` are separate translation units. Launching a `__global__`
defined in another TU works without relocatable device code (`-rdc`), because all `__device__`
code lives in `density.cu`.

### 5.1 NVRTC compatibility

`interface.cuh`, `density.cu` and the device parts of the runtime must compile under NVRTC
(`-std=c++20`):

- No host standard headers (`<cstdint>`, `<cmath>`, `<cstdio>`, ...) in device code. Types are
  spelled `int`, `long long`, `unsigned`, `unsigned long long`, `float`, `double`, `bool`
  (`long long` is 64-bit on every CUDA platform, including Windows).
- `orchestration.h` and `orchestration.cu` are host-only and may use the standard library.
- Kernels are `extern "C"` so a future host can find them by name with `cuModuleGetFunction`.

### 5.2 Toolchain floor

CUDA 12+, C++20 (`-std=c++20`), compute capability `sm_70`+. The public header itself only
needs C++17.

---

## 6. Type mapping

| IR `VariableType`          | Device C++ type                          | Public mirror (`orchestration.h`) |
| -------------------------- | ---------------------------------------- | --------------------------------- |
| `F32` / `F64`              | `float` / `double`                       | same                              |
| `I8`                       | `signed char`                            | same                              |
| `I32` / `I64`              | `int` / `long long`                      | same                              |
| `Bool`                     | `bool`                                   | `bool`                            |
| `Vec3`                     | `double3`                                | `Vec3 { double x, y, z; }`        |
| `Pos3`                     | `int3`                                   | `Pos3 { int x, y, z; }`           |
| `Array(T, N)`              | `tw::Array<T', N>`                       | `T'' name[N]` (struct field)      |
| `PermutationTable`         | `const tw::PerlinNoiseSampler*`          | n/a (panic)                       |
| `InterpolatedNoiseSampler` | `const tw::InterpolatedNoiseSampler*`    | n/a (panic)                       |
| `Extern(name)`             | `tw::<cuda_sanitize(name)>`              | n/a (panic)                       |
| `DensityInput`             | never converted (read through buffers)   | n/a                               |

- `double3` is 24 bytes, 8-aligned. `int3` is 12 bytes, 4-aligned. Both are layout-compatible
  with the public mirrors. `orchestration.cu` `static_assert`s `sizeof`, `alignof` and `offsetof`
  of every mirrored struct and field.
- Owned AutoConsts field types are `tw::PerlinNoiseSampler` / `tw::InterpolatedNoiseSampler`
  (the table types without the pointer).

---

## 7. Expression lowering

Lowering follows `tungsten-rust/src/expression.rs` and adds the C++-specific rules below.
Operands keep the Rust backend's cast insertion: `convert_expression_to_type` for assignments,
call arguments, and the arguments of `Construct`/array literals. Braced initialization rejects
narrowing, so every element is cast to the element type.

### 7.1 References to program data

| IR                                | CUDA                                                                                  |
| --------------------------------- | ------------------------------------------------------------------------------------- |
| `Constant(r)`                     | `interface.constant(r)` (namespace-scope, §9.6)                                         |
| `Variable(r)`                     | the local's name (unique per function, `fresh_name` as in Rust)                       |
| `HostInput(r)`                    | `ctx.host_consts-><interface.host_field(r)>`                                           |
| `PermutationTable(r)`             | `&ctx.auto_constants-><interface.auto_const_field(r)>`                                 |
| `DensityVariable(input, idx)`     | `tw::buffer<T>(ctx.buffers, BufferLayout::<buffer_field>)[i]`; `T` = producer's return type |
| `FunctionCall { f, params }`      | `<interface.helper(f)>(p, ctx, params cast to the callee's parameter types...)`        |
| `ExternCall { name, params, .. }` | `tw::<cuda_sanitize(name)>(params cast to parameter_types...)`                        |

For `DensityVariable`, the index `i` is the explicit index cast to `int`. With no explicit index
it is `0` when the input's dimensions are `(1,1,1)`, and `tw::index(p)` otherwise, matching Rust.

Extern calls are always qualified with `tw::`, so they never collide with generated helpers.
Extern arguments are cast to every declared parameter type (like Java), not only to
`f32`/`i32` (like Rust).

### 7.2 Integer semantics (Java semantics, both profiles)

Java wraps on overflow. In C++ signed overflow is UB and NVVM exploits it. On `I8` and `I32` (`U` =
`unsigned`) and `I64` (`U` = `unsigned long long`):

| IR op                | Emitted                                                     |
| -------------------- | ----------------------------------------------------------- |
| `Add`/`Subtract`/`Multiply` | `(T)((U)(a) op (U)(b))`                              |
| `Negate`             | `(T)(0 - (U)(a))`  (literals: folded, see §7.6)             |
| `ShiftLeft`          | `(T)((U)(a) << (b & M))`                                    |
| `ShiftRight`         | `(a >> (b & M))` (arithmetic; defined in C++20)            |
| `LogicalShiftRight`  | `(T)((U)(a) >> (b & M))`                                    |
| `Divide`             | `a / b` (plain; see §13 for checks)                          |
| `BitAnd`/`BitOr`/`BitXor` | plain                                                  |
| `Not` on integer     | `~a`                                                        |
| `Not` on bool        | `!a`                                                        |

- `M` is 31 for `I8` and `I32` and 63 for `I64`. When the shift count is an integer literal, the mask is
  folded at codegen time (`x << 33` on `I32` emits `x << 1`) and no `&` is emitted.
- The operation's width comes from the derived operand types. If either side is `I64` the 64-bit
  form is used. If neither type can be derived, the plain operator is emitted and the converter
  records a warning. Otherwise `I32` wins over `I8`.
- `I8` (`T` = `signed char`) is computed in 32 bits, as C++ promotes it, and every integer result
  is cast back to `signed char`: the forms above, plus `(T)(a / b)`, `(T)(a >> (b & M))`,
  `(T)(a & b)` (and `|`, `^`) and `(T)(~a)`. `I8` literals are emitted as `(signed char)v`.
- `I32 ± Bool` (allowed by `derive_binop_type`) works without casts: `bool` converts to `unsigned`.
- Converting back from unsigned to signed is modular in C++20.
- **Known divergence from the Rust backend:** `tungsten-rust` maps `LogicalShiftRight` to `>>` on
  signed values (arithmetic). CUDA follows Java (`>>>`). Differential validation will disagree on
  negative operands until the Rust backend is fixed.
- **Known divergence from the Rust backend:** shifts of an `I8` by 8 or more overflow in Rust
  (panic in debug, count masked by 7 in release). CUDA and Java mask by 31 on the promoted `int`.

### 7.3 Casts (`ExplicitCast` and inserted casts)

| From → To                        | Emitted                                                                 |
| -------------------------------- | ----------------------------------------------------------------------- |
| `F64`/`F32` → `I8`               | `tw::f2i8(x)`: saturating to [-128, 127], NaN → 0 (Rust `as i8`)     |
| `F64`/`F32` → `I32`              | `tw::f2i32(x)`: saturating, NaN → 0 (Java/Rust `as` semantics)       |
| `F64`/`F32` → `I64`              | `tw::f2i64(x)`: saturating, NaN → 0                                  |
| integer → integer, int → float, `F64` ↔ `F32`, `Bool` → int | `(T)(x)`                          |

`tw::f2i8`, `tw::f2i32` and `tw::f2i64` are runtime functions, `__host__ __device__ constexpr`,
overloaded for `float` and `double`. `tw::f2i8` is `tw::f2i32` clamped to [-128, 127] (Java
`(byte)` would wrap instead). In device code the runtime must lower them to `__double2int_rz`,
`__double2ll_rz`, `__float2int_rz` and `__float2ll_rz`: PTX `cvt.rzi` saturates and maps NaN to 0.
For constant evaluation the runtime uses a portable constexpr implementation.
(Emitting the intrinsics directly is not possible because constant initializers are evaluated at
compile time on the host, where the intrinsics don't exist.)

### 7.4 Vec3 / Pos3

- Arithmetic (`Vec3 ± Vec3`, `Vec3 ± Pos3`, `Pos3 ± Vec3`, scalar forms as the IR produces them)
  is emitted with plain operators. The runtime defines the global `operator+`, `operator-`,
  `operator*`, `operator/` for `double3`/`int3` (§12.2).
- `Construct { Vec3, [x, y, z] }` → `double3{(double)x, (double)y, (double)z}`. `Pos3` →
  `int3{...}`. These are aggregate initializers rather than `make_double3`, so they are usable in
  `constexpr` constants.
- `Field` with `known_idnex` or the names `x`/`y`/`z` → `.x`/`.y`/`.z`.
- `ArrayAccess` on a `Vec3`/`Pos3` value → `tw::get(v, i)` (runtime, returns `double` per
  `element_type`).

### 7.5 Arrays and aggregates

- `ArrayLiteral([a, b, ...])` → `tw::Array<T, N>{{(T)a, (T)b, ...}}` with `T` from the derived
  element type. An empty literal panics (the type can't be derived).
- `ArrayAccess` on `tw::Array` → `a[(int)i]`.
- `ConstructExtern { t, [(name, e), ...] }` → `tw::T{.name = e, ...}` (C++20 designated
  initializers). The runtime must declare the fields in the same order the IR lists them. The
  compiler checks names and order, so drift is a compile error, not a silent bug.
- `Construct { Extern(name), args }` → `tw::<name>::make(args...)` (the runtime counterpart of
  Rust's `Name::new`).
- `Construct` on any other type panics.

### 7.6 Literals

| IR            | Emitted                                                                                     |
| ------------- | ------------------------------------------------------------------------------------------- |
| `Double(v)`   | Shortest round-trip decimal with `.` or exponent (`1.0`, `0.1`, `1e-7`). `-0.0` stays `-0.0`. NaN → `TW_NAN_F64`, ±∞ → `TW_INF_F64` / `(-TW_INF_F64)`. |
| `Float(v)`    | Same, with an `f` suffix (`1.0f`, `1e-7f`); `TW_NAN_F32`, `TW_INF_F32`.                      |
| `Int(v)`      | Decimal. `i32::MIN` → `(-2147483647 - 1)`.                                                   |
| `Long(v)`     | Decimal with `LL`. `i64::MIN` → `(-9223372036854775807LL - 1)`.                             |
| `Bool(v)`     | `true` / `false`                                                                            |

Negative literals are parenthesized wherever they are operands (`a - (-1)`, never `a--1`). The
printer parenthesizes by precedence, not by rendering everything with parentheses.

### 7.7 `UnaryOperator::Ref(mutable)`

`Ref(false)` → `&x` (`const T*`), `Ref(true)` → `&x` (`T*`). Runtime externs take pointers where
Rust takes `&`/`&mut`.

If the operand is not an lvalue (a variable, field, array element or buffer element), the
temporary is materialized inside the expression: `&tw::lval(expr)`, where `tw::lval(T&& x) -> T&`
returns its argument. A temporary bound to a function argument lives until the end of the full
expression, so the pointer is valid for the call.

This is how the agreed "hoist temporaries" approach is implemented. Hoisting into pre-statements,
as Java does, would break `While` conditions (evaluated once instead of on every iteration) and the
right operand of `And`/`Or` (it would run even when short-circuited). Materializing inside the
expression has neither problem. For `Ref(true)`, writes through the pointer go to the temporary and
are discarded, the same as `&mut (a + b)` in Rust.

`Ref` on a `PermutationTable`/sampler-typed operand (already a pointer) is a no-op.

### 7.8 Other operators

Comparisons and `And`/`Or` (→ `&&`/`||`) are plain. `Field` on a pointer-typed base uses `->`.

---

## 8. Statement lowering

| IR                          | CUDA                                                                                 |
| --------------------------- | ------------------------------------------------------------------------------------ |
| `Assign { target, value }`  | `x = value;` with `value` cast to the variable's type when the derived type differs  |
| `Return(e)`                 | `return e;`                                                                          |
| `If`                        | `if (c) { ... } else { ... }` (no `else` when empty)                                  |
| `While`                     | `while (c) { ... }`                                                                  |
| `Repeat { count, body }`    | `for (int <fresh "repeat_counter"> = 0; ... < count; ++...) { body }` (count must fit in `int`) |
| `Break`                     | `break;`                                                                             |

Locals are declared at the top of the function as `T name{};`, like Rust's hoisted declarations.
The IR guarantees definite assignment; value-initialization only keeps the code free of UB and
compiler warnings. Variables named `origin`, `origin_scale`, `position_scale` and `pos3`
(`Name::Named`) are bound in the prologue as in `tungsten-rust::function::default_binding`:
`ctx.config.origin`, `ctx.config.origin_scale`, `ctx.config.position_scale`, `p.pos`.
`DensityInput`-typed variables are never declared.

---

## 9. Device code (`density.cu`)

Layout of `density.cu`, all inside `namespace <namespace>::dev`:

1. Header comment: generator, profile, required compiler flags (§14).
2. `#define TUNGSTEN_PROFILE_EXACT 1` or `#define TUNGSTEN_PROFILE_TOLERANCE 1`, then
   `#include` of `runtime_include` and the profile header (in that order: the profile's externs
   use runtime types), then `interface.cuh`. The profile header sits next to `runtime_include`.
3. Constants in dependency order (§9.6).
4. Forward declarations of every density function and helper.
5. Density functions and helpers (any order).
6. Kernel bodies, then kernel wrappers.

Forward declarations are required because C++ needs declaration before use. The Rust backend
never had to care about order.

### 9.1 Uniform parameters and `Ctx`

Every density function and helper takes `p` and one context:

```cpp
struct Ctx {                                       // interface.cuh
    tw::Config config;                             // per instance, built from literals
    char* buffers;                                 // slab base (§10.1)
    const AutoConsts* __restrict__ auto_constants;
    const DeviceHostConsts* __restrict__ host_consts;
};

__device__ R density_fn_d5x49x5(tw::PositionIterator p, const Ctx& ctx);
__device__ R some_helper(tw::PositionIterator p, const Ctx& ctx, double a, int b);
```

`ctx` replaces Rust's `config, buffers, auto_constants, host_consts`. This keeps the argument count
through deep helper chains down. `FunctionConverter`'s `UniformVars` becomes `{ p, ctx }`.

Function qualifier by `InlinePolicy`: `Auto` → `__device__`; `Force` → `__device__ __forceinline__`;
`Never` → `__device__ __noinline__`. The default lets nvcc decide.

### 9.2 Kernels: body + thin wrapper

Each scheduled `Instance` gets a body and a wrapper. The body holds all per-instance logic, so a
future fused kernel can call several bodies without changing them.

```cpp
// Body: bounds check, Ctx, position, call, store. Constants are per instance.
__device__ void <kernel>_body(int3 local, double3 origin, char* buffers,
                              const AutoConsts* __restrict__ auto_constants,
                              const DeviceHostConsts* __restrict__ host_consts) {
    if (local.x >= DX || local.y >= DY || local.z >= DZ) return;
    const Ctx ctx{tw::Config{origin, double3{OSX, OSY, OSZ}, double3{PSX, PSY, PSZ},
                             int3{DX, DY, DZ}},
                  buffers, auto_constants, host_consts};
    const tw::PositionIterator p = tw::make_position(ctx.config, local);
    const R r = <density_fn>(p, ctx);
    tw::buffer<R>(buffers, BufferLayout::<buffer_field>)[tw::index(p)] = r;
}

// Wrapper: 1D launch; decodes the linear id into `local`, x fastest.
extern "C" __global__ void __launch_bounds__(T)
<kernel_symbol>(double3 origin, char* buffers,
                const AutoConsts* __restrict__ auto_constants,
                const DeviceHostConsts* __restrict__ host_consts) {
    const int i = (int)(blockIdx.x * blockDim.x + threadIdx.x);
    const int yz = i / DX;
    const int z = yz / DY;
    <kernel>_body(int3{i - yz * DX, yz - z * DY, z},
                  origin, buffers, auto_constants, host_consts);
}
```

- `OS*`/`PS*` are `inst.scaled_origin()` / `inst.scaled_position()` as `double` literals, and `D*`
  are the dimensions, as in the Rust kernel's `Config`.
- `extern "C"` names live in the global symbol namespace even inside `namespace dev`. That is why
  they carry `kernel_prefix` (§4.3). `interface.cuh` declares them so `orchestration.cu` can launch
  them with `<<<>>>`.
- **Index order is a hard contract:** `x` fastest, then `y`, then `z`:
  `idx = x + DX * (y + DY * z)`. It is defined once in the runtime (`tw::index`,
  `tw::make_position`) and must match the Rust runtime's `index()` / `iter_3d`. `threadIdx.x` maps
  to `x`, so consecutive threads store to consecutive addresses (coalesced).
- Ids past the element count decode to `z >= DZ`, so the body's bounds check covers them.
- An instance with more than `i32::MAX` elements panics.
- `(1,1,1)` instances are ordinary kernels (one block, one thread). Fusion will remove that
  overhead later.

### 9.3 Launch shape (`launch.rs`)

Every instance is a 1D launch, computed from its element count `N = dx·dy·dz` and
`B = block_size`:

```
T = min(N, B)          // threads per block
blocks = ceil(N / T)
```

A 3D block shape would pad narrow dimensions to a power of two (a 5×49×5 instance as (8,16,1)
blocks runs 65 warps where 39 do); the FP64 units pay for those dead lanes. The rule is
deterministic, so both sides compute the same shape from the same function. `__launch_bounds__(T)`
is emitted, and the host launches `<<<blocks, T>>>`.

### 9.4 Wave fusion (future; the design must allow it)

Fusion only merges instances **within one wave**. Instances of a wave are independent by
construction (`arrange_waves`), so no grid-wide sync is needed. Waves still run as separate,
stream-ordered launches.

Plan, which needs no changes to bodies:

- `generate_kernel_of` (or a CUDA-specific entry point taking `Vec<Instance>`) emits one
  `extern "C" __global__` per fused group.
- The fused kernel uses a 1D grid with `T` threads per block, where `T` is the largest member's
  thread count. Each member `m` owns a contiguous `blockIdx.x` range `[start_m, start_m + blocks_m)`.
  Inside its range the kernel computes the member-local linear id and decodes it into `local`
  exactly like the single-instance wrapper (§9.2), then calls `m`'s `_body`.
- Ranges and member shapes are compile-time constants, so dispatch is a chain of comparisons on
  `blockIdx.x`.
- The host launches one fused kernel per group instead of one per instance. `waves_for` pruning
  still applies per entry, so fused groups are formed per entry, or a fused group is launched only
  if all its members are needed.

Keep `launch.rs` and the `_body` signature stable with this in mind.

### 9.5 Density functions and helpers

Converted like `tungsten-rust::function::{convert_density, convert_helper}`: one density
function per `(unit, dimensions)` (`interface.density_fn`), helpers once each
(`interface.helper`), plus the helpers and constants they reach. Generation is idempotent per item
(`done_*` sets, as in `RCLCodegen`).

### 9.6 Constants

- Emitted at namespace scope in **dependency order**: a topological sort over `const_refs`, ties
  broken by the `Interface`'s constant order so output is deterministic. Cycles panic. C++ needs
  this order; Rust didn't.
- Scalars (`F32`, `F64`, `I32`, `I64`, `Bool`): `constexpr T name = init;`.
- Aggregates (`tw::Array`, `double3`, `int3`, `tw::<Extern>`): `__device__ constexpr T name = init;`.
  A plain host `constexpr` aggregate can't be indexed dynamically from device code.
- Initializers use the same lowering as expressions (§7) and must be constant expressions. The
  runtime contract therefore requires constexpr-constructible extern types, `constexpr`
  `tw::f2i8`/`tw::f2i32`/`tw::f2i64`, and constexpr `operator`s for vector types.
- **Verify during implementation:** `__device__ constexpr` aggregates whose initializers reference
  other such constants, under both nvcc and NVRTC. Fallback: emit aggregates as
  `__device__ const` with the initializer fully folded by the backend.

---

## 10. Device memory

### 10.1 Buffer slab

All instance buffers live in **one `cudaMalloc` allocation per `Orchestration`**. Byte offsets are
compile-time constants in `interface.cuh`. The offsets are computed by the C++ compiler, not by
Rust, because element sizes of runtime types (`tw::Array`, extern types) are only known in C++:

```cpp
struct BufferLayout {
    static constexpr unsigned long long <buf0> = 0;
    static constexpr unsigned long long <buf1> =
        tw::align_up(<buf0> + N0 * sizeof(T0), 256ull);
    ...
    static constexpr unsigned long long total_bytes = tw::align_up(<last> + Nk * sizeof(Tk), 256ull);
};
```

- Member names are `interface.buffer_field(inst)`, in `interface.instances()` order. Every
  buffer is 256-byte aligned.
- Element type = the unit's return type, mapped by §6.
- `tw::buffer<T>(char* base, unsigned long long offset) -> T*` is a runtime helper (a
  `reinterpret_cast`).
- After allocation, the slab is zeroed with `cudaMemsetAsync`, matching Rust's zeroed
  `make_boxed::<InternalBuffers>()`.

### 10.2 AutoConsts (permutation tables / samplers)

- `struct AutoConsts` in `interface.cuh`: one field per permutation table,
  `interface.auto_const_field(r)`, type `tw::PerlinNoiseSampler` or `tw::InterpolatedNoiseSampler`.
  It must be flat and pointer-free, with identical host and device layout (§12.4).
- Initialized **on the host** in `create`. Arguments as in the Rust `permutation_table_specs`:
  `tw::make_permutation_table(seed, id_lo, id_hi, subident_index, sub_lo, sub_hi)` /
  `tw::make_base3d_perm_table(seed)`. The seed halves come from `xoroshiro_seed` and are emitted
  as `(long long)0x...ULL` hex literals with the ident in a comment.
- AutoConsts can be large, so it is built in heap memory (`std::unique_ptr<AutoConsts>`), never
  on the stack. It is copied once with `cudaMemcpy` into its own `cudaMalloc` block.
- Kernels read it through `const AutoConsts* __restrict__` (the read-only cache path). It is
  per `Orchestration`, so several seeds can coexist in one process.

### 10.3 HostConsts

- Public `HostConsts` (in `orchestration.h`) uses the public mirror types. The device-side
  `DeviceHostConsts` (in `interface.cuh`) has the same fields in the same order with device types.
  `orchestration.cu` `static_assert`s that they are layout-identical, so upload is a byte copy.
- Field names: `interface.host_field(r)`, for static host inputs only. `HostInput::Dynamic`
  panics. A host input whose type has no public mirror (§6) panics.
- Per call, the struct is uploaded into **one device `DeviceHostConsts` per `Orchestration`**
  through a ring of `host_consts_ring` pinned staging slots (§11.3).
- If the program has no static host inputs, both structs are empty, the `enqueue_*` methods keep
  the `host_consts` parameter for a uniform API, and no copy is issued.

---

## 11. Host orchestration

### 11.1 Public header (`orchestration.h`)

```cpp
#pragma once
#include <memory>

namespace tungsten_gen {

struct Vec3 { double x, y, z; };
struct Pos3 { int x, y, z; };

struct HostConsts { /* one field per static host input, public mirror types */ };

struct Status {
    int code = 0;                       // cudaError_t value; 0 = success
    bool ok() const { return code == 0; }
    const char* message() const;        // cudaGetErrorString, implemented in orchestration.cu
};

class Orchestration {
public:
    static Status create(long long seed, std::unique_ptr<Orchestration>& out);
    ~Orchestration();
    Orchestration(const Orchestration&) = delete;
    Orchestration& operator=(const Orchestration&) = delete;

    // One pair per entry point:
    Status enqueue_<entry>(Vec3 origin, const HostConsts& host_consts);
    const double* result_<entry>() const;   // N > 1 elements; T = public element type
    double result_<entry>() const;          // when the entry has 1 element
    static constexpr int <entry>_len = N;
    static constexpr int <entry>_dims[3] = {DX, DY, DZ};

    Status sync();    // waits for all queued work
    Status query();   // non-blocking: success, cudaErrorNotReady (as code), or an error

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
    explicit Orchestration(std::unique_ptr<Impl>);
};

} // namespace tungsten_gen
```

The header has no CUDA includes and no CUDA types, so it compiles with g++/clang/MSVC. A
standalone C++ tool or the JNI wrapper includes only this file and links the nvcc-built objects.
Result element types without a public mirror panic at generation time.

### 11.2 Lifecycle

`create(seed, out)`:

1. Uses the **current device** (the caller picks it with `cudaSetDevice` beforehand).
2. `cudaStreamCreateWithFlags(&stream, cudaStreamNonBlocking)`.
3. `cudaMalloc` the slab (`BufferLayout::total_bytes`), then `cudaMemsetAsync(0)` on the stream.
4. Builds `AutoConsts` on the host (§10.2), then `cudaMalloc` + `cudaMemcpy`.
5. `cudaMalloc` the device `DeviceHostConsts`. Allocates `host_consts_ring` pinned staging
   slots (`cudaHostAlloc`), each with a `cudaEvent_t` (`cudaEventDisableTiming`).
6. `cudaHostAlloc` one pinned result buffer per entry (`<entry>_len * sizeof(T)`).
7. `cudaStreamSynchronize`, then sets `out`.

If any step fails, `Impl`'s destructor releases whatever was acquired and `out` is left null.

Destructor: `cudaStreamSynchronize(stream)`, then frees everything in reverse order. Errors are
ignored (it is a destructor).

### 11.3 `enqueue_<entry>(origin, host_consts)`

1. `slot = ring[next++ % host_consts_ring]`. `cudaEventSynchronize(slot.event)`: this blocks only
   if the H2D that last read this pinned slot hasn't finished.
2. `memcpy` the public `HostConsts` into the slot.
   `cudaMemcpyAsync(device_host_consts, slot, sizeof, H2D, stream)`, then
   `cudaEventRecord(slot.event, stream)`.
3. For each wave of `interface.waves_for(entry)`, for each instance in order: launch
   `<kernel_symbol><<<grid, block, 0, stream>>>(make_double3(origin.x, origin.y, origin.z), slab,
   auto_consts, device_host_consts)`. Grid and block come from `launch.rs` as literals. After each
   launch: `TUNGSTEN_CUDA_CHECK(cudaGetLastError())`.
4. `cudaMemcpyAsync(result_<entry>_pinned, slab + BufferLayout::<entry_buffer>, bytes, D2H, stream)`.
5. Return success. Nothing waits for the GPU.

Ordering guarantees (one host thread per `Orchestration`, all work on one stream):

- Calls can be queued while earlier ones are in flight. They execute in order.
- Reusing the single device `DeviceHostConsts` is safe: each call's H2D is stream-ordered after
  the previous call's kernels.
- `result_<entry>()` is valid after `sync()` (or `query()` returning success). It stays valid until
  the next `enqueue_<entry>()`; that call's D2H overwrites the same pinned buffer. Calls to
  *other* entries don't touch it.
- No host syncs happen between waves. Stream order enforces dependencies. (The old generator's
  per-wave `cudaStreamSynchronize` is not carried over.)
- An `Orchestration` is not thread-safe. Use one per host thread.

### 11.4 Error handling

Every CUDA call in `orchestration.cu` goes through `TUNGSTEN_CUDA_CHECK(expr)`:

```cpp
#ifdef TUNGSTEN_CUDA_CHECK_HEADER
#include TUNGSTEN_CUDA_CHECK_HEADER     // consumer-provided override
#endif
#ifndef TUNGSTEN_CUDA_CHECK
#define TUNGSTEN_CUDA_CHECK(expr) \
    do { cudaError_t tw_e_ = (expr); \
         if (tw_e_ != cudaSuccess) return ::tungsten_gen::Status{(int)tw_e_}; } while (0)
#endif
```

By default methods return a `Status`, which works with `-fno-exceptions` and is safe for JNI. A
consumer can override the macro to throw or abort, by defining it before the include or with
`-DTUNGSTEN_CUDA_CHECK_HEADER="my_check.h"`. Method signatures don't change. Device-side traps
(§13) show up as a sticky launch error on the next checked call or on `sync()`.

---

## 12. Runtime contract (`tw::`, not generated, not in v1)

Everything below must be provided by `runtime_include` and the profile headers. Device-facing
parts must be NVRTC-compatible (§5.1). Semantics match the **Rust runtime** (the exactness
reference), not vanilla Java directly.

### 12.1 Files

- `tw/tungsten_runtime.cuh`: core types and helpers (12.2–12.4).
- `tw/profile_exact.cuh` / `tw/profile_fast.cuh`: the math/noise externs, same names and
  signatures in both, different implementations (§14).

### 12.2 Core

- `double3`/`int3` global operators `+ - * /` for the operand combinations the IR produces
  (including mixed `double3`/`int3` → `double3`, and scalar forms). They are `constexpr
  __host__ __device__`. Defining `TUNGSTEN_EXTERNAL_VECTOR_OPS` skips them, for consumers that
  already define the same operators (e.g. `helper_math.h`).
- `tw::get(double3, int)`, `tw::get(int3, int)`.
- `template <class T, int N> struct tw::Array { T v[N]; constexpr T& operator[](int);
  constexpr const T& operator[](int) const; }`: trivially copyable aggregate.
- `tw::lval(T&& x) -> T&`.
- `tw::f2i8(float|double) -> signed char`, `tw::f2i32(float|double) -> int`,
  `tw::f2i64(float|double) -> long long`: `constexpr __host__
  __device__`, saturating, NaN → 0. On device they must lower to the `__*2int_rz` / `__*2ll_rz`
  intrinsics.
- `tw::align_up(unsigned long long, unsigned long long)`: constexpr.
- `tw::buffer<T>(char*, unsigned long long) -> T*`.
- `TW_NAN_F64`, `TW_INF_F64`, `TW_NAN_F32`, `TW_INF_F32`: constant-expression macros.
- `operator==` / `operator!=` for `double3`/`int3` (and `tw::Array`), since comparisons are
  emitted as plain operators (§7.8).
- Debug-check macros used when `debug_checks` is on (§13). Both are expressions and must evaluate
  each argument once: `TW_CHECK_INDEX(i, n, where)` evaluates to `i`, `TW_CHECK_DIV(a, b, where)`
  evaluates to `a / b`. `where` is a string literal.

### 12.3 Iteration

- `struct tw::Config { double3 origin; double3 origin_scale; double3 position_scale; int3 dimensions; }`.
  It is an aggregate initialized in this field order.
- `struct tw::PositionIterator { int3 pos; /* plus whatever index() needs */ }`.
- `tw::make_position(const tw::Config&, int3 local) -> tw::PositionIterator`: must produce the
  same `pos` as the Rust runtime's `iter_3d` item for that cell.
- `tw::index(const tw::PositionIterator&) -> int`: `x + dx * (y + dy * z)`.

### 12.4 Samplers and init (host)

- `tw::PerlinNoiseSampler`, `tw::InterpolatedNoiseSampler`: flat, pointer-free, standard-layout,
  trivially copyable, identical layout on host and device.
- `__host__ tw::make_permutation_table(long long seed, long long id_lo, long long id_hi,
  long long subident_index, long long sub_lo, long long sub_hi) -> tw::PerlinNoiseSampler` and
  `__host__ tw::make_base3d_perm_table(long long seed) -> tw::InterpolatedNoiseSampler`: ports of
  the Rust runtime's initializers. Bit-identical tables are required in both profiles.

### 12.5 Externs and extern types

- Every `ExternCall` name `n` the frontends emit must exist as `__device__ tw::<cuda_sanitize(n)>`,
  taking pointers where the IR passes `Ref`.
- Every `Extern(name)` type `t` must exist as `tw::<cuda_sanitize(t)>`. It must be
  default-constructible, constexpr-constructible (for constants), and have fields in IR order
  (for designated initializers) and a static `make(...)` (for `Construct`).
- The intrinsic set is not frozen yet. Until it is, a missing extern is a compile error in the
  consumer's build, not something the backend detects.

---

## 13. Debug checks (`CudaConfig::debug_checks`)

This is a backend flag. With `false` (the default), no checks are emitted. With `true`:

- Every buffer read/write and every `tw::Array` index is wrapped: `buf[TW_CHECK_INDEX(i, N, where)]`
  (`N` = buffer length or array size).
- Every integer `Divide` becomes `TW_CHECK_DIV(a, b, where)`, which catches `b == 0` and `MIN / -1`.
- `where` is the generated function name, or the kernel symbol for the store in a kernel body.
- Runtime semantics: `printf` the location, then `__trap()`.

Switching the flag requires regenerating. Release output stays free of check code.

---

## 14. Profiles (`CudaConfig::profile`)

The profile is fixed at generation time, so the output says which one it is.

| | `Exact` | `Tolerance` |
| --- | --- | --- |
| Profile define | `TUNGSTEN_PROFILE_EXACT` | `TUNGSTEN_PROFILE_TOLERANCE` |
| Extern header | `tw/profile_exact.cuh` (ports of the Rust runtime's math) | `tw/profile_fast.cuh` (libdevice / intrinsics allowed) |
| Required flags | `--fmad=false`; `--prec-div=true --prec-sqrt=true --ftz=false` (nvcc defaults); never `--use_fast_math` | any; `--use_fast_math` allowed |
| Validation | bitwise equal to Rust output | epsilon comparison |

- The required flags are written in the `density.cu` header comment and in a `static constexpr
  const char* profile` / `required_flags` member of `Orchestration`. Nothing can enforce `--fmad`
  at compile time; a future NVRTC host must pass the flags itself.
- Integer semantics (§7.2), shift masking and saturating casts (§7.3) are **identical in both
  profiles**. They are correctness, not precision.
- Neither profile changes IR types (no f64 → f32 demotion). Precision changes belong in an
  optimizer pass.
- No other codegen differences exist in v1. Any later profile-dependent lowering is keyed on
  `CudaConfig::profile` in `types.rs`/`expression.rs` and listed in this table.

---

## 15. Validation

Downstream only, as for the Rust backend. `tungsten-mc-compile` compiles the generated files
against the runtime with nvcc (and later NVRTC), runs them on a GPU, and compares with the
generated Rust for the same program: bitwise for `Exact`, epsilon for `Tolerance`. This repo adds
no GPU or nvcc tests. A text-level smoke test like `tungsten-java/src/tests.rs` may be added but
isn't required.

Expected differential mismatches to rule out first: `LogicalShiftRight` (§7.2), and any extern
whose exact-profile port isn't finished.

---

## 16. Warnings (`DeviceCodegen::warnings`)

- An integer op whose width couldn't be derived (§7.2), with the function name.
- Kernels with `bx*by*bz < 32` (partial warps), as an informational hint for fusion.
- No size/compile-time warnings in v1 (judged not a concern for now).

---

## 17. Edge cases checklist

| Case | Handling |
| --- | --- |
| No entry points | Valid files; `Orchestration` with only `create`/`sync`/`query`. |
| No permutation tables / host inputs | Empty `AutoConsts` / `HostConsts` structs (legal C++); HostConsts copy skipped. |
| Same unit at several dimensions | One density function per `(cu, dims)`; one buffer + kernel per instance. |
| Same unit + dims, different scales | Shared density function; separate kernels with different `Config` literals. |
| Two entry points on the same instance | Same buffer; unique `enqueue_`/`result_` names. |
| Helper reading a `DensityVariable` | `find_density_input` searches all units (as in Rust); index from the caller's `p`. |
| Explicit `DensityVariable` index of type `I64` | Cast to `int`. |
| Constant referencing a constant | Topological emission (§9.6). |
| Function used before its definition / recursion | Forward declarations (§9). |
| `Ref` on an rvalue, in a `While` condition or `&&` operand | `&tw::lval(...)` (§7.7). |
| Braced init with int → double | Explicit casts on every element (§7). |
| `x << 40` on `I32` | Folded to `x << 8` (Java semantics). |
| `(int)NaN`, `(int)1e30` | `tw::f2i32`: 0, `INT_MAX`. |
| `i32::MIN` / `i64::MIN` literals | `(-2147483647 - 1)` / `(-9223372036854775807LL - 1)`. |
| NaN / ±∞ / `-0.0` literals | `TW_NAN_*` / `TW_INF_*` / `-0.0`. |
| Name is a C++/CUDA keyword or builtin | `NamingRules::cuda()` reserved list (§4.1). |
| Name with `__` or leading `_X` | `cuda_sanitize` (§4.2). |
| Two generated programs in one binary | Different `namespace` + `kernel_prefix`. |
| Entry result of extern / sampler type | Panic (no public mirror). |
| `HostInput::Dynamic` | Panic. |
| Instance > `i32::MAX` elements, grid y/z > 65535 | Panic. |
| `enqueue_X` twice before `sync()` | Both run in order; `result_X` holds the second result. |
| Kernel trap with `debug_checks` | Sticky error, reported by the next checked call / `sync()`. |

---

## 18. Future work

1. **Wave fusion** (§9.4): multi-instance kernels within a wave.
2. **Runtime headers** in this crate once intrinsics stabilize (§12).
3. **Rust host orchestration** for `CudaCodegen` via NVRTC/cudarc (kernel lookup by `extern "C"`
   symbol, flags from §14).
4. **C++ CPU device codegen** as a separate `DeviceCodegen`.
5. Shared-memory staging of permutation tables per kernel.
6. CUDA Graph replay per entry; batching several origins per call.
7. `Interface::kernel_body` name, removing the derived `_body` suffix (§4.3).
8. Convert panics to `BackendError` across all backends.
9. Fix `LogicalShiftRight` in `tungsten-rust` (§7.2).

---

## 19. Decision log (from the design interview)

| Topic | Decision |
| --- | --- |
| Host side | CUDA C++ for both device and orchestration; device codegen pluggable behind `DeviceCodegen` for a future Rust host. |
| Batching | One origin per call; async on a per-`Orchestration` stream. |
| Exactness | Two profiles, exact and tolerance, chosen by backend config + runtime extern headers. |
| Consumers | C++ only (JNI via a separate wrapper); one host thread per `Orchestration`. |
| Buffers | One slab, constexpr offsets. |
| Perm tables | Global memory, `const __restrict__`; shared-memory staging later. |
| FMA | Exact profile requires `--fmad=false` (documented, not enforced). |
| Integer overflow | Always wrap via unsigned casts. |
| Shifts / casts | Java semantics; masks omitted (folded) for literal counts; saturating float→int. |
| f64 demotion | No; IR types are kept. |
| Arrays | Runtime `tw::Array<T,N>`. |
| Vec3/Pos3 | `double3`/`int3` + global operators from the runtime. |
| Ref | Pointers; rvalues materialized in-expression (`tw::lval`). |
| Runtime | Contract only for now; in scope once intrinsics stabilize. |
| Index order | `x` fastest, then `y`, then `z`. |
| Kernel shape | Device body + thin `extern "C"` wrapper; multi-instance (fused) kernels later. |
| Uniform args | `p` + one `const Ctx&`. |
| Inlining | Plain `__device__` by default; config override. |
| HostConsts | Device copy per call via pinned staging ring. |
| Files | Public CUDA-free `.h` + `interface.cuh` + `density.cu` + `orchestration.cu`. |
| Errors (host) | `TUNGSTEN_CUDA_CHECK` macro, `Status` by default. |
| Toolchain | CUDA 12+, C++20, sm_70+, device code NVRTC-compatible. |
| Exact math reference | The Rust runtime's implementations. |
| AutoConsts init | Host, then one H2D copy. |
| Results | Internal pinned buffer per entry, `result_<entry>()` after `sync()`. |
| Debug checks | Backend flag. |
| Testing | Downstream only. |
| AST | New `model/`, like `tungsten-java`. |
| Namespaces | Config namespace (`::dev` for device code) + `tw::` runtime; prefixed `extern "C"` kernels. |
| Compile size | Not a concern for now; single device TU. |
| In-flight calls | Allowed, stream-ordered. |
| Profile selection | Backend config only (generation time). |
| Stream/device | Own non-blocking stream, current device. |
| Launch | Per-instance 1D grid, linear id decoded to `(x, y, z)`. |
| ConstructExtern | C++20 designated initializers. |
| Unsupported IR | Panic, like Rust/Java. |
