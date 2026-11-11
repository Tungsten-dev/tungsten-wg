# Tungsten use guide

Tungsten compiles Minecraft-style world generation (density functions) into fast Rust, CUDA or Java code. This guide covers what you can do with it, how to use it, the rules of the `.tw` text format, and the features that are still in development.

Status labels used below:

- **Available**: works on every backend today.
- **In development**: implemented in part. The section says what works.
- **Planned**: designed in `specs/`, not usable yet.

The details live in these files:

| Topic | File |
| --- | --- |
| Overall design and crate rules | `docs/redesign.md` |
| `.tw` format (authoritative) | `crates/tungsten-parse/SPEC.md` |
| Building programs from Rust | `crates/tungsten-builder/README.md` |
| Test harness and corpus | `crates/tungsten-testsuite/README.md`, `TESTING.md` |
| Workers, mappings, refinement | `specs/workers.md`, `specs/density-mappings.md`, `specs/refinement.md` |
| Landing order of the three | `specs/combined-rollout.md` |

---

## 1. What you can do with Tungsten

- Describe a world-generation computation as a **program**: pure functions over positions, grouped into compute units that fill grids of cells.
- Compile that one program to **Rust** (CPU, default), **CUDA C++** (GPU, `cuda` feature) or **Java** (`java` feature). Every backend gives the same values, bit for bit.
- Run the program on a reference **interpreter** to get expected values. The test suite compares each backend with it.
- **Inspect** a program and its schedule (IR text, DOT/SVG graphs, diffs) with the `inspect` backend.
- Let the compiler **inline** small units into their readers (`Inline::All`).

This repository is the backend side. A frontend (for Minecraft, `tungsten-mc-compile`) turns Minecraft's density functions into a Tungsten program. You can also write programs by hand in `.tw` files, or build them from Rust with `tungsten-builder`.

---

## 2. The model in one page

A **program** is made of these parts:

| Part | What it is |
| --- | --- |
| `fn` | A function. It has typed parameters, local variables and a body of statements. |
| `unit` | A **compute unit**: a main `fn` plus the helpers, hosts, autoconsts, constants and inputs it uses. A unit is evaluated once per cell of a grid. |
| `input` | A named link from one unit to another unit's grid: "this buffer holds unit `X` computed on this grid". Readers use `density(name)`. |
| `entry` | A unit evaluated on a grid and returned to the caller. |
| `host` | A value the caller passes at run time (a *host const*), such as a blend factor. |
| `param` | A value passed once when the program is created, such as a seed. |
| `auto` | A value built once from params by a runtime function (a noise table, a salt). Programs can only read it. |
| `const` | A compile-time constant. |

A **grid** is `@dims(x, y, z)` cells plus two scales:

- `@origin_scale` multiplies the caller's origin.
- `@position_scale` is the world size of one cell step.

A function sees where it is through `ambient(...)`:

| Ambient | Type | Meaning |
| --- | --- | --- |
| `ambient(pos3)` | `pos3` | The integer cell position |
| `ambient(origin)` | `vec3` | The origin passed to the run |
| `ambient(origin_scale)` | `vec3` | The grid's origin scale |
| `ambient(position_scale)` | `vec3` | The grid's position scale |

**Reading another grid.** `density(d)` reads the buffer of input `d` at the current cell. `density(d, i)` reads it at an explicit flat index. Reads are always of data the compiler can schedule before the reader, so programs have no cycles.

**Scheduling is automatic.** The compiler works out which buffers each entry needs, orders them into waves, and generates one kernel per unit and grid. You never write the schedule.

---

## 3. Using Tungsten

### 3.1 From Rust

Add the crate:

```toml
[dependencies]
tungsten-wg = "0.2"                                  # Rust backend only
# tungsten-wg = { version = "0.2", features = ["cuda"] }
```

Build a program with `tungsten-builder`, then compile it:

```rust
use tungsten_wg::builder::prelude::*;
use tungsten_wg::{Compiler, Inline, Passes, RustBackend, RustCompile, RustConfig, RustOutput};

let mut pb = ProgramBuilder::new();
let mut f = FnBuilder::new("height", F64);
f.ret(pos3().y().cast(F64) * 0.5);
let unit = pb.finish_unit(f);
pb.entry(unit, Grid::new((16, 1, 16)));
let program = pb.build()?;                           // all mistakes at once, with file and line

let registry = tungsten_wg::registry_from([])?;      // externs and records the program uses
let out: RustOutput = Compiler::new(program, registry, RustBackend::new(RustConfig::default()))
    .passes(Passes::default().inline(Inline::All))   // optional
    .deny_warnings()
    .compile()?;
```

What `compile` does, in order:

1. Validates the backend's config.
2. Checks the program (lowering and the core checker).
3. Runs the passes, and checks again if they changed anything.
4. Builds an `Interface` and calls the backend's `generate`.

It fails with `CompileError::Backend`, `Invalid`, `InvalidAfterPasses` or `Warnings`.

To target several languages, build one `Compiler` per backend and clone the program for all but the last. The CUDA backend has a numeric `Profile`: `Exact` (default, bitwise equal to Rust) or `Tolerance` (epsilon-equal, intrinsics allowed).

The builder derives every list (helpers, hosts, inputs, constants) from the function bodies, so you can't forget one. It also types every expression as you write it. Details are in `crates/tungsten-builder/README.md`.

### 3.2 From `.tw` text

`.tw` is the text form of the same program. It is used for test files and for debugging. The `tungsten-parse` crate reads it (`load`) and prints any program back as canonical text (`print`).

A complete file:

```text
// A grid of 4 cells, each reading a coarser input.
@fuel(1000)
program {
    host scale: f64;

    fn leaf() -> f64 {
        return (ambient(pos3).x as f64);
    }
    unit leaf_u { main leaf; }

    input inner = leaf_u @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);

    fn top() -> f64 @hosts(scale) {
        return (density(inner) * host(scale));
    }
    unit top_u {
        main top;
        hosts [scale];
        inputs [inner];
    }
    entry top_u @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
@case "c1" origin(0, 0, 0) host(scale = 2)
@expect "c1" top_u[0, 0, 0] = 0.0
@expect "c1" top_u[3, 0, 0] = 6.0
```

Everything outside `program { … }` is the *envelope*: directives that say how to test it (§4.9).

### 3.3 Running the test suite

Drop a `.tw` file into a directory under `crates/tungsten-testsuite/corpus/`. No code changes are needed. Every file gets these tests:

| Test | Checks |
| --- | --- |
| `roundtrip` | Print and parse give the same program |
| `lowering` | Lowering is stable and matches a snapshot |
| `interp` | Every `@case` matches its `@expect` lines on the interpreter |
| `inline` | Inlining doesn't change cell values |
| `rust::original`, `rust::inlined` | The Rust backend matches the interpreter bit for bit (Java and CUDA the same, when their toolchains are found) |

```bash
cargo test -p tungsten-testsuite                  # everything
cargo test -p tungsten-testsuite -- density/      # one corpus directory
TW_BACKENDS=rust cargo test -p tungsten-testsuite # pick backends
TW_LARGE=1 cargo test -p tungsten-testsuite       # include @tier(large) files
```

A file that expects the checker to refuse it uses `@expect reject(Kind)` and goes in `corpus/reject/`.

---

## 4. The `.tw` format: parsing rules

This section is the short version of `crates/tungsten-parse/SPEC.md`. Where they differ, the spec wins.

### 4.1 Lexical rules

- UTF-8 source. Whitespace (space, tab, LF, CRLF) never matters.
- Comments are `//` to the end of the line. There are no block comments.
- **Identifiers** are `[A-Za-z_][A-Za-z0-9_]*` and can't be keywords. A name that is a keyword, or that has other characters (`minecraft:overworld/x`), is written quoted: `` `minecraft:overworld/x` ``. Inside backticks, `` \` `` and `\\` are escapes.
- **Strings** are `"…"` with the escapes `\\ \" \n \t \r \u{hex}`. They are used for `@name`, case names and reasons.
- **Label suffixes** carry slot numbers for printed programs: `name#N`, `name##N`, `%N` (no spaces). A hand-written file almost never needs them. Every label must be unique within its kind.
- **Keywords**: `program fn unit main input entry host param auto const use let vars return break if else while repeat true false as new extern ext mut dynamic density ambient helpers autos hosts consts inputs inf nan`, plus the type names. `refinement`, `of`, `rerun` and `with` are *contextual* keywords: they are still usable as names.

### 4.2 Numeric literals

| Written | Type |
| --- | --- |
| `42`, `1_000` | `i32` |
| `-128b` or `-128_i8` | `i8` |
| `1L` or `1_i64` | `i64` |
| `0xFFFF_FFFF`, `0b101` | A bit pattern of the width: `0xFFFFFFFF` is `i32` `-1` |
| `0x80_i8`, `0x8000000000000000_i64` | Bit pattern of that width |
| `1.5`, `1e10`, `2.0e-7` | `f64` |
| `0.1f` or `0.1_f32` | `f32` |
| `inf`, `-inf`, `nan`, `nan(0x7ff8000000000001)` | `f64` special values |
| `inf_f32`, `-inf_f32`, `nan_f32`, `nan_f32(0x7fc00001)` | `f32` special values |
| `true`, `false` | `bool` |

Rules that trip people up:

- **The suffix decides the type.** `1` is never a float inside a body, so write `1.0`. Attribute positions (`@origin_scale(1, 1, 1)`, `origin(0, 0, 0)`, expected values) are the exception: there an integer is accepted where a float is expected.
- Short suffixes (`b`, `L`, `f`) only work on decimal literals. For hex and binary use `_i8` / `_i64`.
- Radix prefixes are lowercase. `_` goes between digits only (`1_000`, not `1__0` or `1_`).
- Integers must fit: `-2147483648` is fine, `2147483648` is an error. Hex and binary literals can't take a sign.
- `1.` and `.5` are not literals. A float needs digits on both sides of the point.
- A finite literal that overflows (`1e999`) or rounds a nonzero value to zero (`1e-400`) is an error. Write `inf` explicitly.
- `-nan` is an error. `nan(0x…)` must encode a NaN.
- **Glued minus.** A `-` directly followed by a digit or `inf`, in operand position, is part of the literal. In infix position `-` is always subtraction. So `a -1` is `a - 1`, `a - -1` subtracts `-1`, and `- 1` or `-(1)` negates `1`.

### 4.3 Types

```text
bool  i8  i32  i64  f32  f64  vec3  pos3  handle
[T; n]               // array of n elements
extern Name          // a record or opaque type from the registry
```

`vec3` is three `f64`. `pos3` is three `i32`. `handle` types name runtime objects (`handle PerlinNoiseSampler`).

### 4.4 Program items

A program is `program { item* }`. Items can come in any order and may refer forward: names are collected first, then bodies are resolved.

```text
host name: T;                          // run-time value (or `host name dynamic;`)
param name: T;                         // creation-time value: i64, f64 or bool
auto name: T [@name("x")] = init(param(p), 5L, 1.5, true, "text");
const name = <const expr>;
fn name(a: T, b: T) -> T [@hosts(h1, h2)] [@hash(n)] { ... }
unit name { main f; helpers [..]; autos [..]; hosts [..]; consts [..]; inputs [..]; }
input name = unit_name <grid> [mapping] [@previous];
entry [label =] unit_name <grid>;
use prelude::{lerp, smoothstep};
```

Key rules:

- **Lists are never derived.** A unit must list every helper it reaches, every host, auto, constant and input its code reads, including the ones its helpers read. Omitted means empty. The checker reports what's missing (`helper_not_listed`, `input_not_listed`, …). This is explicit on purpose, so reject tests can write wrong lists.
- A unit needs exactly one `main`. Each list may appear at most once.
- `input` is a **top-level** item, not part of a unit. Several units can list the same input.
- `entry` without `label =` is labelled with its unit's name. Entry labels are what `@expect` refers to.
- `const` has no type annotation. Its type comes from its value. Allowed in a const expression: literals, other consts, operators, casts, field access, indexing, array literals and constructors.
- `@hosts(...)` on a `fn` lists the hosts that function reads itself.
- An `auto` argument can be `param(name)`, an `i64` (`12L`, because `12` would be `i32`), an `f64`, a bool or a string.
- `use prelude::{...}` copies the named functions plus everything they call. You still must list all of them in the unit's `helpers`.

### 4.5 Functions and statements

```text
fn name(x: f64, y: f64) -> f64 {
    vars { r: f64 }              // optional: declare variables up front
    let t: f64 = (x + y);        // or declare in place
    if (t < 0.0) { r = -t; } else if (t > 1.0) { r = 1.0; } else { r = t; }
    repeat 3 { r = (r * 0.5); }  // fixed count, a plain non-negative integer
    while (r > 1.0) { r = (r / 2.0); break; }
    return r;
}
```

- Every simple statement ends with `;`.
- `let x: T = e;` declares and assigns. `let x: T;` only declares. A `let` **must** have a type: there is no inference.
- Variables live in one flat namespace per function. Blocks don't scope, and a name can't be declared twice.
- Parameters may be untyped, `fn f(a, b)`, if they are declared in `vars { }`.
- `if` and `while` conditions take no mandatory parentheses.
- There are no loops except `while` and `repeat N`, and `break` leaves the nearest loop.

### 4.6 Expressions

| Form | Meaning |
| --- | --- |
| `x` | A variable. A bare name that isn't a variable falls back to a constant |
| `const(c)`, `host(h)`, `auto(a)` | Constant, host input, autoconst |
| `density(d)`, `density(d, i)` | Read of an input at the current cell, or at flat index `i` |
| `ambient(origin \| pos3 \| origin_scale \| position_scale)` | Where the function is running |
| `f(a, b)` | Call to a program function |
| `ext name(a, b)` | Call to a registered extern. The signature comes from the registry |
| `ext name::<(T, T) -> R>(a, b)` | Extern call with the signature spelled out |
| `e.x`, `e.x::<T>` | Field access. `x/y/z` on `vec3` give `f64`, on `pos3` give `i32` |
| `e[i]` | Array, `vec3` or `pos3` index |
| `vec3(a, b, c)`, `pos3(a, b, c)` | Construct |
| `new T { f: e }`, `new T(a, b)` | Construct a record |
| `[a, b, c]` | Array literal |
| `e as T` | Cast |
| `-e`, `!e`, `&e`, `&mut e` | Unary operators |

**Operators and precedence**, tightest first: postfix (`()` `.f` `[]`), unary, `as`, `* /`, `+ -`, `<< >> >>>`, `&`, `^`, `|`, comparisons, `&&`, `||`. All are left-associative except comparisons.

Two restrictions that differ from Rust, on purpose, so Rust and Java can never disagree:

- **Comparisons don't chain.** `a < b < c` is an error. Parenthesize.
- **Shift and bitwise operators don't mix.** `a | b | c` and `a << 1 << 2` are fine. `a & 1 == 0`, `a << 1 + 2` and `a & b | c` are errors. Parenthesize each part.

There are no implicit conversions. `x + 1` with `x: f64` is a type error: write `x + 1.0`, or cast. The one allowed mix is `vec3` with `pos3` (the result is `vec3`).

**Struct ambiguity.** Record construction always starts with `new`, so `if x { … }` is never read as `x { … }`.

**Derivations.** Only these are inferred, and anything else missing is an error:

1. An extern's signature, from the registry.
2. A field's type and base type, from the base expression.
3. `source_hash = 0`.

**Limits.** Expression depth, block depth and type depth are each capped at 256. Deeper input is a diagnostic, not a crash.

### 4.7 Libraries

`use lib::{a, b};` copies functions and constants from a library file (today the testsuite provides `prelude`). The prelude has `lerp`, `smoothstep`, `as_index`, `corner_x/y/z`, and `sin`, `cos`, `exp`, `log`, `pow` written in plain Tungsten.

An imported name that collides with a local name is an error. Library code can't read hosts, inputs or autoconsts. Printed output expands the imports, so a printed file is self-contained.

### 4.8 Errors

Each stage (`lex`, `parse`, `elaborate`) stops at its first error and reports file, line, column and an underline. `parse` never panics, on any input.

### 4.9 The test envelope

Directives go before or after `program { }`:

```text
@fuel(1000)                          // step limit for the interpreter
@tier(smoke)                         // @tier(large) is skipped unless TW_LARGE=1
@cells(sample = 64, seed = 1)        // check only a sample of cells
@knownfail(java, "reason")           // this backend is expected to fail (a pass is an error)
@case "name" origin(0, 0, 0) param(seed = 7) host(scale = 2.0)
@expect "name" top[1, 0, 0] = 1.5    // one cell
@expect "name" top[*] = 0.0          // every cell
@expect "name" top = [1.0, 2.0]      // every cell, x-fastest order
@expect "name" error(OutOfFuel)      // the case should fail
@expect reject(InputNotListed)       // the file should be refused by the checker
```

Expected values may be `vec3(..)`, `pos3(..)`, arrays and records (`new T { f: v }`). `nan` matches any NaN. Floats are compared by bits.

---

## 5. Features in development

These three specs land together, one layer at a time (`specs/combined-rollout.md`):

1. **Model**: the data structures, the text format, the builder API. *Done.*
2. **Interpreter**: validation and reference semantics. *Mappings are in progress, refinement follows.*
3. **Rust**, then **Java**, then **CUDA**: code generation.

Until a backend implements a feature, it **refuses** programs that use it with a `BackendError::Codegen` naming the feature. It never generates wrong code silently. The corpus files for the new features carry `@knownfail(<backend>, "pending: combined-rollout phase N")` notes that are removed as each backend lands.

### 5.1 Density mappings: In development

**What they are.** Today an un-indexed `density(d)` reads the buffer at the reader's flat cell number, which silently reads the wrong cell if the grids differ. A **mapping** is declared on the input and says exactly how the reader's cell maps into the input's buffer:

| Mapping | Text | Use |
| --- | --- | --- |
| 1-to-1 (default) | nothing | The reader and input have equal dims |
| Expand | `@expand(fx, fy, fz)` | A coarse or flat input read from a larger grid |
| Linear interpolation | `@lerp(fx, fy, fz)` | A coarse input smoothed up to a finer grid |

Each factor is a number `n >= 1` or `*`:

- `*` (*repeat*) means the input has size 1 on that axis, so every reader coordinate reads coordinate 0.
- `n` means each input cell covers `n` reader cells on that axis.

```text
// (5,49,5) coarse noise, interpolated up to a (16,384,16) reader
input noise = grid_u @dims(5, 49, 5) @origin_scale(1, 1, 1) @position_scale(4, 8, 4) @lerp(4, 8, 4);

// A flat column of (4,1,4) read from every y level
input col = column @dims(4, 1, 4) @origin_scale(1, 0, 1) @position_scale(4, 1, 4) @expand(4, *, 4);

// A single cell read from any grid
input one = leaf_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @expand(*, *, *);
```

The attribute can go anywhere among the input's attributes, at most once. Entries don't take a mapping.

**Meaning.** With `p` the reader's position, `Expand` reads `buf[flat(p.x / fx, p.y / fy, p.z / fz)]`, with `*` giving `0`.

`Lerp` needs an `f32` or `f64` input. On each axis with factor `n > 1` it takes the cell `p / n` and the next one, and interpolates with fraction `(p % n) / n`. An axis with `*` or `1` is not interpolated and drops out. For all three axes the order is fixed:

```text
lerp(a, b, t) = a + t * (b - a)
x00 = lerp(v000, v100, tx)    x10 = lerp(v010, v110, tx)
x01 = lerp(v001, v101, tx)    x11 = lerp(v011, v111, tx)
y0  = lerp(x00, x10, ty)      y1  = lerp(x01, x11, ty)
result = lerp(y0, y1, tz)
```

Every backend must produce exactly these bits. That means no skipping a lerp when `t == 0` (it changes `-0.0` and NaN results), no FMA, and no reordering of axes.

**Rules the checker enforces:**

| Rule | Error |
| --- | --- |
| Every numeric factor is at least 1 | `MappingBadFactor` |
| `*` only on an axis where the input has size 1 | `MappingRepeatOnWideAxis` |
| `@lerp` only on `f32` or `f64` inputs | `MappingNotFloat` |
| Identity read: reader dims equal input dims | `MappingDimsMismatch` |
| Expand and lerp: the reader's far edge stays inside the input (the high corner counts, even with weight 0) | `MappingOutOfRange` |

These checks run only for units that read the input **un-indexed**. An input read only through `density(d, i)` has no reader rule, because its index expression is its own mapping. An explicit index always ignores the mapping.

**What changes for existing programs.**

- A `(1,1,1)` input read from a larger grid must now say `@expand(*, *, *)`. It generates the same code as before.
- Two inputs can map one buffer in different ways. Declare two `input` items, which share the buffer and its kernel.
- Mapped reads aren't inlined, apart from mappings that are identity on the grid. They are reported as `InlineRejection::MappedRead`.
- From Rust, `InputSpec::mapping(..)` sets it. It is part of the builder's input merge key.

**Where it stands.** The model, text format, builder, validation, and interpreter lowering are in. Rust, Java and CUDA code generation is pending, so a backend that doesn't implement it yet returns `BackendError::Codegen`. The only mappings allowed on every backend are `Identity` and all-`*` expand. Once a backend lands, lerp is generated as folded inline helpers with no runtime support to link.

### 5.2 Workers: Planned (model layer done)

**What it changes.** Today the generated code is one object that owns everything, and a frontend running on several threads clones it per thread. Workers split it in two:

- **`Orchestration`** is global and thread-safe (`Send + Sync`). It is created once from the program's params, builds the autoconsts and never changes afterwards.
- **A worker** belongs to one thread and runs **exactly one entry**. It owns that entry's buffers. To run another entry, make another worker.

```rust
let orch = Orchestration::new(seed);                  // shared across threads
let mut w = orch.final_density_worker();              // one per thread, not Clone
let hc = FinalDensityHostConsts { blend: 0.5, ..Default::default() };
let out: &[f64; 98304] = w.run(origin, &hc);
```

```java
Orchestration orch = Orchestration.create(seed);
Orchestration.FinalDensityWorker w = orch.finalDensityWorker();
double[] out = w.run(x, y, z, hc);
```

Every worker has a typed result and its own `<Entry>HostConsts` holding only the hosts that entry can read. Buffer layout is private to the worker, so later memory optimizations won't change the API.

**Where it stands.** `Interface` already computes the worker layout, and the naming rules exist. No backend generates the worker API yet. This is a **breaking change to generated code** that ships with v0.3. The `.tw` format doesn't change.

### 5.3 Refinement: In development

**What it is.** A worker's `run` computes an entry's whole closure. **Refinement** adds a second call that reruns only part of that closure, shifting `Pos3` each time (marching), so a large volume can be computed one slice at a time instead of as one big eager 3D buffer.

```text
worker.run(origin, hosts)                      // everything, offset 0
worker.refine_step(offset(0, 383, 0), hosts)   // reruns the marked instances
worker.refine_step(offset(0, 382, 0), hosts)   // everything else stays frozen
```

- **Rerun** instances compute again on each call, either as themselves or as a **refined unit** (`with`) that replaces them.
- **Frozen** instances keep the value from the last time they were written.
- A refined unit can read **its own previous value at the current cell** with `@previous`.
- Origin and scales never change between calls. Only `Pos3` moves, and the buffer index stays put.

```text
program {
    // fn definitions omitted

    input prev = seed   @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) @previous;
    input d    = detail @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    input col  = column @dims(4, 1, 4)   @origin_scale(1, 0, 1) @position_scale(4, 1, 4) @expand(4, *, 4);
    input grid = coarse @dims(5, 49, 5)  @origin_scale(1, 1, 1) @position_scale(4, 8, 4) @lerp(4, 8, 4);

    unit surface { main surface_main; inputs [prev, d, col, grid]; }
    entry main = seed @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);

    refinement step of main @march(0, 383, 0) {
        rerun seed   @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1) with surface;
        rerun detail @dims(16, 1, 16) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    }
}
```

Here `run` computes `seed`, `column` and `coarse`. Each `refine_step` computes `detail`, then `surface`, and writes the result into `seed`'s buffer. `column` and `coarse` aren't listed, so they stay frozen and are read through their `@expand` and `@lerp` mappings at the marched position.

**Syntax.**

```text
refinement NAME of ENTRY_LABEL [@march(mx, my, mz)] {
    rerun UNIT <grid> [with REFINED_UNIT];
    ...
}
input NAME = UNIT <grid> [mapping] @previous;
```

- A missing `@march` means no march (all zero). Each `rerun` writes its grid out in full.
- Readers name the **target** (`detail`), whichever unit wrote it last.
- A refined unit has no grid of its own. It runs at its target's dims and scales.

**Rules the checker enforces** (each is a `LowerErrorKind`, so `@expect reject(Kind)` can name it):

| Rule | Error |
| --- | --- |
| The entry exists and refinement names are unique | `RefineBadEntry`, `RefineDuplicateName` |
| `march` components are `>= 0` | `RefineBadMarch` |
| `reruns` contains the entry's own instance, with no duplicates | `RefineEntryNotRerun`, `RefineDuplicateRerun` |
| Every rerun is reachable from the entry through other reruns | `RefineRerunUnreachable` |
| A `with` unit returns the target's type | `RefineTypeMismatch` |
| Targets have equal position scales on every marched axis | `RefineMarchScaleMismatch` |
| The instances that run form no cycle (ignoring `@previous`) | `RefineCycle` |
| A `with` unit isn't an entry and no normal unit reads it | `RefinedUnitInNormalGraph` |
| `@previous` is only listed by `with` units, and its unit, dims and scales equal the rerun's target | `PreviousOutsideRefinedUnit`, `PreviousNotTarget` |
| A `@previous` input has an identity mapping and is never read with an index | `PreviousMapped`, `PreviousIndexed` |

Offsets and call order are caller preconditions with no generated checks: call `refine_*` after `run`, and keep every offset component in `[0, march]`.

**Hosts.** A frozen buffer holds what was written last, including the hosts of that write. Hosts are the way to pass step-dependent values such as a step count.

**Testing refinement.** The planned envelope adds `@refine "case" name offset(x, y, z) host(...)` and `@expect "case" after(n) ...` to drive and check a sequence of steps. These aren't in the parser yet.

**Optimizer.** The inliner won't inline a `with` unit, a unit listed in any `reruns`, or one read directly by a running unit, because that would recompute a frozen value on every step.

**Where it stands.** The data model (`Refinement`, `Rerun`, `DensityInput::previous`), text format for `refinement`, `@march`, `rerun` and `@previous`, builder API (`pb.refinement`, `RerunSpec`, `InputSpec::previous`), and the validation rules above are in. The interpreter doesn't yet run refine steps (the `@refine` / `after(n)` test directives and the sequence driver are still to do), and no backend generates refine calls.

### 5.4 Smaller changes that are already in

These landed earlier on the way to v0.3:

- **Program-level inputs.** Inputs live in one program arena and units list the ones they use. Several units can share an input, and a helper shared between units can read it.
- **Generic autoconsts.** The IR has no knowledge of noise tables. A frontend declares `param`s and `auto`s with runtime initializer functions, and the program only reads them.
- **`Compiler` pipeline.** One backend per compile, typed output, passes run by the compiler and not by backends.

---

## 6. Quick reference

### Common mistakes

| Symptom | Likely cause |
| --- | --- |
| `helper_not_listed`, `input_not_listed` | The unit's lists must include everything its helpers reach, including library functions pulled in by `use` |
| Type error on `x * 2` | Write `x * 2.0`. There are no implicit conversions |
| Parse error on `a & 1 == 0` | Parenthesize: `(a & 1) == 0` |
| `MappingDimsMismatch` | The input has different dims from the reader. Add `@expand` or `@lerp`, or fix the grid |
| A `(1,1,1)` input read from a bigger grid is rejected | Add `@expand(*, *, *)` |
| Literal out of range | `3000000000` needs `L`, and `1e999` needs `inf` |
| Backend says a feature isn't implemented | Mappings and refinement are not generated by that backend yet (§5) |

### Where to look when something fails

1. A diagnostic with a file, line and underline: a text problem. Read §4.
2. `CompileError::Invalid`: the program failed lowering or the core checker. The error kind is a `LowerErrorKind` or a core checker kind.
3. `BackendError::Codegen`: the backend can't emit something, and the message names the feature.
4. A test failing only on one backend: compare with the interpreter's value in hex. Real gaps go in `PAPER.md` with a narrow `@knownfail` (`docs/testing-policy.md`).
