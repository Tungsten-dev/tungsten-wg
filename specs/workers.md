# Orchestration and workers: a thread-safe global, single-entry per-thread workers

Status: implemented on Rust, Java and CUDA, and documented (`specs/combined-rollout.md` §11 has the implementation notes). This is a breaking change to the generated code of every backend, so it ships
in v0.3. It lands after `specs/compiler.md` and `specs/auto-consts.md`, and before `specs/density-mappings.md` and
`specs/refinement.md`, whose backend work targets workers directly.

## 1. Summary

Today each backend generates one object that does everything. Rust's `Orchestration` holds the autoconsts and every
instance's buffers, and it has one `orchestrate_<entry>` method per entry. A frontend that runs on several threads
clones all of it per thread. `tungsten-mc`'s hand-written `PreliminarySurfaceCache` wraps one in a `Mutex`.

From now on, the generated code has two layers:

- **`Orchestration`** is global and thread-safe. It is created once from the program's params and builds the
  autoconsts. It is immutable after creation and cheap to share. Its only job is to spawn workers. Later, it is also
  where shared spatial caches for triggered stages live (§10).
- **A worker** belongs to one thread and isn't thread-safe. Each worker runs **exactly one entry point**, and it owns
  that entry's buffers, which are one contiguous array per element type. To run another entry, create another worker.

```rust
// Rust
let orch = Orchestration::new(seed);                  // Send + Sync + Clone (an Arc bump)
let mut w = orch.final_density_worker();              // FinalDensityWorker: Send, not Clone
let hc = FinalDensityHostConsts { blend: 0.5, ..Default::default() };
let out: &[f64; 98304] = w.run(origin, &hc);

// On a pool: one worker per thread
rayon::scope(|_| chunks.par_iter().for_each_init(|| orch.final_density_worker(), |w, c| { w.run(c.origin(), &hc); }));
```

```java
// Java
Orchestration orch = Orchestration.create(seed);      // immutable, safe to share
Orchestration.FinalDensityWorker w = orch.finalDensityWorker();
double[] out = w.run(x, y, z, hc);                    // the pool array; entry output at [0, FINAL_DENSITY_LEN)
```

```cpp
// CUDA
std::shared_ptr<gen::Orchestration> orch;
gen::Orchestration::create(seed, orch);
std::unique_ptr<gen::FinalDensityWorker> w;
orch->create_final_density_worker(w);                 // own stream, slab and pinned ring
w->enqueue(origin, hc); w->sync(); const double* out = w->result();
```

### Goals

- A shared, thread-safe `Orchestration` that builds the autoconsts once, with no per-thread copy of them.
- Per-thread workers that allocate only their own entry's closure, with no locks on the hot path.
- Typed per entry: each worker type has the exact return type and its own `<Entry>HostConsts` holding only the hosts
  its entry can read.
- One copy of the density code in every backend, whichever workers use it.
- Buffer layout is private to the worker, so liveness packing can be added later without an API change (§12).
- An API shape that triggered stages (§10) extend instead of break.
- Same values, bit for bit, on the interpreter and every backend. This change touches no semantics.

### Non-goals

- Triggered stages or spatial caches themselves. §10 only fixes the extension points.
- Thread pools, `run_many`, or any scheduling across workers in generated code.
- Liveness packing of buffers (§12).
- Multi-device CUDA. An Orchestration belongs to one device.
- Byte-identical generated code. Values must match. Text changes everywhere.
- Porting `tungsten-mc-compile` or `tungsten-mc`. §11 records what a port needs.
- Changes to `tungsten-core`, its proofs, or the `.tw` format.

## 2. Where things live

| Item | Crate | Notes |
|---|---|---|
| `WorkerLayout`, `Slot`, `Pool`, `Interface::worker`, `Interface::entry_hosts`, `Interface::input_params` | `tungsten-interface` | Computed in `Interface::build` (§4) |
| `BufferBounds` | `tungsten-interface` | Plain data. The meaning is shared, and each backend maps it (§6) |
| Naming: worker, host-consts and factory names | `tungsten-interface::naming` | Next to `entry_method_names` and `entry_base_names` |
| Codegen for Orchestration, workers and kernels | each backend's `orchestration.rs`, `function.rs`, `expression.rs` | §5 |
| `.buffer_bounds(..)` setter | `RustCompile`, `JavaCompile`, `CudaCompile` | `specs/compiler.md` §5 pattern |
| Harnesses | `tungsten-testsuite/src/harness/{rust,java,cuda}.rs` | Move to workers (§8) |

## 3. Generated API

Names below use the entry `final_density`. `<base>` is the entry base name from `entry_base_names`, and `<Base>` is its
UpperCamel form, made unique by the naming rules against the runtime's names and all other generated type names.
Params are those of `specs/auto-consts.md` §5.1, positional, in arena order.

### 3.1 Rust

```rust
#[derive(Clone)]
pub struct Orchestration { shared: Arc<Shared> }
struct Shared { auto_constants: AutoConsts }          // §10: caches join here later

impl Orchestration {
    pub fn new(seed: i64 /* , other params */) -> Self;
    pub fn final_density_worker(&self) -> FinalDensityWorker;
    // one factory per entry
}

#[derive(Clone, Copy)]
pub struct FinalDensityHostConsts { pub blend: f64 /* only this entry's hosts */ }
impl Default for FinalDensityHostConsts { /* generated, see §5.2 */ }

pub struct FinalDensityWorker {
    orch: Orchestration,                               // keeps autoconsts alive; 'static
    buffers: InternalBuffers,                          // one Box<[T]> per element type (§4.2)
    hosts: HostConsts,                                 // private program-wide copy (§4.4)
}

impl FinalDensityWorker {
    pub const LEN: usize = 98304;
    pub const DIMS: (i32, i32, i32) = (16, 384, 16);
    pub fn run(&mut self, origin: Vec3, host_consts: &FinalDensityHostConsts) -> &[f64; 98304];
}

const _: () = { fn s<T: Send + Sync>() {} fn w<T: Send>() {} fn a() { s::<Orchestration>(); w::<FinalDensityWorker>(); } };
```

- A single-cell entry (`dims == (1, 1, 1)`) returns `T` by value, as today.
- `run` returns a view of its pool: `<&[T; N]>::try_from(&self.buffers.<pool>[0..N]).unwrap()`. The entry output is at
  offset 0 of its pool (§4.2).
- `HostConsts`, `InternalBuffers` and `AutoConsts` lose `pub` on the module surface. They are implementation types.
  `AutoConsts` keeps `#[repr(C)]` and `Clone`.
- Workers aren't `Clone` (D9). A new worker costs one allocation per pool.
- The static assertion makes a runtime handle type that isn't `Send + Sync` fail at a line that names `Orchestration`,
  instead of at a distant `thread::spawn`.

### 3.2 Java

```java
public final class Gen {
    public static final class Orchestration {               // immutable after create; safe to share
        private final AutoConsts autoConsts;
        public static Orchestration create(long seed /* , params */);
        public FinalDensityWorker finalDensityWorker();
    }
    public static final class FinalDensityHostConsts { public double blend; }
    public static final int FINAL_DENSITY_LEN = 98304;

    /** Not thread-safe: one per thread. */
    public static final class FinalDensityWorker {
        public double[] run(int x, int y, int z, FinalDensityHostConsts hc);
    }
    static final class AutoConsts { ... }                   // package-private now
    static final class HostConsts { ... }                   // package-private, program-wide
    static final class InternalBuffers { final double[] f64s; final int[] i32s; final Pair[] pair; ... }
}
```

- `run` returns the **whole pool array** of the entry's element type (D12). The entry output is at
  `[0, FINAL_DENSITY_LEN)`, and `length` may be larger. It is overwritten by the next call on that worker, as today.
- A single-cell entry returns the value.
- Thread safety comes from `final` fields: publishing an `Orchestration` through any safe channel is enough.

### 3.3 CUDA

`orchestration.h` stays CUDA-free:

```cpp
class Orchestration : public std::enable_shared_from_this<Orchestration> {
public:
    static constexpr const char* profile = "...";
    static constexpr const char* required_flags = "...";
    /// Builds the autoconsts on the host and copies them to the current device once.
    static Status create(long long seed /* , params */, std::shared_ptr<Orchestration>& out);
    /// Thread-safe. Fails with cudaErrorInvalidDevice if the current device isn't the orchestration's.
    Status create_final_density_worker(std::unique_ptr<FinalDensityWorker>& out) const;
    ~Orchestration();
private:
    struct Shared; std::shared_ptr<const Shared> shared_;   // device AutoConsts, device id
};

struct FinalDensityHostConsts { double blend; };

/// Runs on its own stream. Not thread-safe: use one per host thread.
class FinalDensityWorker {
public:
    static constexpr int len = 98304;
    static constexpr int dims[3] = {16, 384, 16};
    Status enqueue(Vec3 origin, const FinalDensityHostConsts& hc);
    /// Valid after sync() (or query() returning success), until the next enqueue*.
    const double* result() const;
    Status sync();
    Status query();
private:
    struct Impl; std::unique_ptr<Impl> impl_;   // stream, slab, pinned ring, result copy, shared_ ref
};
```

- A worker holds a `shared_ptr` to `Shared`, so it may outlive the `Orchestration` object.
- `create_*_worker` checks the current device. `enqueue` doesn't, and calling it with another current device is a
  documented precondition violation, as with today's per-thread rule.
- `debug_checks` and `profile` stay per compile, not per worker.

## 4. `Interface`

### 4.1 Worker closure

For entry `e`, the **worker schedule** is `waves_for(&target)`, where `target` is the instance `e` computes. It's exposed
as `Interface::worker_schedule(e)`, and `specs/refinement.md` §6.1 extends it. The worker's instances are that schedule's
instances in `instances()` order. Nothing else gets a slot.

### 4.2 Layout

```rust
pub struct WorkerLayout {
    pub pools: Vec<Pool>,                 // first-use order over the worker's instances, entry's pool first
    pub slots: Vec<(Instance, Slot)>,     // worker's instances, in instances() order
}
pub struct Pool { pub element: VariableType, pub len: usize }   // len in elements
pub struct Slot { pub pool: usize, pub offset: usize, pub len: usize }

impl Interface {
    pub fn worker(&self, entry: usize) -> &WorkerLayout;
    pub fn slot(&self, entry: usize, inst: &Instance) -> Option<Slot>;
}
```

- Pools are keyed by the **IR** element type (`VariableType`, with `Handle` and record names compared by name). Two IR
  types that map to one backend type stay separate pools, which is simple and correct.
- **The entry's output is at offset 0 of pool 0.** Java's return value (§3.2) needs this, and later packing must keep it.
- The other instances follow in `instances()` order, packed contiguously. In this version there's no packing and no
  sharing of ranges.
- **CUDA** has one byte slab per worker, not pools: C++ knows the sizes of runtime records, Rust doesn't. CUDA emits a
  `<Base>Layout` struct per worker with `static constexpr unsigned long long` byte offsets, built as today with
  `tw::align_up(.., 256)`, entry output first. It uses `WorkerLayout.slots` for order and lengths and ignores `pools`.
- The layout is not public API. Generated code may change it in any release.

### 4.3 Offset parameters

```rust
impl Interface {
    /// Inputs `f` reads, directly or through the helpers it calls, in program arena order, deduplicated.
    pub fn input_params(&self, f: Ref<Function>) -> &[Ref<DensityInput>];
}
```

- Every density function and helper gets one offset parameter per entry of `input_params(f)`, after the uniform
  parameters, named `off_<input name>` through the naming rules.
- Every kernel gets the offsets for its unit's main function plus `out_off`, its own slot's offset.
- A call site passes the caller's own offset parameters through. Callers always have them, because `input_params` is
  transitive.
- The worker passes its compile-time constants from `slot(e, ..)`. A density input resolves to the slot of
  `Instance::from_input(di)` in that worker.
- Offset type: Rust `usize`, Java `int`, CUDA `unsigned long long` (bytes).
- The offsets of an input that two workers place differently differ only in value. Code is shared.

A function in no worker schedule (dead code) is still emitted with its parameters. No kernel calls it.

### 4.4 Host consts per entry

```rust
impl Interface {
    /// Static hosts read by any unit in the entry's worker schedule, arena order.
    pub fn entry_hosts(&self, entry: usize) -> &[Ref<HostInput>];
}
```

- `<Base>HostConsts` has exactly those fields, named as in today's `HostConsts`. An entry with no hosts gets an empty
  struct, so every `run` signature has the same shape.
- The program-wide `HostConsts` stays as the **internal** struct that kernels read. Density code doesn't change.
- `run` and `refine_*` copy each field of the per-entry struct into the worker's private `HostConsts`, then launch.
  Fields the entry doesn't own keep their `Default` and are never read. The copy is a few bytes for scalars, and a
  `Copy` (Rust), a reference (Java) or a byte copy (CUDA) for records.
- CUDA uploads the internal device struct through the worker's pinned ring, as today.

## 5. Backends

### 5.1 Buffer reads and writes

A read of input `di` with index `i` (explicit, `0` for single, or `index(p)`):

| Mode | Rust | Java | CUDA |
|---|---|---|---|
| `PerBuffer` | `rd(&buffers.<pool>, off_d, LEN, i)`: checks `i < LEN` as `usize` (negatives fail), then indexes `off_d + i` | `Rt.idx(i, LEN)` (`Objects.checkIndex`), then `<pool>[off_d + i]` | `TW_CHECK_INDEX(i, LEN, loc)`, then `tw::buffer<T>(ctx.buffers, off_d)[i]` |
| `Slab` | `buffers.<pool>[off_d + i as usize]` | `<pool>[off_d + i]` | Checks against the slab length |
| `Unchecked` | `unsafe { *buffers.<pool>.get_unchecked(off_d + i as usize) }` | Same as `Slab` | No check |

`LEN` is a literal from `di.dimensions`. Kernel writes go to `out_off + index(p)`, which is in range by construction.
They use the `Slab` form, or `Unchecked` under `Unchecked`.

`PerBuffer` must fail exactly where today's per-buffer array fails. On the interpreter that's an index error, so the
differential suite runs `PerBuffer` (§8).

### 5.2 Rust

- `InternalBuffers` is `struct { <pool>: Box<[T]>, .. }` with one field per pool type used by **any** worker (one
  program-wide type, so the density code is shared). A worker fills the pools it doesn't use with empty slices.
- Allocation: `vec![<zero>; len].into_boxed_slice()`. `<zero>` is generated from the type: `0`, `false` and zero `Vec3`
  and `Pos3` for those types, `[<zero>; N]` for arrays and `R::default()` for records. Rust implements `Default` only for
  arrays of up to 32 elements, so `HostConsts` and `<Base>HostConsts` get a generated `impl Default` from the same
  values. **Runtime contract:** a record type used as a unit output or host is `Copy + Default`. This replaces
  `make_boxed`'s zeroing, which was UB for records with niches (D10).
- `AutoConsts` is built as one struct literal in `Orchestration::new`, with each initializer called in arena order.
- Kernels: `fn <kernel>(origin, buffers: &mut InternalBuffers, out_off, off_.., auto_constants, host_consts)`. Per cell:
  `let r = density(.., &*buffers, off_..); buffers.<pool>[out_off + idx] = r;`. These are sequential borrows, as today.
- Density functions take `buffers: &InternalBuffers`.
- Generated code keeps `#[allow(warnings)]`. Under `Unchecked` it contains `unsafe` blocks, and nothing else is unsafe.

### 5.3 Java

- `InternalBuffers` holds `final T[] <pool>` per pool, sized per worker. A worker that doesn't use a pool gets a
  shared zero-length array.
- Kernels and density methods take `InternalBuffers b` and the offsets as `int`s.
- Method-size estimates (`size.rs`) include the new parameters. A function with more than 255 parameter slots is a
  `BackendError::Codegen` naming the function (D7).

### 5.4 CUDA

- `Ctx.buffers` stays `char*`. Density functions take offsets as `unsigned long long` parameters, and the program-wide
  `BufferLayout` is replaced by the per-worker `<Base>Layout` (§4.2).
- Kernel wrappers take `out_off` and the main function's offsets as kernel arguments.
- The Orchestration's `Shared` owns the device `AutoConsts` allocation. Each worker's `Ctx.auto_constants` points at
  it, and that's safe because it is read-only after `create`.
- Each worker owns a stream, a slab of `<Base>Layout::total_bytes`, a pinned `HostConsts` ring of `host_consts_ring`
  slots, and a pinned result buffer.

### 5.5 Config and extension traits

```rust
// tungsten-interface
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferBounds { PerBuffer, Slab, Unchecked }
```

| Backend | Field | Default |
|---|---|---|
| Rust | `buffer_bounds: BufferBounds` | `PerBuffer` |
| Java | `buffer_bounds: BufferBounds` | `PerBuffer`. `Unchecked` behaves like `Slab` and isn't a warning |
| CUDA | `buffer_bounds: Option<BufferBounds>` | `None`: `PerBuffer` if `debug_checks`, else `Unchecked`, matching today. An explicit value wins |

Each backend's extension trait gains `fn buffer_bounds(self, b: BufferBounds) -> Self`. The setter-coverage test in
`specs/compiler.md` §7 covers it.

## 6. Behaviour and edge cases

| Case | Behaviour |
|---|---|
| Two workers of the same entry on two threads | Independent. Each has its own buffers, host copy and stored origin |
| Worker outlives the `Orchestration` value | Fine. It holds an `Arc` (Rust), a reference (Java) or a `shared_ptr` (CUDA) |
| `Orchestration` used from many threads | Safe: only `&self` methods, and autoconsts are immutable |
| Worker used from two threads at once | Rust: impossible (`&mut self`, not `Sync`). Java and C++: documented as unsupported |
| Program with zero entries | `Orchestration` is generated with no factories |
| Entry with no hosts | Empty `<Base>HostConsts` |
| Two entries with the same unit and grid | Two worker types, identical layouts. Harmless |
| An instance shared by two entries | One kernel and one density function. Each worker passes its own offsets |
| A pool type used by no instance of a worker | Empty slice or array. Never indexed |
| `run` returns, then `run` again | Rust: the borrow prevents holding the old view. Java/CUDA: the old result is overwritten, as today |
| Rust runtime handle type not `Sync` | Compile error at the generated static assertion |
| Rust record without `Default` | Compile error in `InternalBuffers::new` / `HostConsts` |
| CUDA worker created on the wrong device | `create_*_worker` returns `cudaErrorInvalidDevice` |
| CUDA sticky error in one worker | Only that worker's stream is affected in principle. CUDA sticky errors are context-wide, so the docs say any sticky error makes the Orchestration and all its workers unusable |
| Index out of range in a program | `PerBuffer`: as today (panic, exception, `TW_CHECK_INDEX`). `Slab`: may read a neighbour's data, or fail at the pool end. `Unchecked` (Rust): UB |

## 7. Removed

- `Orchestration::orchestrate_<entry>` (Rust), `Orchestration.orchestrate_<entry>` with `new Orchestration(AutoConsts)`
  (Java), and `Orchestration::enqueue_<base>`/`result_<base>` (CUDA).
- Public `AutoConsts.create` (Java), public `HostConsts` (all), `make_boxed` (Rust runtime use), and the program-wide CUDA
  `BufferLayout`.
- `Clone` on the per-thread object (Rust).

There's no compatibility layer (D4).

## 8. Testing

All through `TESTING.md` and `docs/testing-policy.md`.

| Area | Tests |
|---|---|
| Harnesses | Rust, Java and CUDA harnesses create one `Orchestration` per case and one worker per expected entry. The whole corpus must stay green, with no new `@knownfail`. They compile with `BufferBounds::PerBuffer` |
| Shared instances | Corpus: two entries sharing a mid-graph instance with different closures, so the instance gets different offsets per worker. Values equal the interpreter |
| Shared helper reads | Corpus: a helper reading an input, called from two units in different entries (offset pass-through) |
| Pools | Corpus: one entry mixing `f64`, `i32`, `bool` and a record unit output, so there are several pools, and a record-typed entry |
| Host consts | Corpus: two entries with disjoint and overlapping hosts, so each `<Base>HostConsts` has only its own. The harness binds only those. An entry with no hosts |
| Concurrency | Rust (`tungsten-testsuite/tests/`): N threads, each with its own worker from one `Orchestration`, on distinct origins. Results equal a sequential run. Java: the same with an `ExecutorService`. CUDA: two workers on two host threads, behind the CUDA milestone gate |
| Bounds modes | Rust: an out-of-range read program (an existing `@knownfail`-free OOB corpus case or a new one) fails under `PerBuffer`, compiles under `Slab` and `Unchecked` (not run, since UB). Java `Unchecked` output equals `Slab` output. CUDA: `None` + `debug_checks` emits `TW_CHECK_INDEX`, and `None` alone doesn't |
| Interface | `worker(e)`: entry at pool 0, offset 0. Only closure instances get slots. Determinism (build twice, compare). `input_params` is transitive and in arena order |
| Static assertion | A Rust trybuild-style or `compile_fail` doctest: a runtime handle type with `Cell` fails at the assertion |
| Rust records | The test runtime's records derive `Default`. A record without it fails to compile with a clear error |
| Large arrays | Corpus: a unit output and a host of type `[f64; 33]` compile and match the interpreter |
| Java limits | A synthetic function reading > 255 inputs gives `BackendError::Codegen` |
| Naming | Entry names that collide after CamelCasing (`a_b`, `aB`) get unique worker types. An entry named `orchestration` doesn't clash |

### Milestones

1. **Interface.** `WorkerLayout`, `input_params` and `entry_hosts`, with unit tests. Nothing generated changes.
2. **Rust.** Pools, offsets, Orchestration/worker split, per-entry hosts, `BufferBounds`. Rust harness moves. The corpus
   is green on Rust.
3. **Java.** The same. Java harness moves.
4. **CUDA.** Per-worker layout and slab, `Shared` device autoconsts, worker streams. CUDA harness moves.
5. **Concurrency tests.**
6. **Docs.** README usage, `docs/redesign.md` (usage example, the Rust runtime contract: `Copy + Default` records,
   `Send + Sync` handles) and generated-code docs. Lands with the documentation of the other v0.3 specs.

Each milestone keeps the corpus green on the backends it touches. Backends not yet moved keep the old API until their
milestone, so the harnesses are moved per backend.

## 9. Effects on other specs

Written into those specs: the creation API in `specs/auto-consts.md` §5.1, the `buffer_bounds` setters in
`specs/compiler.md` §5, and lerp helpers on pools in `specs/density-mappings.md` §5.2. `specs/refinement.md` §6 adds its
refine calls to the workers defined here.

## 10. Extension points for triggered stages

Not designed here. These fix what the later spec can rely on, so it doesn't need to break this API:

- **Where caches live.** In `Orchestration`'s private `Shared` (Rust `Arc<Shared>`, Java `final` fields, CUDA
  `shared_ptr<const Shared>`). Caches are interior-mutable and thread-safe. Adding them adds fields, not methods, to the
  public surface.
- **Who computes a miss.** The requesting worker (D13). A worker's private state can grow sub-slots for the cached
  stage's closure. It computes the region on its own buffers (and stream on CUDA) and publishes into `Shared`.
  Deduplicating or racing concurrent misses is the later spec's decision.
- **How a cached input is read.** Today an input is `(pool, offset)` in the worker's buffers. A cached input is instead
  read from a region owned by the cache, so the later spec extends the offset parameter of §4.3 to a `(base, offset)`
  pair for such inputs only. Non-cached reads keep their signatures. Because offsets are already per-input parameters
  derived from the bodies, only the functions that read a cached input change.
- **Workers keep `orch`.** Every worker already holds its Orchestration handle (§3), so it can reach the caches without
  changes to the creation API.
- **Layout privacy** (§4.2) lets cached-stage sub-slots be added without an API change.

## 11. For a later `tungsten-mc` / `tungsten-mc-compile` port

- `main.rs`'s per-thread `orchestrator_cloned` becomes one `Orchestration` plus `for_each_init(|| orch.<entry>_worker())`.
- `PreliminarySurfaceCache` keeps its `RwLock<HashMap>`, but replaces `Mutex<Orchestration>` with a worker per calling
  thread (thread-local), which removes the lock contention. A triggered stage would later replace the hand-written cache.
- Its runtime's handle types must be `Send + Sync`, and its records must be `Copy + Default`. A record with a
  `Vec3` or `Pos3` field can only derive `Default` if those implement it, so the runtime's `Vec3` and `Pos3`
  need `Default` too (the test runtime derives it). The generated zero of a record is
  `<R as Default>::default()`.
- Java/JNI: `new Orchestration(AutoConsts.create(seed))` becomes `Orchestration.create(seed)` plus a worker per thread,
  and callers index `[0, <BASE>_LEN)` of the returned array.

## 12. Decisions

| # | Decision | Rationale | Rejected |
|---|---|---|---|
| D1 | Global immutable `Orchestration` plus single-entry per-thread workers | Autoconsts are built once and shared. Per-thread state is minimal. Typed per entry | Today's all-in-one cloned object: duplicates autoconsts and all buffers per thread |
| D2 | Triggered stages deferred. The API shape and extension points are fixed now (§10) | Keeps this spec implementable now without boxing in caches | Designing caches now: much bigger. Ignoring them: risks an API break |
| D3 | Shared handle (`Arc`/reference/`shared_ptr`) from worker to orchestration | Workers are `'static`, so they fit thread-locals and pools | `Worker<'o>` borrow: hard to store in thread-locals and `'static` tasks |
| D4 | Replace the old API outright | v0.3 is breaking anyway. mc-compile is pinned | Keeping the old type as a convenience: double the surface |
| D5 | One worker type per entry, with exact return types | Misuse is a type error. Return types differ per entry | Runtime-selected entry: erased types, runtime errors |
| D6 | Buffers in one contiguous array per worker, with offsets passed as arguments. CUDA uses one byte slab; Rust and Java use one array per element type | One copy of the density code serves every worker. Safe in Rust and Java. Room for caches and packing | Per-worker structs with duplicated code (code size); pointer table (indirection, `unsafe`); full program struct per worker (memory); a literal byte slab in Rust/Java (`unsafe`, slow and complex in Java) |
| D7 | Per-input scalar offset parameters, derived transitively from bodies | No indirection, and offsets fold and inline. Shared helpers just pass them through | Offset table indexed by input id (one load per read, an extra uniform param) |
| D8 | `BufferBounds::{PerBuffer, Slab, Unchecked}` as a backend option. Rust/Java default `PerBuffer`. CUDA follows `debug_checks` unless set | Keeps today's failure behaviour by default and lets frontends trade safety for speed | Slab-only checks (silent neighbour reads); no unchecked mode |
| D9 | Workers aren't `Clone`. The Orchestration is | Creation is cheap. Prevents accidentally duplicating frozen refinement state | Clone workers (fork a march: no current need) |
| D10 | Rust records are `Copy + Default`, and buffers and host structs use generated zero values | Safe. Removes `make_boxed`'s zeroing UB, and works for arrays longer than 32 | Keeping zeroed memory; `#[derive(Default)]` and `T::default()`, which stop at 32 elements |
| D11 | Thread safety by documented contract plus a Rust static assertion | A clear error at the generated code for a non-`Sync` runtime | Contract only (indirect errors) |
| D12 | Java `run` returns the pool array, with the entry output at offset 0 | No copy per call | Copy into a result array (768 KB per call for big entries); returning a view (API change, slower reads) |
| D13 | Triggered-stage misses are computed by the requesting worker | No shared worker to contend on. Fits per-thread resources | A dedicated locked internal worker (today's `Mutex<Orchestration>` contention) |
| D14 | `HostConsts` per entry in the API, copied into a private program-wide struct | Callers see only the hosts their entry uses, and the density code is shared | Per-host scalar params (signature growth, records by reference); per-entry struct with duplicated code |
| D15 | CUDA worker owns a stream, slab, pinned ring and result buffer. The Orchestration owns the device autoconsts | Same model as today, per worker. One device copy of the autoconsts | Caller-supplied streams (more misuse, CUDA types leak into the API) |
| D16 | Java `AutoConsts` becomes package-private, wrapped by `Orchestration` | One shape across backends | Keeping it public alongside |
| D17 | Lands before density-mappings and refinement | v0.3 ships as one release, so their lerp helpers and refine calls are written once, on workers | Landing last: both would be written on the old `Orchestration` and rewritten |
| D18 | `specs/workers.md` | Root `SPEC.md` holds the testsuite design | Overwriting or appending to `SPEC.md` |

Decided without an interview question, and easy to revisit: pools are keyed by the IR type; there's an empty
`<Base>HostConsts` for host-less entries; the UpperCamel worker names and `<base>_worker` factory names; `LEN`/`DIMS`
constants; the CUDA device check only in `create_*_worker`; the Java 255-parameter limit as a codegen error; dead
functions are still emitted with offset params.

## 13. Open and deferred

- **Triggered stages / spatial caches.** Their own spec, building on §10. It covers miss deduplication, eviction, and
  keying by region.
- **Liveness packing** of worker buffers. The layout is private and the entry stays at offset 0. With refinement,
  frozen buffers must stay live across calls.
- **Const-generic or specialized offsets.** If measurements show runtime offsets block optimization in Rust or the JVM,
  emit per-worker wrappers with constant offsets for hot kernels.
- **Multi-device CUDA.** One Orchestration per device for now.
- **`run_many` / pool helpers** in generated code, if frontends keep writing the same `for_each_init` pattern.
- **Java parameter limit.** If real programs approach 255 offset params, fall back to an offset table for that function.

## 12. Implementation notes

The notes for each backend are in `specs/combined-rollout.md` §11, because workers landed per backend together with mappings and refinement. The ones that differ from this spec:

- **CUDA `Slab`** generates the same code as `Unchecked`, since worker-shared device code has no slab length. §5.1 says it checks against the slab.
- **Java pools** are always allocated (`new T[0]` for a pool the worker doesn't use), instead of one shared zero-length array. Java's 255-slot limit counts every parameter of every generated method, so the kernel with `10 + n` slots for `n` offsets fails first: 245 inputs generate, 246 give `BackendError::Codegen`.
- **Docs** (milestone 6) landed in `README.md` ("Running the generated code"), `docs/redesign.md` (usage and the Rust runtime contract), `use-guide.md` §5.2 and `crates/tungsten-cuda/README.md`.
