# What a core program means

This document explains what a core IR program *computes*: what its values are, what each
expression and statement does, in what order things happen, and what can go wrong. It is
written for people. It doesn't assume you have read the code.

The interpreter in `src/exec/` is the reference: when this document and the interpreter
disagree, the interpreter wins and this document has a bug. How the interpreter is built is in
[EXEC.md](EXEC.md). Which programs are allowed to run at all is in [CHECK.md](CHECK.md). Why the
code looks the way it does (Verus) is in [VERUS.md](VERUS.md).

Arithmetic follows **Java**. That's on purpose: the Java backend should agree with the
interpreter bit for bit, and the other backends are tested against the same rules.

---

## 1. The big picture

A tungsten program describes **density functions**: functions that give a number (or some other
value) for every cell of a 3D grid. Minecraft world generation evaluates them over a block of
the world, for example "how dense is the terrain at each of these 4×4×4 positions?".

The pieces, from the outside in:

- An **entry** is what the caller actually asks for: "evaluate unit `U` over a grid of
  `dims` cells, with these scales". The result of an entry is one value (or one error) per cell.
- A **unit** is one density function. It has a `main` function that takes no arguments and
  returns the value of one cell. It also says which helpers, constants, tables, hosts and
  density inputs it uses.
- A **density input** lets one unit read another unit's results. Unit `A` declares "I need
  unit `B` evaluated over *this* grid", and `A`'s functions can then read values out of that
  grid.
- **Functions** have parameters, locals, a return type and a body of statements.
- **Constants** are values computed once, before anything else.
- **Hosts** are values the caller passes in (for example a seed or a setting).
- **Tables** are opaque handles to permutation tables and noise samplers. Their contents live
  outside the program; the program only passes the handles to externs.
- **Externs** are functions implemented outside the program (`sin`, `sqrt`, noise sampling, …).

A program plus one **case** gives one result. A case is the world-space origin plus a value for
every host:

```
Case { origin: (x, y, z), hosts: [value for host 0, value for host 1, …] }
```

The result of a case is, for every entry, a list with one `Ok(value)` or `Err(error)` per cell,
plus a list of diagnostics (which constants and inputs failed).

```
         ┌──────────── case: origin, hosts ────────────┐
         ▼                                             ▼
  constants (all, once)  ──▶  entry 0: unit U over dims  ──▶  [cell 0, cell 1, …]
                              entry 1: unit V over dims  ──▶  [cell 0, cell 1, …]
                                    │
                                    └─ before U's cells run, every density input U
                                       declares is computed over its own grid
```

### Everything is plain data

There are no names in the core IR. Everything refers to everything else by position:
`FnId(3)` is the fourth function, `HostId(0)` the first host, and `LocalId(2)` the third local
of the function you're in. Names only exist in `tungsten-check`, which uses them to print
errors.

---

## 2. Values and types

| Type | Values | Notes |
| --- | --- | --- |
| `Bool` | `true`, `false` | |
| `I8`, `I32`, `I64` | signed integers of that width | Arithmetic wraps around (see §6) |
| `F32`, `F64` | IEEE-754 floats | Stored as their raw bits. `NaN` keeps its payload, `-0.0` and `0.0` are different values |
| `Vec3` | three `f64`s | A position or direction in world space |
| `Pos3` | three `i32`s | A cell position in a grid |
| `Array(T, n)` | exactly `n` values of type `T` | The length is part of the type |
| `Record(r)` | one value per field of record `r`, in declaration order | Record layouts come from the registry (`Program.records`) |
| `Handle(ty)` | an opaque handle to a runtime object of handle type `ty` | Only passed to externs |

There is no type for "a density input" or "a function". You can only *read* a density input
(with a `Density` expression) and *call* a function. Neither can be stored in a variable.

### Why floats are bits

Floats are stored as their bit patterns (`u32` for `f32`, `u64` for `f64`). Two consequences:

- Two values are **equal as values** exactly when their bits are equal. So `NaN` equals the same
  `NaN`, and `-0.0` is not the same value as `0.0`. This is what the test harness uses to compare
  results.
- The program's own `==` operator is **IEEE equality**, which is different: `NaN == NaN` is
  `false` and `-0.0 == 0.0` is `true`. See §6.

---

## 3. Where a cell is: the context

Every cell is evaluated in a **context** that the program can read through `Ambient`
expressions:

| Ambient | Type | Value |
| --- | --- | --- |
| `Origin` | `Vec3` | The case's origin. The same for every cell, every entry and every input |
| `OriginScale` | `Vec3` | The `origin_scale` of the entry (or density input) being evaluated |
| `PositionScale` | `Vec3` | The `position_scale` of the entry (or density input) being evaluated |
| `Pos3` | `Pos3` | The cell's integer position `(x, y, z)` inside its grid, each from `0` to `dims - 1` |

The core IR doesn't compute world positions itself. A world position like
`origin * origin_scale + pos3 * position_scale` is ordinary program code that combines these
ambients. The interpreter only provides the raw ingredients.

### Grid layout

A grid of `dims = (dx, dy, dz)` has `dx * dy * dz` cells. Cells are numbered in **flat order**
with **x varying fastest**:

```
flat(x, y, z) = x + y*dx + z*dx*dy
```

So for a `3×5×2` grid, cell `(1,0,0)` is number 1, `(0,1,0)` is number 3 and `(0,0,1)` is
number 15. Results come back in this order, density input buffers are stored in this order,
and cells are evaluated in this order.

---

## 4. Evaluating a case

Evaluation happens in a fixed order. The order matters for which error you see when several
things fail (§10), but never for the values you get when nothing fails.

1. **All constants**, in id order (`ConstId(0)`, `ConstId(1)`, …), whether anything reads them or
   not.
2. **Each entry**, in order. For entry `e` with unit `U`:
   1. **Every density input `U` declares** is computed over its whole grid (§9), in the order
      `U` lists them.
   2. **Every cell** of the entry's grid, in flat order: call `U.main()` with the cell's
      context. Its return value is the cell's value.

### Constants are eager and global

All constants are evaluated before any entry, even constants nobody reads. If **any** constant
fails, **every cell of every entry** fails with the error of the lowest-numbered failing
constant. Every failing constant is listed in the diagnostics.

*Why:* backends emit constants as items that are computed when the generated code starts (or is
built), whether or not they are read. A failing constant fails the whole backend run, so the
interpreter must fail the whole case too.

### Density inputs are eager too

Before any cell of a unit runs, every density input the unit declares is computed over **all**
of its cells, even inputs that are never read and cells that are never read. If any of them
fails, every cell of the unit (in that context) fails. See §9.

*Why:* a backend computes every cell of every scheduled input buffer up front. If one of those
cells fails, the backend fails, even if no-one reads that cell. A lazy interpreter would say
`Ok` where every backend fails.

### Each cell is independent

Inside a cell, the first error ends that cell. Other cells carry on. A cell's result is either
a value or exactly one error.

---

## 5. Expressions

Expressions are evaluated **left to right**, and the first error stops the expression (and the
cell). The table gives what each expression does. "Evaluates `a`, then `b`" is the order
guarantee.

| Expression | Meaning |
| --- | --- |
| `Lit(l)` | The literal's value |
| `Local(l)` | The current value of local `l`. Reading a local that hasn't been assigned yet is `Stuck(Unassigned)` (the checker rules this out) |
| `Const(c)` | The value of constant `c` (computed once per case, §8) |
| `Host(h)` | The case's value for host `h` |
| `Table(t)` | A `Handle(t)` value, typed `Handle(ty)` with the table's handle type |
| `Ambient(a)` | The context value (§3) |
| `Density { input, index }` | A value read out of a density input's buffer (§9) |
| `Call { func, args }` | Evaluates the arguments left to right, then calls `func` with them (§7) |
| `ExternCall { ext, args }` | Evaluates the arguments left to right, then calls the extern (§11) |
| `Binary { op, left, right }` | Evaluates `left`, then `right`, then applies `op` (§6). Exception: `And` and `Or` short-circuit |
| `Unary { op, operand }` | Evaluates the operand, then applies `op` (§6) |
| `Field { base, sel }` | Evaluates `base`, then picks a component: `.x/.y/.z` of a `Vec3` (an `f64`), of a `Pos3` (an `i32`), or field `i` of a record |
| `Index { array, index }` | Evaluates `array`, then `index` (an `i32`). `index < 0` or `index >= n` is `OutOfBounds` |
| `MakeVec3 { x, y, z }` | Evaluates `x`, `y`, `z` in order and builds a `Vec3` |
| `MakePos3 { x, y, z }` | Evaluates `x`, `y`, `z` in order and builds a `Pos3` |
| `MakeRecord { record, fields }` | Evaluates the field values **in the order they are written**, then stores each in its declared slot. The record value is always in declaration order |
| `ArrayLit(es)` | Evaluates the elements in order and builds an array |
| `Cast { to, operand }` | Evaluates the operand, then converts it (§6.5) |

A few things worth spelling out:

- **Short-circuit.** `a && b` doesn't evaluate `b` when `a` is `false`, and `a || b` doesn't
  evaluate `b` when `a` is `true`. So `false && (1 / 0 == 0)` is `false`, not `DivByZero`. The
  bitwise operators `&` and `|` on `Bool` are **not** short-circuit: both sides are always
  evaluated.
- **Record construction order.** `new Pair { b: f(), a: g() }` calls `f` before `g`, because
  that's the written order. The resulting value still stores `a` first, because that's the
  declaration order.
- **Annotations don't change meaning.** `ExternCall` carries the parameter and return types the
  call site was written with, and `Field` carries the type the field was annotated with. The
  checker compares them with the real types. At run time they are ignored.

---

## 6. Operators

### 6.1 Integers (`I8`, `I32`, `I64`)

Both operands have the same type (shifts excepted). Everything follows Java:

| Operator | Meaning |
| --- | --- |
| `+ - *` | Wrap around on overflow: `i32::MAX + 1 == i32::MIN` |
| `/` | Truncates toward zero: `-7 / 2 == -3`. Dividing by zero is `DivByZero`. `MIN / -1 == MIN` (it wraps instead of failing) |
| `== != < <= > >=` | The usual comparisons, giving a `Bool` |
| `& \| ^` | Bitwise and, or, xor |
| `<< >> >>>` | Shift left, arithmetic shift right (copies the sign bit), logical shift right (shifts in zeros). The right operand is always an `I32`. Only its low 5 bits count for `i8`/`i32` and its low 6 bits for `i64`, so `1 << 33 == 2` for `i32` |
| unary `-` | Negation, wrapping: `-i32::MIN == i32::MIN` |
| unary `!` | Bitwise not |

**`I8` works like Java's `byte`:** the operands are widened to `i32`, the `i32` operation runs,
and the result is cut back to 8 bits. So `127 + 1 == -128` and `16 * 16 == 0`. This matters for
`>>>`: `(byte)-8 >>> 1` widens `-8` to the 32-bit `0xFFFFFFF8`, shifts that to `0x7FFFFFFC`, and
truncates to `-4`. It does **not** give `124`, which an 8-bit logical shift would.

### 6.2 Floats (`F32`, `F64`)

| Operator | Meaning |
| --- | --- |
| `+ - * /` | IEEE-754, rounded to the operand type. Division by zero is **not** an error: `1.0 / 0.0 == +∞`, `0.0 / 0.0 == NaN` |
| `==` / `!=` | IEEE equality: `NaN == x` is always `false`, `0.0 == -0.0` is `true`. `!=` is exactly `!(a == b)`, so `NaN != NaN` is `true` |
| `< <= > >=` | IEEE ordering: any comparison with `NaN` is `false` |
| unary `-` | Flips the sign bit: `-(0.0) == -0.0` |

There are no bitwise operators or shifts on floats.

### 6.3 Booleans

| Operator | Meaning |
| --- | --- |
| `&&` / `\|\|` | Logical and/or, short-circuit (§5) |
| `&` / `\|` / `^` | Logical and/or/xor, both sides always evaluated |
| `==` / `!=` | Equality |
| unary `!` | Not |

Booleans can't be ordered (`true < false` is a type error).

### 6.4 Vectors (`Vec3`, `Pos3`)

`+ - * /` work **componentwise**:

| Left | Right | Result | How |
| --- | --- | --- | --- |
| `Vec3` | `Vec3` | `Vec3` | `f64` operation per component |
| `Pos3` | `Pos3` | `Pos3` | `i32` operation per component (wrapping). For `/`, the components are divided x, then y, then z; the first zero divisor is `DivByZero` |
| `Vec3` | `Pos3` | `Vec3` | The `Pos3` is converted to `f64` exactly, then `f64` operations |
| `Pos3` | `Vec3` | `Vec3` | Same, the other way round |

Unary `-` negates every component (wrapping for `Pos3`). There is no scalar broadcast
(`vec * 2.0` is a type error; write `vec * MakeVec3(2.0, 2.0, 2.0)`), and vectors can't be
compared.

### 6.5 Casts

Integers are converted through `i64`, which holds every `i8` and `i32` exactly.

| From | To | Rule |
| --- | --- | --- |
| any integer | wider integer | Exact (sign-extended) |
| any integer | narrower integer | Keeps the low bits: `(byte)300 == 44`, `(int)(1L << 32) == 0` |
| any integer | `F32` / `F64` | Rounds to the nearest float (ties to even): `(float)16777217 == 16777216.0` |
| `F32` / `F64` | `I32` / `I64` | Rounds toward zero, saturates at the type's min/max, and `NaN` becomes `0`: `(int)-2.9 == -2`, `(int)3e10 == i32::MAX`, `(int)∞ == i32::MAX` |
| `F32` / `F64` | `I8` | Like Java's `(byte)f`: first convert to `i32` (saturating), then keep the low 8 bits. So `(byte)300.0 == 44` and `(byte)1e10 == (byte)i32::MAX == -1` |
| `F32` | `F64` | Exact |
| `F64` | `F32` | Rounds to nearest (ties to even). Signs, infinities and `NaN` are kept |
| `Bool` | any integer | `true → 1`, `false → 0` |
| `Pos3` | `Vec3` | Each component converted to `f64` exactly |
| `T` | the same numeric `T` | Identity |

Nothing else can be cast (no `Bool → float`, no `float → Bool`, no `Vec3 → Pos3`).

---

## 7. Functions and statements

### 7.1 Calls

A function has `params` parameters and a list of `locals`. The first `params` locals **are**
the parameters. Calling a function:

1. creates a fresh set of locals, the first `params` filled with the argument values and the
   rest **unassigned**;
2. runs the body;
3. returns the value of the first `return` it reaches.

Arguments are passed **by value**: the callee gets its own copy, and assigning to a parameter
doesn't affect the caller. There are no references, no globals you can write to, and no side
effects except through externs (which are expected to be pure, see §11).

If the body ends without a `return`, that's `Stuck(NoReturn)`. If a `break` escapes to the
function body, that's `Stuck(BreakOutsideLoop)`. The checker rules both out.

### 7.2 Statements

| Statement | Meaning |
| --- | --- |
| `Assign { target, value }` | Evaluates `value`, then stores it in local `target` |
| `Return(e)` | Evaluates `e` and leaves the function with it |
| `If { cond, then, else }` | Evaluates `cond` (a `Bool`) and runs one branch |
| `While { cond, body }` | Loops: evaluate `cond`; if `false`, stop; otherwise run `body` and repeat. Limited by **fuel** (§7.3) |
| `Repeat { count, body }` | Runs `body` exactly `count` times. `count` is a fixed number written in the program. `repeat 0` never runs the body. No fuel is used |
| `Break` | Leaves the innermost enclosing `While` or `Repeat`; execution continues after that loop |

A `return` inside a loop leaves the function, not just the loop.

### 7.3 Fuel

`While` loops could run forever, and the interpreter has to stop. So every evaluation gets a
**fuel** number `f`, and **each execution** of a `While` statement may run its body at most `f`
times. If the condition is still `true` after `f` iterations, the loop fails with `OutOfFuel`.
In detail, one iteration is:

```
evaluate cond
if cond is false:            the loop is done (normal exit)
if f iterations already ran: OutOfFuel
run body                     (break → done, return → leave the function)
```

So with fuel `f`, a loop can run its body `f` times and then still exit normally if its condition
becomes `false`. With fuel `0`, any `While` whose condition is `true` the first time fails.

Things to know about fuel:

- The limit is **per loop execution**, not a shared budget. A loop that runs twice (because its
  function is called twice) gets `f` iterations each time. Nested loops each get their own `f`
  per execution.
- `Repeat` never uses fuel. Its iteration count is fixed in the program, so it can't run
  forever, and charging fuel would make a harmless `repeat 1000` fail at fuel 100.
- `tungsten-check` uses `DEFAULT_FUEL = 2^20` unless you pass something else.

*Why per loop:* a shared budget would make results depend on how much work was done before,
including work that the memo (EXEC.md) skips. Per-loop fuel means a cell's value depends only on
the program, the case and `f`, and it gives a simple monotonicity property: if a cell's result
isn't `OutOfFuel` at fuel `f`, it is the same at every larger fuel.

---

## 8. Constants

A constant is a name for an expression (`init`) that is evaluated **once per case**, before any
entry, in id order. A constant's initializer can use literals, operators, casts, field access,
construction, arrays, and other constants. It can't read locals, hosts, tables, ambients or
density inputs, and it can't call functions or externs: a constant is a pure function of the
program alone. (Those are checker rules; see CHECK.md.)

A constant has no declared type. Its type is whatever its initializer produces.

Constants can read other constants, but not in a cycle: `const a = b + 1; const b = a * 2;` is
rejected by the checker.

Reading a constant from a function just gives the value computed at the start of the case.

---

## 9. Density inputs

### 9.1 What an input is

A density input `i` is declared once for the whole program, and any unit can list it. It says:

- `source`: which unit to evaluate,
- `dims`: over which grid,
- `origin_scale`, `position_scale`: with which scales (the origin is always the case's origin).

**Building** the input means evaluating the source unit over all cells of that grid, giving a
**buffer**: one value per cell, in flat order. If any cell fails, the whole buffer fails with
the error of its **first failing cell in flat order**.

Building the source unit is itself a full unit evaluation, so the source's own density inputs
are built first, and so on down the chain. The checker makes sure this chain ends (no unit can
read itself through inputs, CHECK.md §7).

The buffer depends only on the input, never on who reads it. So each buffer is built at most once
per case, and every reader sees the same values.

### 9.2 When inputs are built

Before any cell of unit `U` runs in some context (as an entry, or as the source of another
input), **every** input that `U` lists is built, in list order. If one or more fail, every cell
of `U` in this context fails with the error of the failing input with the **lowest
`InputId`** (not the first in the list). Every failing input is listed in the case's diagnostics
once, with the cell that failed.

### 9.3 Reading an input

A function running as part of unit `U` can read an input only if `U` lists it. A function listed by
several units is checked against each of them. Reading an input the unit doesn't list is
`InputNotListed` (CHECK.md §6(e)). An input that no unit lists is legal and inert: it is never built,
so a failing source has no effect.

The index rules:

- **`Density(i)` without an index** reads "the matching cell":
  - if the input's grid is a single cell `(1,1,1)`, it reads that cell, from any reader cell;
  - otherwise it reads the cell with the **same flat number** as the reader's current cell. If
    the reader's grid has more cells than the input, the reader cells past the end get
    `OutOfBounds`.
- **`Density(i, index)`** with an `i32` index reads buffer cell `index` directly.
  `index < 0` or `index >= cells` is `OutOfBounds`. The index expression is evaluated first.

Watch out: the implicit read matches by **flat number, not by position**. When the reader and
the input have the same dims, flat number `k` is the same `(x, y, z)` in both and it does what
you'd expect. When the dims differ in shape (say `4×1×4` against `2×2×4`), cell `k` of the reader
and cell `k` of the input are different positions. Use an explicit index if you need something
else.

---

## 10. Errors

### 10.1 The kinds

| Error | When |
| --- | --- |
| `DivByZero` | Integer (or `Pos3`) division by zero. Never for floats |
| `OutOfBounds` | Array index or density-input index outside its range |
| `OutOfFuel` | A `While` ran out of fuel (§7.3) |
| `ExternDomain` | An extern declared with `domain_error: true` reported a domain error (e.g. `sqrt(-1)`) |
| `Stuck(reason)` | Something that a checked program can never do. See §10.2 |

The first four are **real outcomes**: a perfectly valid program can divide by zero for some
inputs. Backends must fail in those cases too (§10.4).

### 10.2 Stuck

`Stuck` means "this program shouldn't have been allowed to run". It covers broken ids, values of
the wrong shape, unassigned locals, missing returns, a cycle, an extern that broke its contract,
and so on:

| Reason | Means |
| --- | --- |
| `BadIndex` | An id points past its table |
| `Rank` | A call or read that could recurse forever (a dependency cycle) |
| `Shape` | A value of the wrong kind (e.g. adding a `Bool` to an `f64`) |
| `Unassigned` | A local read before it was assigned |
| `NoReturn` | A function ended without `return` |
| `BreakOutsideLoop` | A `break` escaped a function body |
| `ExternResult` | An extern returned a value that doesn't match its declared return type |
| `ExternDomainUndeclared` | An extern reported a domain error but isn't declared to have one |
| `MainParams` | A unit's main function has parameters |
| `Layout` | A grid whose size can't be computed (negative dims or overflow) |
| `Case` | The case doesn't have a value for some host |

The interpreter doesn't trust the checker: it checks all of this as it goes and reports `Stuck`
instead of crashing. But for a program that passes the checker and a case that passes
`check_case`, `Stuck` never happens. This is the **type soundness** property. For now it is
tested (random mutations of real programs, see TESTING.md), and later it will be proven.
`tungsten-check`'s `Prepared::eval` turns any `Stuck` into an error of its own, because it always
means a bug in the checker or the interpreter.

`Stuck` is still a per-cell result, not an abort of the whole case. One stuck cell doesn't stop
the other cells.

### 10.3 Which error you get

When several things fail, the error you see is always the same one: the interpreter's choice is
**canonical**, fixed by the semantics rather than by some evaluation schedule. In order of
priority:

1. The **lowest failing constant** (it fails every cell of every entry).
2. Then, for a unit, the **lowest failing `InputId`** among the inputs it lists (it fails every
   cell of that unit in that context).
3. Then, for an input buffer, its **first failing cell in flat order**.
4. Then, inside a cell, the **first error left to right** (§5).

### 10.4 What this means for backends

Backends (Rust, Java, CUDA) may schedule work in any order they like. That's why errors are
compared coarsely. Per case:

- If the interpreter reports **no error anywhere** (no failing cell, no failing constant or
  input), every backend must produce **exactly** the same values, bit for bit. Two `NaN`s count
  as equal.
- If the interpreter reports **any** error, the backend must fail the case somehow (an error, a
  panic, an exception, a watchdog). Which error it reports is shown, but never compared, because
  a backend that computes in a different order can legitimately hit a different error first.

---

## 11. Externs

An extern call evaluates its arguments, then hands the extern id and the argument values to the
caller-supplied `Externs` implementation. The implementation answers with a value or with a
domain error.

- The result value must match the extern's declared return type (`ExternDecl.ret`), or the call is
  `Stuck(ExternResult)`.
- A domain error becomes `ExternDomain` if the extern is declared with `domain_error: true`, and
  `Stuck(ExternDomainUndeclared)` otherwise.

Externs are assumed to be **pure**: the same id and arguments always give the same answer. The
interpreter may call an extern any number of times (with the memo off, it recomputes
everything), so an extern with side effects would give results that depend on that.

Tables (`Handle` values) are only handles. Sampling noise or looking up a permutation is
an ordinary extern call that receives the handle as an argument. The `Externs` implementation
decides what each handle means (usually through the table's name and seed in `tungsten-check`'s
`LowerMap`).

---

## 12. Guarantees, in one place

- **Determinism.** The result of a case is a function of `(program, case, fuel)` and the
  externs. Nothing else matters: not the memo, not the scheduling that backends use, not the
  order in which things happen to be cached.
- **Memo independence.** Evaluating with the memo on or off gives identical results (tested).
- **Fuel monotonicity.** A cell whose result isn't `OutOfFuel` at fuel `f` has the same result
  at every fuel `≥ f` (tested, proof planned).
- **No stuck states for checked programs.** A program that passes `check`, with a case that
  passes `check_case`, never produces `Stuck` (tested, proof planned).
- **Termination.** Evaluation always ends: `While` is bounded by fuel, everything else by the
  program's structure and by the ranks (EXEC.md §3). Verus proves this for the interpreter as it
  is written.
