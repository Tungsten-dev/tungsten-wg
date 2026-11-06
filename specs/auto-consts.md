# Generic autoconsts

Status: design agreed, not implemented. Lands after milestone 5 of `specs/program-level-inputs.md`, which deletes
`tungsten-legacy`, and therefore after `specs/builder.md`. It lands before `specs/density-mappings.md` and
`specs/refinement.md`, which don't depend on tables.

## 1. Summary

The IR knows what a Minecraft noise table is. `PermutationTableInput` is a closed enum, every backend matches on it,
and `tungsten-rust` computes Minecraft's MD5 seed split. Adding a table kind means changing the IR, core, every backend
and the test runtime.

This spec replaces the enum with a generic description of a value built once, when an orchestration is created:

```rust
// Before
OptimizedProgram { permutation_tables: Arena<PermutationTableInput>, .. }
Expression::PermutationTable(Ref<PermutationTableInput>)
VariableType::{PermutationTable, InterpolatedNoiseSampler}

// After
OptimizedProgram { auto_params: Arena<AutoParam>, auto_consts: Arena<AutoConst>, .. }
Expression::AutoConst(Ref<AutoConst>)
VariableType::Handle(&'static str)
```

The frontend decides which autoconsts exist, what they're called, which runtime function builds each one and which
arguments it gets. `tungsten-wg` carries that description, checks it and emits it, and never interprets it.

```text
program {
    param seed: i64;

    auto temperature: handle PerlinNoiseSampler
        = make_permutation_table(param(seed), 5481474280546213738L, -5330868219781169595L, 0L, 0L, 0L);
    auto salt: i64 = derive_salt(param(seed), "caves");

    fn run() -> f64 {
        return (ext sample(auto(temperature), ambient(pos3).x) + (auto(salt) as f64));
    }
    unit u {
        main run;
        autos [temperature, salt];
    }
    entry u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
@case "c" origin(0, 0, 0) param(seed = 1)
```

### Goals

- No Minecraft knowledge in `tungsten-wg`: no Perlin, Base3D or MD5, in code or in dependencies.
- A frontend can add a table kind, or a seed-derived value, without changing this repo.
- The seed stops being special. It's an ordinary `i64` param, and a program can have none or several.
- Lowering catches wrong initializer arguments, instead of `rustc`, `javac` or `nvcc`.
- Interpreter and backends agree bit for bit, as they do for everything else.
- Tools for frontends: a builder API, `.tw` syntax, the registry check and argument evaluation for the interpreter.

### Non-goals

- Hashing helpers or runtime code for frontends. String-to-seed derivation is the frontend's job.
- Autoconsts built from other autoconsts (§9).
- Array, `vec3` or record value autoconsts, and `Str` params (§9).
- Initializers that can fail in generated code (§4.4).
- Byte-identical generated code. Values must match. Text may change anywhere (§7).
- Porting `tungsten-mc-compile`. §8 records what its port must do.

## 2. Data model

### 2.1 `crates/tungsten-ir/model.rs`

```rust
pub struct OptimizedProgram {
    // existing fields, without `permutation_tables`
    pub auto_params: Arena<AutoParam>,
    pub auto_consts: Arena<AutoConst>,
}

pub enum ComputeUnit {
    DensityFunction {
        // Replaces `permutation_table_inputs`.
        auto_consts: Vec<Ref<AutoConst>>,
        ..
    },
}

/// A value supplied once, when the orchestration is created. Programs can't read it directly.
pub struct AutoParam {
    pub name: String,
    pub t: VariableType,        // I64, F64 or Bool
}

pub struct AutoConst {
    pub name: String,           // Field-name hint and display name
    pub t: VariableType,        // Handle, or a scalar: Bool, I8, I32, I64, F32, F64
    pub initializer: String,    // Bare name of a runtime function
    pub args: Vec<AutoConstArg>,
}

pub enum AutoConstArg {
    Param(Ref<AutoParam>),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
}

pub enum Expression {
    // Replaces `PermutationTable`.
    AutoConst(Ref<AutoConst>),
    ..
}

pub enum VariableType {
    // Replaces `PermutationTable` and `InterpolatedNoiseSampler`.
    /// An opaque runtime type, built by an autoconst and only passed to externs.
    Handle(&'static str),
    ..
}
```

`AutoConstArg` and `AutoConst` compare `F64` by bits, so `Eq` and `Hash` hold, and so `0.0` and `-0.0` are different
arguments.

| File | Change |
| --- | --- |
| `retag.rs` | Both arenas join the tag map. The walker visits `Param` refs in `auto_consts`, the unit list and `Expression::AutoConst` |
| `rebuild.rs` | `compact` and `permute` handle both arenas. `compact` keeps relative order, because it's the creation API's order (§5.1). `float_bits` and `fingerprint` include `F64` arguments |
| `tungsten-optimizer/inline.rs` | Renames only. `extend_unique` on unit lists already does what's needed |
| `tungsten-inspect` | Displays `initializer(args)` for an autoconst, and renders both kinds as table-style nodes |

### 2.2 Two kinds of autoconst

| | Handle | Value |
| --- | --- | --- |
| `t` | `Handle(name)` | `Bool`, `I8`, `I32`, `I64`, `F32` or `F64` |
| Read gives | A reference to the runtime object | The scalar itself |
| Program may | Pass it to externs | Use it like a host value |
| Stored in core as | A table (§4.2) | A host (§4.3) |

A const initializer can't read either. `ConstExpr` has no way to express it, and core's rule that const initializers
can't read hosts stays as it is.

Handles keep today's other rules: a function can hold one in a variable, and a host may have a handle type. This spec
doesn't widen or narrow those.

## 3. Checks

### 3.1 Registry

```rust
pub enum AutoArgType { I64, F64, Bool, Str }

RegistryBuilder::new()
    .auto_initializer("make_permutation_table", &[I64, I64, I64, I64, I64, I64], Handle("PerlinNoiseSampler"))
    .extern_fn("perm_get", &[Handle("PerlinNoiseSampler"), I32], I32)
```

Initializers have their own namespace, separate from externs, because backends call them through different paths and
only on the host. A name can be declared once. In a `RegistrySpec` TOML they're an `[[auto_initializer]]` list, with
`handle Name` as the type spelling.

Handle names aren't declared. A typo already fails as a type mismatch at the extern call or against the initializer's
return type.

### 3.2 Lowering errors

Lowering checks all of these, and collects them like its other errors:

| Rule | Error |
| --- | --- |
| Param type is `I64`, `F64` or `Bool` | `BadAutoParamType` |
| Param names are unique | `DuplicateAutoParam` |
| Autoconst type is a handle or an allowed scalar | `BadAutoConstType` |
| Initializer is in the registry | `UnknownAutoInitializer` |
| Argument count matches | `AutoArgCount` |
| Each argument matches its declared type. A `Param` has its param's type | `AutoArgType { index }` |
| Registry return type equals the autoconst's `t` | `AutoInitializerReturn` |
| A `Str` argument has no NUL (`const char*` can't hold it) | `AutoArgHasNul { index }` |
| `Param` and `Expression::AutoConst` refs resolve | The existing `DanglingRef`, with new `EntityKind`s |

A frontend that skips the checker gets what it gets today: the runtime's compiler reports the error.

## 4. Core and the interpreter

### 4.1 Core changes

Core stays free of autoconsts as a concept. It changes only its handle types:

```rust
pub struct HandleTypeId(pub usize);

pub enum Type {
    // Replaces `PermTable` and `Sampler`.
    Handle(HandleTypeId),
    ..
}

pub enum Value {
    // Replaces `PermTable` and `Sampler`.
    Handle(TableId),
    ..
}

// Replaces `TableKind`.
pub struct TableDecl { pub ty: HandleTypeId }
```

- `HandleTypeId` has no declaration table. Core only compares ids, so there's no well-formedness rule to add.
- `Expr::Table(t)` has type `Handle(tables[t].ty)`.
- `value_has_type` keeps its meaning: the table exists and its `ty` equals the expected id. A sampler handle still can't
  pass as a permutation table.
- The names `TableId`, `Expr::Table`, `TableNotListed` and `Program.tables` stay, to keep the proof diff small.
- `SEMANTICS.md` and `CHECK.md` change to match, and `scripts/verify.sh` must pass.

Six files are affected: `ir.rs`, `value.rs`, `util.rs`, `check/types.rs`, `exec/expr.rs` and `lib.rs`.

### 4.2 Lowering handle autoconsts

Lowering interns handle names in order of first appearance: registry declarations first, then the arenas. The result is
deterministic. `LowerMap.handle_types: Vec<String>` gives the names back for error messages.

Each handle autoconst becomes one `TableDecl`, in arena order. A unit's `auto_consts` list provides `U.tables`, as
`permutation_table_inputs` does today.

### 4.3 Lowering value autoconsts

Each value autoconst becomes one extra core host, after the IR's real hosts, in arena order.

- **Reads:** `Expression::AutoConst(r)` lowers to `Expr::Host(h)`.
- **Scoping:** a function's `f.hosts` gets the host when its body reads the autoconst, which lowering finds by scanning.
  `U.hosts` gets it from the unit's `auto_consts` list. Core's rules then apply unchanged.
- **Errors:** `LowerMap.hosts` names these hosts after the autoconst, so an unlisted read says `` autoconst `salt` ``
  rather than a host that doesn't exist in the source.

### 4.4 Supplying values

There's no resolver trait. `tungsten-check` evaluates the arguments and hands the frontend plain data. The frontend
builds whatever it needs and returns the values.

```rust
// `Prepared` and `LowerMap`
pub enum AutoCoreId { Table(TableId), Host(HostId) }

pub struct ResolvedAutoConst<'a> {
    pub core: AutoCoreId,
    pub name: &'a str,
    pub t: &'a VariableType,
    pub initializer: &'a str,
    pub args: Vec<AutoArgValue>,   // Params replaced by their values
}

pub enum AutoArgValue { I64(i64), F64(f64), Bool(bool), Str(String) }

impl LowerMap {
    /// Every autoconst, in arena order, read or not. Params bind by name.
    pub fn resolved_auto_consts(&self, params: &[(String, AutoArgValue)])
        -> Result<Vec<ResolvedAutoConst<'_>>, ParamBindError>;
}

pub struct NamedCase {
    pub origin: ..,
    pub hosts: Vec<(String, Value)>,
    pub auto_values: Vec<(HostId, Value)>,   // New
}
```

The flow for one case:

```rust
let inits = prep.map().resolved_auto_consts(&params)?;
let (externs, auto_values) = my_build(&inits)?;        // Frontend code
let case = prep.case(&NamedCase { origin, hosts, auto_values })?;
prep.eval(&case, &externs, opts)
```

`Externs` keeps its own handle contents, keyed by `TableId`. Core still only sees opaque handles.

`auto_values` is separate from `hosts` and keyed by `HostId`, because autoconst names can collide with host names and
with each other.

| Failure | Result |
| --- | --- |
| Param missing, unknown, duplicated or of the wrong type | `ParamBindError` |
| A value autoconst has no entry in `auto_values` | `CaseBindError::AutoValueMissing { name }` |
| `auto_values` has a `HostId` that isn't a value autoconst, or a duplicate | `CaseBindError::AutoValueUnknown` |
| A value has the wrong type | Core's `HostValue`, named through `LowerMap` |
| The frontend can't build an autoconst | The frontend's own error, returned before `prep.case` runs |

Generated code treats initializers as infallible, as it does today. A panic, a Java exception or a C++ abort isn't a
modelled result, so `new` and `create` keep their return types.

Initializers must be pure and deterministic, as externs must. Every autoconst is built, in arena order, whether or not
anything reads it. An autoconst no unit lists stays in the arena and the generated struct, and is inert otherwise.

## 5. Backends and naming

### 5.1 Creation API

One typed parameter per param, in arena order, which is declaration order. A program with only `seed: i64` keeps
today's signatures.

| Backend | No params | `seed: i64` | `seed: i64, salt: f64` |
| --- | --- | --- | --- |
| Rust | `Orchestration::new()` | `Orchestration::new(seed: i64)` | `Orchestration::new(seed: i64, salt: f64)` |
| Java | `AutoConsts.create()` | `AutoConsts.create(long seed)` | `AutoConsts.create(long seed, double salt)` |
| CUDA | `Orchestration::create(out)` | `Orchestration::create(long long seed, out)` | `Orchestration::create(long long seed, double salt, out)` |

An unread param stays in the signature, so a frontend can keep its API stable. Params aren't merged away.

Parameter order is a contract the frontend controls through declaration order. `permute` changes it, but it's only used
by tests, whose harness is generated per program and binds by name.

### 5.2 `tungsten-interface`

- `auto_const_field_name(rules, &AutoConst)` is `(rules.sanitize)(name)`, made unique. The Perlin formula and the
  `base3d_perm_table` special case go away.
- New `auto_param(r) -> &str`: the sanitized name, made unique against the names generated creation code already uses.
  These are `buffers`, `auto_constants`, `auto_consts`, `_init`, `out`, `impl` and `this`. A param named `buffers`
  therefore compiles.

### 5.3 Emitting

`AutoConstSpec`, `AutoConstArg` in `tungsten-rust` and `permutation_table_specs` are deleted. Each backend iterates
`program.auto_consts` directly. The initializer is emitted as `name` in Rust, `Helpers.name` in Java and `tw::name` in
CUDA, as today.

| | Rust | Java | CUDA |
| --- | --- | --- | --- |
| Handle field type | `T` | `T` | `tw::T` |
| Handle parameter type | `&T` | `T` | `const tw::T*` |
| Handle read | `&auto_constants.f` | `auto_constants.f` | `&auto_constants->f` |
| Value read | `auto_constants.f` | `auto_constants.f` | `auto_constants->f` |
| Type name | `sanitize_name` | `java_sanitize` | Sanitized, with `tw::` |

Argument spelling:

| Argument | Rust | Java | CUDA |
| --- | --- | --- | --- |
| `Param` | The `auto_param` name | Same | Same |
| `I64` | `v_i64` | `LongLit` | `(long long)0x…ULL`, always. `i64::MIN` has no plain decimal literal in C++, and this removes the `i == 3` special case |
| `F64` | The existing literal formatter, including NaN and infinities | Same | Same |
| `Bool` | `true` / `false` | Same | Same |
| `Str` | `&'static str`, new `rcl` literal and type | `String` | `const char*` holding UTF-8 |

Each backend escapes `Str` for its own language, and non-ASCII text arrives as the same code points on every backend.
`Str` values only exist on the host, in `make_auto_consts`, the Java constructor and CUDA's `create`. They never reach a
kernel, so CUDA's device copy isn't affected.

The CUDA `// ident [index] / subident` comment before each assignment becomes `// <name>`. CUDA's `is_pointer` matches
`Handle`. Value fields are scalars, so `AutoConsts` stays trivially copyable as long as runtime handle types are.

The Rust `AutoConsts` keeps `#[repr(C)]` and `Clone`, and the Java class keeps `public final` fields.

## 6. Text format and tests

### 6.1 Syntax

```ebnf
item      = use | host | param | auto | const | fn | unit | entry ;
param     = "param" label ":" type ";" ;
auto      = "auto" label ":" type ("@name" "(" STRING ")")? "=" IDENT "(" (autoarg ("," autoarg)*)? ")" ";" ;
autoarg   = "param" "(" label ")" | LONG | F64 | "true" | "false" | STRING ;
type      = … | "handle" IDENT ;
primary   = … | "auto" "(" label ")" ;
unititem  = … | "autos" "[" labels? "]" ";" ;
```

- **Removed:** the keywords `table`, `tables`, `perlin`, `base3d`, `perm_table` and `interpolated_noise_sampler`, and the
  `table(t)` expression. There's no compatibility form.
- **Literals:** an `I64` argument needs its suffix. A bare `12` is an `i32` literal, and that's an error which suggests
  `12L`.
- **Names:** a param's name is its label. An autoconst's name defaults to its label, and `@name` overrides it. The
  printer labels autoconsts `a<N>` and prints `@name` unless it would equal the label, so P2 holds.
- **Order:** the printer emits `host`s, `param`s, `auto`s, `const`s, `fn`s, `unit`s, `entry`s. Unit lists print `autos`
  where `tables` was.

### 6.2 `@case`

```text
@case "c" origin(0, 0, 0) param(seed = 1) host(n = 3)
```

- `seed(...)` is removed, and `param(...)` mirrors `host(...)`. It's omitted when a program has no params.
- Every declared param must be bound exactly once, with a value of its type.
- An `i64` param accepts the full `u64` range in decimal or hex, reinterpreted as bits, so existing seeds stay valid.
- The core `Case` still has no params. They exist only to compute `resolved_auto_consts` and the generated create call.
- `@cells(sample = …, seed = …)` is a different seed and doesn't change.

The wire format becomes `origin:[u64;3] param* host*`. Params use the existing encoding for their type and are encoded
in the program's arena order after binding by name, as hosts are.

### 6.3 Test runtime

The toy tables stay, under frontend-style ownership. `tungsten-testsuite` acts as a small frontend.

- Its registry declares `make_permutation_table`, `make_base3d_perm_table`, and new toy initializers that cover the
  remaining argument kinds and value autoconsts, in all three runtimes (Rust, Java, CUDA).
- `CaseExterns::new(&[ResolvedAutoConst])` dispatches on the initializer name and returns its externs plus
  `auto_values`.
- Corpus files replace `perlin("minecraft:offset", none, 0)` with the literal `i64`s.
- `SPEC.md` §5.3's definitions and its goldens don't change.
- The `md5` dependency leaves `tungsten-rust`, `tungsten-testsuite` and the workspace.

## 7. Compatibility

**Values only.** On equal programs, the interpreter and all backends must agree bit for bit. Generated text may change
anywhere, and only the differential tests are the bar.

| Consumer | Effect |
| --- | --- |
| `tungsten-mc-compile` | None until its port, because it stays pinned from `specs/program-level-inputs.md` §6.8 |
| JNI (`docs/jni-api.md` in `tungsten-frontends`) | `AutoConsts.create(long seed)` is unchanged if the port declares only `seed`. Public field names follow the port's `name`s |
| `.tw` files | Mechanical rewrite, in the same commit as the syntax change |
| Other specs | `specs/builder.md` §3.2, §5 and its API are edited in place when this lands. `SPEC.md` §5.3 and `crates/tungsten-parse/SPEC.md` are updated |

## 8. For a later `tungsten-mc-compile` port

Not part of this work, but agreed now so it isn't re-decided:

- The frontend owns `xoroshiro_seed` and a helper that turns `(ident, subident, subident_index)` into an `AutoConst`,
  and it declares `seed` as a param and its initializers in its registry.
- **Proof:** `specs/builder.md` §9 applies. Values must match the pinned tag bit for bit.
- **Expected findings:**
  - `transform_spmt/noise.rs:312` declares `VariableType::PermutationTable` for the `base3d_noise` parameter while passing
    a Base3D table. Nominal handle types reject this, so the declaration must become
    `Handle("InterpolatedNoiseSampler")`.
  - Noise ids without a namespace hash differently (`COMPILER_BUGS.md` #8). That bug is now clearly the frontend's.
  - Its Rust runtime's `make_boxed` leaves `AutoConsts` uninitialised (`PAPER.md`, 2026-11-01). A `bool` value autoconst
    makes that undefined behaviour more reachable, so it should zero-initialise first, as the test runtime does.

## 9. Open and deferred

- **Autoconsts built from autoconsts.** Would need ordering, cycle checks and a rule for pointers across CUDA's device
  copy.
- **Array, `vec3` and record value autoconsts.** The core host mechanism would take them. Every backend's initializer
  return and device layout would need coverage first.
- **`Str` params.** Would need string encoding in the public APIs and the harness protocol.
- **Fallible initializers.** Needs failure parity tests and changes `new` and `create`. No initializer can fail today.
- **Parameter structs.** If positional order causes real mistakes, `AutoParams` can be added beside it later.

## 10. Testing

All through the existing structure in `TESTING.md` and `docs/testing-policy.md`.

| Area | Tests |
| --- | --- |
| Corpus (differential) | Handle-only, value-only and mixed programs. No params, one param, and several of mixed types. Each argument kind, including `i64::MIN`, `-0.0`, NaN, `true`/`false`, and `Str` with quotes, a backslash, non-ASCII and non-BMP text. Param names that collide with generated names (`buffers`, `out`, `this`) |
| Core | `value_has_type` with two handle types: a wrong type is rejected. `scripts/verify.sh` passes |
| `tungsten-check` rejects | One test per §3.2 row, plus an unlisted value autoconst read that names the autoconst, and a sampler passed to a perm-table extern |
| Case binding | Each §4.4 failure row. Unbound, extra and wrongly typed `@case` params. A host and an autoconst with the same name stay separate |
| `tungsten-parse` | P1–P6 cover the new items and `@name`. Fuzz generates both arenas. Removed keywords now fail. `12` as an `I64` argument gives the suggested fix |
| `tungsten-ir` | Retag, `compact`, `isomorphic`: `Clone` keeps `Param` refs valid, and `compact` keeps param order. P7: `permute` shuffles both arenas and results are equal |
| Optimizer | Inlining a function that reads each autoconst kind: results are equal and lists are complete |
| Unlisted | An autoconst nothing reads is still built, and it appears in `resolved_auto_consts` |
| Backends | `AutoConsts` is `trivially_copyable` in CUDA with value fields. Device reads of value autoconsts match the host |
| Isolation | A check that fails if `perlin`, `base3d` or `md5` reappears outside the test runtime and history docs |

### Milestones

1. **Handle types.** `VariableType::Handle`, core's `Type::Handle`, `verify.sh`, backends' `convert_type`, `.tw` types and
   the registry. The old enum still drives tables. Nothing else changes.
2. **Autoconst IR, handles only.** Both arenas, `I64` and `Param` arguments, and `param`/`auto`/`@case` syntax. Lowering,
   `resolved_auto_consts`, all backends, `tungsten-inspect`, and the builder's table API. Corpus rewrite. Delete the enum,
   `AutoConstSpec` and `md5`. This can't be split further, because the enum has to go in one step.
3. **Registry checks.** `auto_initializer`, §3.2.
4. **Value autoconsts.** Core-host lowering, `auto_values`, backend reads.
5. **Remaining arguments.** `F64`, `Bool`, `Str`, and the toy initializers for them.
6. **Builder and docs.** The rest of the builder API below, `SEMANTICS.md`, `CHECK.md`, the READMEs and the other specs.

### Builder API

`specs/builder.md` §3 gains these, replacing its table method:

```rust
let seed = pb.param("seed", I64);                   // Same name, same type: existing. Other type: AutoParamConflict
let t = pb.auto_const("temperature", Handle("PerlinNoiseSampler"), "make_permutation_table", [seed.into(), 1i64.into()]);
let v = f.read(t);                                  // Reads like any expression, typed from `t`
```

| Item | Merge key | On a match |
| --- | --- | --- |
| Param | Name | Same type: returns the existing param. Other type: `AutoParamConflict` |
| Autoconst | `t`, `initializer` and `args`, with `F64` by bits | Returns the existing autoconst. The first name wins |

Unit `auto_consts` lists are derived like the other lists, in first-use order, and there's no API to list one by hand.

## 11. Decisions

| Topic | Decision | Rejected, and why |
| --- | --- | --- |
| IR format | `AutoConst` with type, initializer and args, in the IR | Opaque key plus a frontend hook: every backend and tool would need the hook, and the IR would no longer describe the program. Table-only arena: autoconsts could never be more than tables |
| Scope | Handles and typed scalar values | Handles only: no seed-derived values. Autoconst dependencies: ordering, cycles and device-copy pointers, for no current need |
| Seed | An ordinary `i64` param | A built-in `Seed` argument: it keeps a frontend concept in the format and rules out other creation inputs |
| Param storage | New `auto_params` arena | Reuse `HostInput`: hosts are per call and autoconsts are built once, so changing a host per call wouldn't rebuild them |
| Creation API | Positional, in arena order | Params struct: changes every caller, including JNI, with no benefit for the seed-only case |
| Argument kinds | `Param`, `I64`, `F64`, `Bool`, `Str` | `I64` only: frontends would have to encode everything as integers |
| Value and param types | Scalar values. `i64`, `f64`, `bool` params. `Str` literal-only | Host-valid types: array and record coverage in every backend. `Str` params: string encoding in the public APIs and protocol. Both can be added without breaking anything |
| Hashing | Frontend's job | A hash helper in `tungsten-wg`: keeps Minecraft's MD5 split here |
| Handle types | Nominal `Handle(name)` | Untyped handle: loses core's kind check. Reusing `Extern`: every backend branch on `Extern` would have to tell records from handles |
| Core values | Lower to extra hosts | First-class `Expr::Auto`: new proofs and rules for something that behaves exactly like a host |
| Core naming | Keep `Table*` names | Rename to `Auto*`: proof churn, and core really does see only opaque tables |
| Handle declarations | Interned, not declared | `.handle(...)`: boilerplate for a typo that already fails as a type mismatch |
| Initializer declarations | In the registry, checked in lowering | Unchecked: a checked pipeline could still fail in `nvcc` |
| Interpreter data | Plain data from `resolved_auto_consts` | Resolver trait: more API than a data flow needs. Accepted cost: each harness orders its own work and reports its own build failures |
| `auto_values` | Separate field keyed by `HostId` | Extending `hosts` by name: autoconst names can collide with host names and each other |
| Failure | Frontend error before the case runs. Generated code infallible | Fallible `new`/`create`: signature changes and parity tests, for failures that don't exist today |
| Compatibility | Values only | Byte-identical except CUDA, or everywhere: the second needs formatting hints in the IR. Accepted cost: vanilla text diffs stop being a regression check here |
| CUDA `i64` | Always hex | Decimal: `i64::MIN` isn't a valid C++ decimal literal |
| Sequencing | After legacy deletion | Before or with the builder: legacy conversion would need a temporary Minecraft mapping and MD5, and milestones 1–4's byte comparison would depend on it |
| Text format | `param` and `auto`, old keywords removed | Keeping `table` for handles: two item kinds for one arena |
| `@case` | `param(...)` replaces `seed(...)` | `seed(n)` as sugar: two spellings, and the printer has to pick one |
| Spec location | `specs/auto-consts.md` | Root `SPEC.md`: holds the testsuite design |
