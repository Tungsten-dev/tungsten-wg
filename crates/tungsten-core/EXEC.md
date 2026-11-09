# The interpreter, function by function

This document walks through the interpreter in `src/exec/` (plus the helpers it relies on in
`layout.rs`, `value.rs` and `prim.rs`). For each function it says what it does, what it
returns, how it fails and how it is guaranteed to terminate.

What the interpreter *computes* is described without reference to the code in
[SEMANTICS.md](SEMANTICS.md). Read that first if you want the meaning. Read this if you want to
change the code or understand why it is shaped the way it is. The Verus background is in
[VERUS.md](VERUS.md).

---

## 1. The shape of the interpreter

```
eval_program                       exec/mod.rs     consts, then entries, then diagnostics
 ├─ eval_const (every const)       exec/unit.rs
 └─ eval_entry (every entry)       exec/unit.rs
     └─ eval_unit_ctx              exec/unit.rs    build U's inputs, then every cell
         ├─ build_input            exec/unit.rs    ── eval_unit_ctx (source unit, its own grid)
         └─ eval_cell              exec/unit.rs
             └─ eval_call (main)   exec/stmt.rs
                 └─ eval_stmts / eval_stmt         exec/stmt.rs
                     └─ eval_expr                  exec/expr.rs
                         ├─ eval_args
                         ├─ read_density ── build_input
                         ├─ eval_const
                         ├─ eval_call
                         └─ binop / unop / field / cast ── prim.rs
```

It is a straightforward **tree-walking interpreter**: one Rust function per kind of thing
(program, entry, unit, cell, call, statement, expression), each calling the next.

What makes it a bit unusual is that it lives inside `verus!{}` and Verus proves that **every one
of these functions terminates and never indexes out of bounds or overflows**. That explains most
of the oddities you'll see:

- no closures, iterators, `?` on `Option`, or `HashMap`: plain `while` loops and `match`;
- `util::get(&v, i)` instead of `v[i]` whenever `i` is an id from the program;
- an extra `rank: usize` argument on most functions;
- every recursive function has a `decreases` clause.

VERUS.md explains each of these.

The interpreter **never trusts the checker**. Everything the checker guarantees is checked again
at run time, and a violation becomes `Err(Stuck(reason))` instead of a panic. That makes the
interpreter safe on any input, including hand-built and randomly mutated programs, which is what
the "no stuck" property tests rely on.

---

## 2. The data it works with

### `Env` — what never changes during a case

```rust
pub struct Env<'a, E: Externs> {
    pub p: &'a Program,
    pub ranks: &'a Ranks,
    pub case: &'a Case,
    pub ext: &'a E,
    pub fuel: u64,
}
```

Bundled into one struct so that every function doesn't take five extra arguments. `ranks` is
computed by `eval_program` itself from the program (§3). `E` is generic, not `&dyn Externs`,
because Verus has no trait objects.

### `Ctx` — where the current cell is

```rust
pub struct Ctx {
    pub origin: F64x3,          // always the case's origin
    pub origin_scale: F64x3,    // from the entry or input being evaluated
    pub position_scale: F64x3,  // likewise
    pub dims: Dims,             // the grid being evaluated
    pub k: usize,               // the cell's flat index
    pub cell: I32x3,            // decode(k, dims)
}
```

`Ambient` expressions read straight out of this. `k` is kept next to `cell` so that implicit
density reads (`buf[k]`) never need to re-encode the position.

### `Frame` — one call's locals

```rust
pub struct Frame { pub locals: Vec<Option<Value>> }
```

One slot per local of the function. `None` means "not assigned yet". Parameters start as
`Some(argument)`, everything else as `None`. Reading a `None` slot is `Stuck(Unassigned)`.

### `Flow` — how a statement ended

```rust
pub enum Flow { Next, Break, Return(Value) }
```

`Next` means "carry on with the next statement". `Break` and `Return` travel outwards until a
loop or the function body handles them. Errors travel separately, as the `Err` side of the
`Result`.

### `Memo` — the cache, one per case

```rust
pub struct Memo {
    pub enabled: bool,
    pub consts: Vec<Option<Result<Value, EvalError>>>,               // by ConstId
    pub inputs: Vec<Option<Result<Vec<Value>, (usize, EvalError)>>>, // by InputId
    pub listed: Vec<bool>,                                           // by InputId
    pub failures: Vec<Failure>,
}
```

- `consts[c]` caches constant `c`'s result (value or error).
- `inputs[i]` caches input `i`'s whole buffer, or `(first failing cell, error)`.
- `listed[i]` remembers whether input `i`'s failure is already in `failures`, so each failing
  input is listed once. This is kept even with the memo off, so diagnostics don't depend on it.
- `failures` collects failing inputs in the order they were discovered.

With `enabled == false` nothing is cached and every read recomputes from scratch. That mode
exists only for tests: results must be identical either way. It can be **very** slow, because
every density read rebuilds the whole buffer, recursively. Use tiny grids.

The cache is sound because a constant's or an input's result depends only on
`(program, case, fuel, id)`. Nothing about *who* asks or *when* changes it: ranks come from the
program alone, and fuel is per loop.

Create one with `new_memo(p, enabled)`.

### `CaseResult` and `Failure` — what comes out

```rust
pub struct CaseResult {
    pub entries: Vec<Vec<Result<Value, EvalError>>>, // per entry, per cell in flat order
    pub failures: Vec<Failure>,                      // failing consts, then failing inputs
}
pub enum Failure {
    Const { id: ConstId, error: EvalError },
    Input { id: InputId, cell: usize, error: EvalError },
}
```

`failures` is diagnostics: it tells you *why* cells failed when the reason is a constant or an
input, which the cell's error alone doesn't show.

---

## 3. Termination: ranks and layers

The interpreter calls itself in many ways: functions call functions, expressions read
constants, reading an input builds it by evaluating another unit, which evaluates functions…
Verus has to be convinced that this always ends. The argument has three parts, compared in
order (a **lexicographic measure**):

1. **Rank.** `check::acyclic::ranks(p)` gives every function, constant, input and unit a rank,
   or none if it is on or depends on a cycle (CHECK.md §7). Every edge in the dependency graph
   goes to a strictly lower rank. Each evaluation function gets the rank of what it is working
   on, and before it calls `eval_call`, `eval_const`, `build_input` or `eval_unit_ctx` on
   something else, it checks **`target rank < current rank`**. If that fails (or the target has
   no rank), the result is `Stuck(Rank)`. Since the rank goes down at every such step, there can
   only be finitely many.
2. **Layer.** Several functions run at the *same* rank (a function's `eval_call` calls
   `eval_stmts`, which calls `eval_expr`, all for the same function). These are ordered by a
   fixed layer number:

   | Layer | Functions |
   | --- | --- |
   | 5 | `build_input` |
   | 4 | `eval_unit_ctx` |
   | 3 | `eval_cell` |
   | 2 | `eval_call`, `eval_const` |
   | 1 | `eval_stmts`, `eval_stmt` |
   | 0 | `eval_expr`, `eval_args`, `read_density` |

   Going from a statement to one of its expressions drops from layer 1 to layer 0. (Verus can't
   compare the size of a `Stmt` with the size of an `Expr`, so the layer does that job.)
3. **Structure.** Inside one layer, the function recurses into a smaller piece of the same
   tree: a sub-expression, a sub-statement, a child list.

Loops have their own bound: `While` runs at most `fuel` iterations, `Repeat` exactly `count`, and
loops over children run up to the child count.

### Why ranks and not a depth counter

The obvious alternative is a "depth" counter that goes down at every call. But then the result
of a cached constant or buffer could depend on the depth at which it was first computed, and a
later, shallower read would reuse a result computed under a tighter budget. The memo would no
longer be invisible. Ranks are a property of the program alone, so it doesn't matter who asks.

### Why the rank comparison is written out inline

Each call site reads `if r < rank { call(…, r, …) } else { Stuck(Rank) }` directly. A helper
returning `Option<usize>` would be tidier, but exec functions have no `ensures` in this phase,
so Verus wouldn't know that the helper's result is below `rank`. The comparison has to be visible
in the caller. `rank_or(target, current)` only turns *none* into `current`, which then fails the
inline `<` check.

---

## 4. The functions

### 4.1 `eval_program(p, case, ext, fuel, memo) -> CaseResult` (`exec/mod.rs`)

The entry point. In order:

1. Computes `ranks(p)` and builds the `Env`.
2. Evaluates **every constant**, in id order, through `eval_const` at the constant's own rank (a
   constant with no rank is `Stuck(Rank)`). Remembers each failure as `Failure::Const` and the
   **first** (lowest-id) failing error.
3. For **every entry**:
   - if a constant failed: the entry's result is that error repeated once per cell (or a single
     `Stuck(Layout)` cell if the entry's dims don't even give a cell count);
   - otherwise: `eval_entry`.
4. Builds `failures`: the constant failures first, then `memo.failures` (failing inputs, in
   discovery order).

The `memo` is passed in by the caller so that tests can turn it off. `tungsten-check`'s
`Prepared::eval` creates it.

### 4.2 `eval_entry(env, e, memo)` (`exec/unit.rs`)

Builds the base context for entry `e`: the case's origin, the entry's scales and dims. Then
calls `eval_unit_ctx` at the unit's rank. If the unit has no rank, every cell is `Stuck(Rank)`.

### 4.3 `eval_unit_ctx(env, rank, u, base, memo)` (`exec/unit.rs`, layer 4)

Evaluates unit `u` over every cell of `base.dims`. This is used both for entries and for
building an input from its source unit.

1. Computes the cell count `n` (a single `Stuck(Layout)` cell if the dims can't be laid out).
2. **Builds every input `u` lists**, in list order, through `build_input` (each with an inline
   rank check). It doesn't stop at the first failure: all inputs are built, and it keeps the
   error of the failing input with the **lowest `InputId`**.
3. If any input failed: returns that error `n` times. No cell runs.
4. Otherwise, for `k = 0 .. n` in order: decodes `k` to a position, builds the cell's `Ctx`, and
   calls `eval_cell`. Collects the results.

Building all inputs (instead of stopping early) means every failing input ends up in the
diagnostics, and the choice of error is by id, independent of the list order.

### 4.4 `eval_cell(env, rank, u, ctx, memo)` (`exec/unit.rs`, layer 3)

One cell: looks up the unit and its `main`, returns `Stuck(MainParams)` if `main` has
parameters, then `eval_call(main, no arguments)` at `main`'s rank.

### 4.5 `build_input(env, rank, i, memo) -> InputBuild` (`exec/unit.rs`, layer 5)

Builds the buffer of input `i`.

1. If the memo is on and already has `i`: returns `InputBuild::Memoized` right away.
2. Otherwise looks up the declaration, checks the source unit's rank, and calls `eval_unit_ctx`
   on the source unit with the input's own context (case origin, the input's scales and dims).
3. Turns the per-cell results into a buffer: all the values, or `(k, error)` for the first
   failing cell `k` in flat order.
4. If it failed and `listed[i]` is not yet set, adds `Failure::Input` to `memo.failures` and sets
   `listed[i]`.
5. With the memo on: stores the result in `memo.inputs[i]` and returns `Memoized`. With the memo
   off: returns `InputBuild::Fresh(result)`.

`InputBuild` exists so that a cached buffer can be read **in place** (by looking in
`memo.inputs`) instead of being copied out on every read.

### 4.6 `eval_const(env, rank, c, memo)` (`exec/unit.rs`, layer 2)

Returns the cached result if the memo has one. Otherwise evaluates the constant's initializer
with `eval_expr`, an **empty frame** (constants have no locals) and a **placeholder context**
(all zeros, a `1×1×1` grid). The placeholder is never observed in a checked program, because
the checker forbids ambients in constant initializers. Caches the result if the memo is on.

### 4.7 `eval_call(env, rank, g, args, ctx, memo)` (`exec/stmt.rs`, layer 2)

Calls function `g` with already-evaluated arguments.

1. `Stuck(Shape)` if the argument count isn't `params`, or `params > locals.len()`.
2. Builds a fresh `Frame`: arguments in the first `params` slots, `None` in the rest.
3. Runs the body with `eval_stmts`. The result:
   - `Return(v)` → `Ok(v)`;
   - `Next` (fell off the end) → `Stuck(NoReturn)`;
   - `Break` (escaped the body) → `Stuck(BreakOutsideLoop)`;
   - an error → that error.

The `ctx` is passed through unchanged: a called function sees the same cell as its caller.

### 4.8 `eval_stmts(env, rank, ctx, frame, ss, memo)` (`exec/stmt.rs`, layer 1)

Runs the statements in order. Stops at the first one that doesn't end with `Next` (a `Break`,
a `Return` or an error) and passes that up. Returns `Next` if all of them did.

### 4.9 `eval_stmt(env, rank, ctx, frame, s, memo)` (`exec/stmt.rs`, layer 1)

| Statement | Implementation |
| --- | --- |
| `Assign` | `eval_expr` the value, then store it in the frame slot (`Stuck(BadIndex)` if the local doesn't exist) |
| `Return` | `eval_expr`, then `Flow::Return(v)` |
| `If` | Evaluate the condition (`Stuck(Shape)` if it isn't a `Bool`), run one branch with `eval_stmts` |
| `While` | A Rust `loop` with an iteration counter. Each round: evaluate the condition; `false` → `Next`; counter already at `fuel` → `OutOfFuel`; otherwise run the body. `Break` → `Next`, `Return` → passed up |
| `Repeat` | A counted Rust `while` loop, `count` rounds, no fuel. `Break` and `Return` as above |
| `Break` | `Flow::Break` |

**Loops are Rust loops, not recursion.** A recursive `while` (one call per iteration) would match
a mathematical definition more closely, but the Rust stack would grow with the fuel, and with
`DEFAULT_FUEL = 2^20` that would overflow. The later spec will define loops recursively and the
proof will connect the two.

### 4.10 `eval_expr(env, rank, ctx, frame, e, memo)` (`exec/expr.rs`, layer 0)

One `match` arm per expression kind, following SEMANTICS.md §5. Points worth knowing:

- **`Local`**: `Some(v)` → a copy of `v`; `None` → `Stuck(Unassigned)`; missing slot →
  `Stuck(BadIndex)`.
- **`Const`**: inline rank check, then `eval_const`.
- **`Host`**: a copy of `case.hosts[h]`, or `Stuck(Case)` if the case is too short.
- **`Table`**: the handle `Handle(t)`, an opaque id typed by the table's `HandleTypeId`.
- **`Ambient`**: reads `ctx`.
- **`Density`**: `read_density` (§4.12).
- **`Call`**: `eval_args` first, then the inline rank check, then `eval_call`. So the arguments
  are evaluated even if the call then turns out to be stuck.
- **`ExternCall`**: `eval_args`, then `ext.call(id, &args)`. The result is checked with
  `value_has_type` against the declared return type (`Stuck(ExternResult)` if it doesn't match).
  A domain error becomes `ExternDomain` or `Stuck(ExternDomainUndeclared)`, depending on the
  declaration. The call-site annotations (`params`, `ret`) are ignored here.
- **`Binary`**: `And`/`Or` are handled first, with short-circuit. Everything else evaluates left,
  then right, then calls `binop` (§4.13).
- **`Unary`**, **`Field`**, **`Cast`**: evaluate the operand, then `unop`, `field` or `cast`.
- **`Index`**: evaluates the array, then the index. A negative index or one past the end is
  `OutOfBounds`. Anything other than `Array` + `I32` is `Stuck(Shape)`.
- **`MakeVec3` / `MakePos3`**: evaluate x, y, z in order, `Stuck(Shape)` unless they are three
  `F64`s / three `I32`s.
- **`MakeRecord`**: starts with one empty slot per declared field. Evaluates the written fields
  in written order and drops each into its slot (`Stuck(Shape)` if the slot is already filled,
  `Stuck(BadIndex)` if it doesn't exist). Then `Stuck(Shape)` if any slot is still empty.
  Otherwise the record, in declaration order.
- **`ArrayLit`**: evaluates the elements in order.

### 4.11 `eval_args(env, rank, ctx, frame, args, memo)` (`exec/expr.rs`, layer 0)

Evaluates a list of expressions left to right, stopping at the first error. Used for function and
extern arguments.

### 4.12 `read_density(env, rank, ctx, frame, input, index, memo)` (`exec/expr.rs`, layer 0)

1. If there is an explicit index: evaluates it first (`Stuck(Shape)` unless it is an `I32`).
2. Looks up the input's declaration.
3. Picks the buffer position:
   - explicit index: `OutOfBounds` if negative, otherwise the index;
   - no index: `0` if the input's dims are `(1,1,1)`, otherwise `ctx.k` (the reader's flat
     index).
4. Inline rank check on the input, then `build_input`. With the memo on, the buffer is read
   straight out of `memo.inputs[i]`; with it off, out of the fresh result.
5. `OutOfBounds` if the position is past the end of the buffer. A failed buffer gives its error.

In a checked program, step 4 always finds a buffer that has already been built successfully:
the reader's unit built all of its inputs before any of its cells ran (§4.3), and a failure there
would have stopped the cells from running at all.

### 4.13 Operators: `binop`, `unop`, `field`, `cast` (`exec/expr.rs`)

These take already-evaluated values and dispatch on their shapes:

- `binop(op, l, r)`:
  - shifts first: the right operand must be an `I32`, the left any integer → `shift`;
  - same-type pairs → `i8_op`, `i32_op`, `i64_op`, `f32_op`, `f64_op`, `bool_op`;
  - `Vec3`/`Vec3`, `Vec3`/`Pos3`, `Pos3`/`Vec3` → `vec3_op` (a `Pos3` is first converted with
    `pos_to_vec`, exactly);
  - `Pos3`/`Pos3` → `i32_arith` per component, x then y then z;
  - anything else → `Stuck(Shape)`.

  Integer division checks for a zero divisor itself (`DivByZero`) before calling `prim`. `>`
  and `>=` on floats are implemented as `lt(b, a)` and `le(b, a)`, and `!=` as `!eq`, which is
  exactly IEEE behavior including `NaN`.
- `unop(op, v)`: `Not` on `Bool`/integers, `Negate` on numbers and componentwise on vectors.
- `field(sel, v)`: a vector component, or a record field (`Stuck(Shape)` if the record id doesn't
  match).
- `cast(to, v)`: integers travel through `i64` (`from_int`), floats have their own arms, `Bool`
  becomes `0`/`1`, `Pos3` becomes `Vec3`.

Every actual arithmetic operation, comparison and conversion goes through `prim.rs` (§5).

---

## 5. The primitives: `prim.rs`

`prim.rs` is a flat list of tiny functions on raw values: `i32_add`, `i8_ushr`, `f64_lt`,
`f32_to_i8`, `i64_to_f32` and so on. Floats are passed and returned as bits (`u32`/`u64`).

They are the **trusted base**: each body is ordinary Rust (`wrapping_add`, `as` casts,
`f64::from_bits(a) + f64::from_bits(b)`), marked `external_body` so Verus doesn't look inside.
This is the only file allowed to do that. In the proof phase each will get an `ensures` that
equates it to a mathematical definition.

Things to know:

- Rust's `as` from float to int already rounds toward zero, saturates and maps `NaN` to 0, which
  is exactly Java's rule, so `f64_to_i32` is just `as i32`.
- `f32_to_i8` / `f64_to_i8` go through `i32` first (`(x as i32) as i8`), because that's what
  Java's `(byte)` cast does. A direct `as i8` would saturate to `-128 ..= 127` instead, so
`300.0` would give `127` rather than Java's `44`.
- `i8` arithmetic widens to `i32`, applies the `i32` operation and truncates. `i8_ushr` shifts
  the sign-extended 32-bit value, matching Java.
- Shift amounts are masked (`& 31` or `& 63`).
- The division primitives return `0` for a zero divisor so they never panic, but the caller
  always checks first and reports `DivByZero`.

`prim.rs` has its own unit tests for the edge cases (`MIN / -1`, shift masks, `NaN` and infinity
casts, `-0.0`, rounding).

---

## 6. Grid layout: `layout.rs`

| Function | Does |
| --- | --- |
| `len(d)` | `dx * dy * dz`, or `None` on a negative component or overflow |
| `flat(c, d)` | `x + y*dx + z*dx*dy`, or `None` if `c` is outside the grid or it overflows |
| `decode(k, d)` | The inverse of `flat` on `0 .. len(d)`: `x = k % dx`, `y = (k / dx) % dy`, `z = k / (dx*dy)`. `None` past the end or for an empty grid |
| `is_unit(d)` | `d == (1,1,1)` |

All arithmetic is checked, so these never panic. A `None` becomes `Stuck(Layout)` in the
interpreter. After the checker's grid rule (CHECK.md §8) that can't happen.

The interpreter mostly uses `len` and `decode`. Buffers are stored in flat order and reads use
the reader's `k`, so `flat` is only needed in tests (`decode(flat(c)) == c`).

---

## 7. Values: `value.rs`

- `Value` is the run-time value type (SEMANTICS.md §2). Floats are bits, so the derived `==` is
  bitwise.
- `value_has_type(p, v, t)` is the "does this value fit this type" check used for host values
  (`check_case`) and extern results. It is recursive over arrays and records and checks that table
  handles exist and have the right kind.
- `clone_value`, `clone_values`, `clone_result`, `clone_case` are the copies the interpreter uses.
  The `impl Clone for Value` outside `verus!{}` delegates to `clone_value` (VERUS.md explains why).
- `trait Externs { fn call(&self, ext: ExternId, args: &[Value]) -> Result<Value, ExternFail>; }`
  is the one hook to the outside world. `ExternFail::Domain` is its only failure.

---

## 8. Calling it from plain Rust

You normally don't call `eval_program` directly. `tungsten-check` wraps it:

```rust
let prep = tungsten_check::prepare(&program, &registry)?;            // lower + check, once
let case = prep.case(&NamedCase { origin: [0.0; 3], hosts: vec![…] })?; // bind hosts by name
let result = prep.eval(&case, &my_externs, EvalOptions::default())?;  // per case
```

`Prepared::eval` runs `check_case`, creates a `Memo`, calls `eval_program`, and then scans the
result: any `Stuck` (in a cell or in `failures`) turns the whole call into
`EvalFailure::Stuck { reasons, failures }`, because it means a checker or interpreter bug.
`EvalOptions` holds the fuel (`DEFAULT_FUEL = 2^20`) and the memo switch. `evaluate` does all
three steps in one call.

An `Externs` implementation usually needs names (which extern is `ExternId(3)`? which seed does
`TableId(0)` have?). It gets them from `prep.map()` (the `LowerMap`) or `prep.extern_id(name)`.

---

## 9. Costs and limits

- **Memo on (the default):** each constant is evaluated once and each input buffer is built once
  per case. A cell costs roughly the work of its own expressions.
- **Memo off:** every constant read re-evaluates the constant, and every density read rebuilds the
  whole buffer (and, recursively, everything it depends on). Exponential in the depth of the
  input chain. Only for the equivalence tests, on tiny grids.
- **Fuel:** a `While` that legitimately needs more than `DEFAULT_FUEL` iterations gives
  `OutOfFuel`. Pass more fuel in `EvalOptions` (the testsuite's `@fuel(n)` does this).
- **Values are copied, not shared.** Reading a local, a host, a constant or a buffer cell clones
  the value. That's simple and verifiable, and density values are small.

---

## 10. Changing the interpreter

- **A new `Expr` or `Stmt` variant:** add its arm to `eval_expr` / `eval_stmt`, and to everything
  listed in the README's "When you change something" (the checker, `clone_*`, wf, …).
- **A new `Value` variant:** `clone_value`, `value_has_type`, and the dispatch in `binop` /
  `unop` / `cast` / `field`.
- **A new evaluation function** that calls into another rank (like `build_input` does) needs its
  own layer number, chosen so that every call it makes goes either to a lower rank or a lower
  layer at the same rank.
- Keep the error order canonical (SEMANTICS.md §10.3): if you add a place where several things
  can fail, decide which one wins by id or position, never by evaluation order that a backend
  might do differently.
- Run `scripts/verify.sh` afterwards: it is the acceptance test for termination and bounds.
