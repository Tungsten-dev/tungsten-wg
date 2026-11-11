# tungsten-cuda

CUDA C++ backend. Modelled on `tungsten-rust` (device side + orchestration side on top of
`tungsten-interface`) and validated against the Rust output downstream. The crate docs in
`src/lib.rs` describe the code layout; this file holds the contracts and decisions the code
can't state.

## Generated files

| File | Compiled by | Notes |
| --- | --- | --- |
| `orchestration.h` | any C++17 host compiler | Public API. No CUDA headers or types, so g++/clang/MSVC and a JNI wrapper can include it alone. |
| `interface.cuh` | nvcc / NVRTC | Structs the device code is generated against, kernel declarations. |
| `density.cu` | nvcc / NVRTC | All `__device__` code and the kernels. |
| `orchestration.cu` | nvcc (host code) | Host class, layout `static_assert`s. |

- `density.cu` and `orchestration.cu` are separate TUs. Launching a kernel from another TU needs
  no `-rdc`, because all `__device__` code lives in `density.cu`.
- `interface.cuh`, `density.cu` and the device parts of the runtime must compile under NVRTC: no
  host standard headers, types spelled `int`/`long long`/`unsigned`/... (`long long` is 64-bit on
  every CUDA platform). Only the two host files may use the standard library.
- Toolchain floor: CUDA 12+, `-std=c++20`, `sm_70`+.
- Two generated programs can share one binary with different `namespace` and `kernel_prefix`.

## Semantics contracts

| Topic | Rule |
| --- | --- |
| Integer arithmetic | Java semantics in both profiles: `+ - *` and negation wrap (computed via unsigned casts), shift counts are masked by 31/63 (folded for literal counts), `>>>` is a logical shift. `I8` is computed in 32 bits and cast back. |
| Float → int | `tw::f2i8/f2i32/f2i64`: saturating, NaN → 0 (Rust `as` semantics; `f2i8` clamps where Java `(byte)` would wrap). |
| Index order | `idx = x + DX * (y + DY * z)`, `x` fastest. Defined once in the runtime (`tw::index`, `tw::make_position`) and must match the Rust runtime's `index()` / `iter_3d`. |
| Constants | Emitted in dependency order (C++ needs declaration before use); cycles panic. Scalars are `constexpr`, aggregates `__device__ constexpr`, since a host `constexpr` aggregate can't be indexed dynamically on the device. |
| `Ref` on an rvalue | Materialized inside the expression: `&tw::lval(expr)`. The temporary lives until the end of the full expression. Writes through a `Ref(true)` to a temporary are discarded, like `&mut (a + b)` in Rust. |
| Extern calls | Always `tw::`-qualified, every argument cast to its declared parameter type. |
| Unsupported IR | Panics naming the function or unit (`HostInput::Dynamic`, a result or host input with no public mirror type, a sampler-typed unit, more than `i32::MAX` elements). |

Known divergences from `tungsten-rust`, to rule out first in differential tests:

- `LogicalShiftRight`: Rust emits an arithmetic `>>` on signed values; CUDA follows Java `>>>`.
- `I8` shifts by 8 or more: Rust overflows (debug panic, release masks by 7); CUDA and Java mask
  by 31 on the promoted `int`.

## Host orchestration contract

- `Orchestration::create(<params>, out)` takes one typed argument per auto param, in declaration order, and
  fills a `std::shared_ptr<Orchestration>`. It builds the autoconsts on the host and copies them to the
  current device once (the caller calls `cudaSetDevice` first). An `Orchestration` is immutable and
  thread-safe; its job is to spawn workers.
- `create_<entry>_worker(out)` makes one `<Base>Worker` per entry, on the current device. Another device gives
  `cudaErrorInvalidDevice`. A worker owns a non-blocking stream, a slab laid out by its `dev::<Base>Layout`
  (256-byte aligned, the entry's output first), a pinned `HostConsts` staging ring and a pinned result
  buffer. It holds a `shared_ptr` to the shared state, so it may outlive the `Orchestration`.
- A worker is not thread-safe: use one per host thread. Two workers on two threads don't interact.
  CUDA sticky errors are context-wide: after one, the `Orchestration` and every worker are unusable.
- All work of a worker runs on its stream, in order. No host syncs happen between waves.
  `enqueue(origin, <Base>HostConsts)` copies that entry's hosts into the device struct and launches the
  worker's schedule. Entries with refinements also get `enqueue_refine_<name>(Pos3 offset, hosts)`, which
  continues from the origin of the last `enqueue`.
- `result()` is valid after `sync()` (or `query()` returning success) and until the next `enqueue*()` of
  that worker.
- Buffer reads follow `CudaConfig::buffer_bounds`: `None` means `PerBuffer` (`TW_CHECK_INDEX`, which the runtime
  must define) with `debug_checks`, else `Unchecked`. `Slab` generates the same code as `Unchecked`, since
  device code shared by workers has no slab length.
- Every CUDA call goes through `TUNGSTEN_CUDA_CHECK`, which returns a `Status` by default (works
  with `-fno-exceptions`, safe for JNI). Consumers can override it by defining it first or with
  `-DTUNGSTEN_CUDA_CHECK_HEADER="my_check.h"`. Device traps surface as a sticky error on the next
  checked call or `sync()`.

## Profiles

| | `Exact` (default) | `Tolerance` |
| --- | --- | --- |
| Extern header | `tw/profile_exact.cuh` (ports of the Rust runtime's math) | `tw/profile_fast.cuh` (libdevice / intrinsics allowed) |
| Required flags | `--fmad=false --prec-div=true --prec-sqrt=true --ftz=false`, never `--use_fast_math` | any |
| Validation | bitwise equal to Rust | epsilon |

The flags are written in the `density.cu` header and in `Orchestration::required_flags`; nothing
can enforce `--fmad` at compile time, so a future NVRTC host must pass them itself. Integer
semantics and saturating casts are identical in both profiles (correctness, not precision).
Neither profile changes IR types. Any future profile-dependent lowering belongs in this table.

`debug_checks` wraps every buffer/array index in `TW_CHECK_INDEX(i, n, where)` and every integer
division in `TW_CHECK_DIV(a, b, where)` (catches `b == 0` and `MIN / -1`). The runtime prints the
location and calls `__trap()`. Release output contains no check code.

## Runtime contract (`tw::`)

The production runtime is not in the repo yet (`../cuda-density`); the testsuite has its own toy-table version in `crates/tungsten-testsuite/runtime/cuda/tw/`; it moves here once the intrinsic set stabilizes. Until then a
missing extern is a compile error in the consumer's build. Semantics follow the Rust runtime, not
vanilla Java. `runtime_include` (default `tw/tungsten_runtime.cuh`) plus the profile header,
next to it, must provide:

- Global `constexpr __host__ __device__` operators `+ - * /` for `double3`/`int3` in every
  combination the IR produces (mixed → `double3`, scalar forms), and `==`/`!=` for `double3`,
  `int3` and `tw::Array`. `TUNGSTEN_EXTERNAL_VECTOR_OPS` skips the arithmetic ones (e.g. with
  `helper_math.h`).
- `tw::get(double3|int3, int)`; `template <class T, int N> struct tw::Array { T v[N]; operator[] }`
  (trivially copyable aggregate); `tw::lval(T&&) -> T&`; constexpr `tw::align_up`;
  `tw::buffer<T>(char*, unsigned long long) -> T*`.
- `tw::f2i8/f2i32/f2i64` overloaded for `float`/`double`, `constexpr __host__ __device__`. On the
  device they must lower to `__double2int_rz`-style intrinsics (PTX `cvt.rzi` saturates and maps
  NaN to 0). The backend can't emit those directly because constant initializers are evaluated on
  the host.
- `TW_NAN_F64`, `TW_INF_F64`, `TW_NAN_F32`, `TW_INF_F32` as constant expressions;
  `TW_CHECK_INDEX` / `TW_CHECK_DIV` as expressions that evaluate each argument once.
- `tw::Config { origin, origin_scale, position_scale, dimensions }` (aggregate, this order),
  `tw::PositionIterator { int3 pos; ... }`, `tw::make_position(config, local)` (same `pos` as the
  Rust `iter_3d` item), `tw::index(p)`.
- Handle types (`tw::<Name>` for `Handle("Name")`): flat, pointer-free, trivially copyable, same layout
  on host and device. Host initializers are `tw::<initializer>(args)`, called in `create` with `i64`
  arguments as `(long long)0x…ULL`, `f64` through the usual literal spelling, `bool` and strings as
  `const char*` of UTF-8 (octal escapes). They must give values bit-identical to the other runtimes.
  Value autoconsts are scalar fields of `AutoConsts`, which stays trivially copyable (asserted).
- `__device__ tw::<cuda_sanitize(n)>` for every extern call (pointers where the IR passes `Ref`),
  and `tw::<cuda_sanitize(t)>` for every extern type: default- and constexpr-constructible, fields
  in IR order (checked by designated initializers), and a static `make(...)`.

## Design decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| Host side | C++ orchestration; device codegen stays independent behind `DeviceCodegen`. | Coupling device code to the host class: a future Rust host (NVRTC/cudarc) must be able to drive `CudaCodegen`. |
| Uniform args | `p` + one `const Ctx&`. | Rust's four separate parameters: too many arguments through deep helper chains. |
| Kernel shape | Per-instance `_body` + thin `extern "C"` wrapper. | Logic in the `__global__` itself: wave fusion needs to call several bodies from one kernel unchanged. |
| Kernel symbols | `extern "C"`, prefixed with `kernel_prefix`. | Mangled names: a future host finds kernels with `cuModuleGetFunction`. The prefix is needed because `extern "C"` ignores namespaces. |
| Launch | 1D grid of `ceil(N / B)` blocks, linear id decoded to `(x, y, z)`. | 3D blocks: pad narrow dimensions to powers of two (5×49×5 as (8,16,1) runs 65 warps where 39 do). |
| `(1,1,1)` instances | Ordinary one-thread kernels. | Special-casing: wave fusion will remove the overhead. |
| Buffers | One slab per `Orchestration`, offsets as `constexpr` in `interface.cuh`, 256-byte aligned. | One `cudaMalloc` per buffer; offsets computed in Rust, which doesn't know the sizes of runtime types. |
| AutoConsts | Built on the host in heap memory, one H2D copy, read through `const __restrict__`. | The stack (too large). Shared-memory staging is deferred, not rejected. |
| HostConsts | One device copy per worker, uploaded per call through a ring of pinned staging slots; byte copy, since public and device structs are `static_assert`ed layout-identical. | A blocking upload per call: the ring only waits if a slot's previous H2D is still running. |
| Integer overflow | Always wrap via unsigned casts. | Plain signed ops: UB in C++, and NVVM exploits it. |
| `Ref` on rvalues | `&tw::lval(expr)` in the expression. | Hoisting into pre-statements (as Java does): breaks `While` conditions (evaluated once) and short-circuited `&&`/`\|\|` operands. |
| Vector construction | Aggregate `double3{...}` / `int3{...}`. | `make_double3`: not usable in `constexpr` constants. |
| Extern records | C++20 designated initializers. | Positional init: field-order drift would compile silently. |
| Profiles | Fixed at generation time by `CudaConfig`; same IR types in both. | Runtime switch; f64 → f32 demotion (belongs in an optimizer pass). |
| Exactness | `--fmad=false` required, documented, not enforced. | Enforcement: impossible at compile time. |
| Host errors | `TUNGSTEN_CUDA_CHECK` returning `Status`, overridable. | Exceptions: break `-fno-exceptions` builds and JNI. |
| Batching | One origin per call, async on a per-`Orchestration` stream; no per-wave syncs. | The old generator's per-wave `cudaStreamSynchronize`. |
| AST | Own owned AST in `model/`, like `tungsten-java`. | Porting `old/cuda/` (reference only). |
| Inlining | Plain `__device__` by default, `InlinePolicy` overrides. | Forcing inlining: let nvcc decide. |
| Testing | Text-level smoke tests here; GPU differential tests in `tungsten-testsuite` (local, needs nvcc and a GPU); more downstream in `tungsten-mc-compile`. | nvcc/GPU tests in this crate. |

## Open questions

- Not yet verified under nvcc and NVRTC: `__device__ constexpr` aggregates whose initializers
  reference other such constants. Fallback: emit them as `__device__ const` with the initializer
  folded by the backend.
- The `_body` suffix is derived outside the `Interface`; a helper literally named `<kernel>_body`
  panics. Proper fix: `Interface::kernel_body`.
- Future work: wave fusion (members of one wave get contiguous `blockIdx.x` ranges in one kernel
  and call their `_body`; keep `launch.rs` and the body signature stable for it), runtime headers
  in this crate, a Rust host via NVRTC/cudarc, a C++ CPU `DeviceCodegen`, shared-memory staging of
  permutation tables, CUDA Graph replay and multi-origin batching, panics → `BackendError` across
  backends, fixing `LogicalShiftRight` in `tungsten-rust`.
