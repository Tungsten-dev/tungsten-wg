# The checker, rule by rule

This document describes every rule the checker (`src/check/`) enforces. For each rule it gives
the **rule** itself (what exactly is accepted) and the **explanation** (why the rule exists and
what would go wrong without it). It ends with a reference table of every error kind.

What a program *means* once it is accepted is in [SEMANTICS.md](SEMANTICS.md). How the checker
code is shaped for Verus is in [VERUS.md](VERUS.md).

---

## 1. What the checker is for

The core IR can express plenty of programs that make no sense: adding a `Bool` to an `f64`,
reading a local before it has a value, calling a function that doesn't exist, a constant
defined in terms of itself. The lowering in `tungsten-check` deliberately **doesn't** catch
these. It translates what it is given and leaves every "is this sensible?" question to the
checker, so that there is exactly one place that decides.

The checker's promise is:

> If `check(p)` accepts a program, and `check_case(p, case)` accepts a case, then evaluating
> that case never gets `Stuck`.

"Stuck" is the interpreter's word for "this should never have been allowed to run" (see
SEMANTICS.md §10.2). Real run-time failures, such as dividing by zero or running out of fuel,
are still possible. Those depend on the data, and no checker could rule them out.

Today that promise is tested: programs are mutated at random, and the ones the checker still
accepts are run, and must never get stuck. The later proof phase will prove it.

A second job is to make sure **backends** can compile the program: the scoping rules (§6) exist
because the backends generate code per unit and need every unit's lists to be complete.

### Entry points

| Function | Does |
| --- | --- |
| `check(p)` | All program rules. `Ok(())` or every error found |
| `check_case(p, case)` | Does this case fit this program? (§9) |
| `check::acyclic::ranks(p)` | The rank of every function, constant, input and unit. The interpreter uses it too (§7) |

From plain Rust you normally go through `tungsten-check`: `prepare` lowers and checks,
`Prepared::case` binds a case by host names and runs `check_case`.

---

## 2. How errors are reported

### 2.1 Two stages

1. **Well-formedness** (§3) first: does every id point at something that exists? If anything is
   wrong here, the checker reports those errors and **stops**.
2. Only for well-formed programs: **types** (§4), **flow** (§5), **scoping** (§6), **grids**
   (§8) and **cycles** (§7). These report everything they find.

*Why stop after stage 1?* Every later rule looks things up by id. If ids can be broken, every
rule would have to handle "this id points nowhere" everywhere, and would report confusing
follow-up errors. A wf failure is also never the user's fault: it means the lowering has a bug
or someone built the program by hand.

### 2.2 Where an error is

Every error carries a **path**: the item it is in (a function, a constant, a unit, an input, an
entry, or "the program") and the steps from that item down to the exact node:

```
fn `top` › stmt 1 › value › left
```

means: in function `top`, statement 1, the value being assigned or returned, its left operand.
The core checker produces these paths with ids (`CorePath`), and `tungsten-check` renders them
with names, so checker errors look exactly like lowering errors.

### 2.3 Order

Errors come out in **walk order**: constants by id, then functions by id, then units, inputs and
entries, and cycles at the very end. Inside an item, they are in pre-order: a node's own error
comes before the errors of its children, and children are in the order they appear in the source.

(The rules don't all walk in that order: typing, for example, can only report a parent after it
has typed the children. So `check` collects everything and then sorts it stably.)

### 2.4 No cascades

One mistake gives one error. Typing computes "the type of this expression, or *unknown*". An
expression whose type is unknown is already reported, so its parents stay quiet about it:

```
return ((p + 1) + p);   // p: f64
```

reports only `` `+` can't be applied to `f64` and `i32` `` at the inner `+`. The outer `+` and
the `return` see "unknown" and say nothing.

Other rules avoid repeats in their own ways, described with each rule: an unassigned local is
reported once per function, a scoping violation once per unit and offending id, and so on.

---

## 3. Well-formedness

Well-formedness ("wf") is only about **ids and links**. It says nothing about types. It is
defined twice, and both must agree:

- as a mathematical definition (`spec fn wf_program` in `src/wf.rs`), which the proofs will use;
- as executable code that reports errors (`check_wf` in `src/check/wf.rs`).

`tungsten-check`'s tests also hold a plain-Rust copy (`wf_mirror.rs`) and check that it agrees
with `check_wf`. If you change one of the three, change all of them.

### The rules

| Rule | Error |
| --- | --- |
| Every id is in range of its table: units, functions, constants, hosts, tables, inputs, externs, records, everywhere they occur (in expressions, unit lists, function host lists, input and entry declarations) | `WfBadId { table, id }` |
| Every `LocalId` is below the enclosing function's `locals.len()` | `WfBadId { table: Local, … }` |
| A constant's initializer uses **no** locals at all (it is checked as if it had zero locals) | `WfBadId { table: Local, … }` |
| Every type is wf: `Record(r)` needs record `r` to exist, recursively through `Array` | `WfBadId { table: Record, … }` |
| Each function has `params <= locals.len()` | `WfParams` |
| A record field selector `Record(r, i)`, and each field of a `MakeRecord`, names an existing field of `r` | `WfField` |

### What wf deliberately doesn't check

Types, exactly-once record fields, scoping lists, cycles, dims, definite assignment, `break`
placement and density-read rules. Those are the later stages. Wf is kept small because it is a
precondition for *everything*, including the proofs.

---

## 4. Types

Typing assigns every expression a type, checks it against what its context needs, and checks
the annotations the program carries.

### 4.1 Expressions

**Numeric** means `I8`, `I32`, `I64`, `F32`, `F64`. **Integer** means `I8`, `I32`, `I64`.

| Expression | Rule | Type |
| --- | --- | --- |
| `Lit` | — | The literal's type |
| `Local(l)` | — | The local's declared type |
| `Const(c)` | — | The constant's derived type (§4.4) |
| `Host(h)` | — | The host's declared type |
| `Table(t)` | — | `Handle(ty)`, with the table's handle type |
| `Ambient` | — | `Pos3` for `Pos3`, `Vec3` for the other three |
| `Density { input, index }` | `index`, if present, is an `I32` | The return type of the source unit's `main` |
| `Call { func, args }` | As many arguments as `func` has parameters (`Arity`); argument `k` has the type of parameter `k` (`TypeMismatch`) | `func`'s return type |
| `ExternCall { ext, params, ret, args }` | The annotation `params`/`ret` equals the registry declaration (`ExternSignature`); arity; each argument has the declared parameter type | The declared return type |
| `Binary` | See §4.2 (`BadBinary`) | See §4.2 |
| `Unary Not` | `Bool` or an integer (`BadUnary`) | Same as the operand |
| `Unary Negate` | Numeric, `Vec3` or `Pos3` (`BadUnary`) | Same as the operand |
| `Field { base, sel, ty }` | `.x/.y/.z` of a `Vec3` (→ `F64`) or a `Pos3` (→ `I32`), or field `i` of the matching record (`BadFieldBase`). The annotation `ty` must equal the field's real type (`FieldAnnotation`) | The field's real type |
| `Index { array, index }` | `array` is an `Array(T, n)` (`NotArray`); `index` is an `I32` | `T` |
| `MakeVec3` | Three `F64`s | `Vec3` |
| `MakePos3` | Three `I32`s | `Pos3` |
| `MakeRecord { record, fields }` | Every field of the record exactly once (`RecordFieldMissing`, `RecordFieldRepeated`); each value has its field's type | `Record(record)` |
| `ArrayLit(es)` | Not empty (`EmptyArray`); every element has the type of the first | `Array(T, len)` |
| `Cast { to, operand }` | See §4.3 (`BadCast`) | `to` |

**Explanations:**

- **Annotations are checked, not trusted.** An `ExternCall` records the signature the call
  site was written with, and a `Field` records the type the frontend thought the field had. The
  lowering could have checked and dropped them, but then that check would be one more piece of
  unverified code. Keeping them means a frontend that disagrees with the registry is caught here,
  with a clear message, instead of producing a backend compile error.
- **`index` must be an `I32`**, not any integer. One index type keeps indexing identical in
  every backend, and `I32` is enough because no grid has more than `i32::MAX` cells (§8).
- **Records: exactly once.** The lowering keeps record fields in written order and allows
  duplicates and gaps, so that a mistake in the source is still visible here. A missing field
  would leave the value incomplete. A repeated one would make it unclear which value wins.
- **No empty arrays.** An empty array literal has no element to take the type from, and arrays
  of length 0 are useless in density code anyway.
- **Errors still give a type where it's clear.** A call with the wrong arity still has the
  callee's return type, and a `MakeRecord` with a missing field is still a `Record(record)`. This
  keeps one mistake from causing errors in the surrounding code.

### 4.2 Binary operators

| Operators | Operands | Result |
| --- | --- | --- |
| `+ - * /` | Two of the same numeric type | That type |
| `+ - * /` | `Vec3`/`Vec3` | `Vec3` |
| `+ - * /` | `Pos3`/`Pos3` | `Pos3` |
| `+ - * /` | `Vec3`/`Pos3` or `Pos3`/`Vec3` | `Vec3` |
| `== !=` | Two of the same type, `Bool` or numeric | `Bool` |
| `< <= > >=` | Two of the same numeric type | `Bool` |
| `&& \|\|` | Two `Bool`s | `Bool` |
| `& \| ^` | Two of the same integer type, or two `Bool`s | That type |
| `<< >> >>>` | An integer on the left, an `I32` on the right | The left type |

**Explanations:**

- **No implicit conversions.** `p + 1` with `p: f64` is an error. Java would promote the `1`,
  but other backends (and the people reading generated code) would have to know the exact
  promotion rules. Writing `1.0` or a cast is clearer.
- **No scalar broadcast for vectors.** `vec * 2.0` is not allowed. Some backends have no
  operator overloading, so every broadcast would turn into a hand-written expansion anyway.
- **No equality on vectors, records or arrays.** Only scalar types. It avoids having to decide
  what `==` means componentwise and for `NaN`s inside a vector.
- **Shift amounts are always `I32`**, as in Java, and the shifted value keeps its type.

### 4.3 Casts

Allowed: numeric → numeric (including to the same type), `Bool` → integer, and `Pos3` → `Vec3`.
Nothing else. What each cast computes is in SEMANTICS.md §6.5.

*Why so few?* Every allowed cast needs exact, agreed-on semantics in every backend. These are the
ones density code uses, and they all have a clear Java meaning. `float → Bool` or
`Vec3 → Pos3` would need a rounding rule nobody has agreed on.

### 4.4 Statements and units

| Rule | Error |
| --- | --- |
| `Assign`: the value has the local's declared type | `TypeMismatch` |
| `Return`: the value has the function's return type | `TypeMismatch` |
| `If` / `While` condition is a `Bool` | `TypeMismatch` |
| A unit's `main` has **no parameters** | `MainParams` (at `unit › main`) |

*Why no parameters on main?* `main` computes one cell. Everything it needs comes from the
context (ambients), hosts, constants and inputs. There is nothing that could be passed as an
argument. Its return type can be anything: that's the unit's cell type.

### 4.5 Constants

A constant has **no declared type**. Its type is the type of its initializer. Constants are
typed in **rank order** (§7), so a constant's dependencies always have their type before the
constant itself is typed. A constant that is part of a cycle (or depends on one) gets the type
*unknown*: its initializer is still checked, but reading it never causes a type error. The cycle
itself is reported once, in §7.

A constant's initializer **may not contain** a function call, a density read, an ambient, a host
read, an extern call or a table (`ConstForbidden { what }`). Locals are already ruled out by wf.

*Why?* A constant is computed **once per case, before anything else** (SEMANTICS.md §8). It
can't depend on a cell (ambients, density reads) or on anything a unit provides. The list is
also exactly what `tungsten-ir`'s own constant expressions (`ConstExpr`) can't express: allowing
extern calls or tables here would accept programs that no frontend can produce and that backends,
which emit constants as standalone items, were never written to handle.

---

## 5. Flow

Flow analysis answers three questions for every function:

1. Is every local **definitely assigned** before it is read?
2. Does every path through the body end in a **`return`**?
3. Is every **`break`** inside a loop?

### 5.1 How it works

The checker walks each function's body from top to bottom, keeping track of:

- whether the current point is **reachable**, and
- the set of locals that are **definitely assigned** at this point.

At the start, the code is reachable and exactly the parameters are assigned.

| Statement | Effect |
| --- | --- |
| `Assign { target, value }` | Checks `value` first (so `x = x + 1` with `x` unassigned is an error), then marks `target` assigned |
| `Return(e)` | Checks `e`, then the rest of the block is unreachable |
| `Break` | An error if not inside a loop. The rest of the block is unreachable |
| `If` | Checks the condition, then each branch separately, starting from the current state. Afterwards: if both branches end unreachable, the code after the `if` is unreachable. If only one does, the state after the `if` is the other branch's state. Otherwise, a local is assigned after the `if` only if **both** branches assign it |
| `While` / `Repeat` | Checks the body with a copy of the current state (one loop level deeper, so `break` is fine inside). Afterwards, the state is **exactly what it was before the loop**, whatever the body did |

At the end of the body, if the end is still reachable: `MissingReturn`.

### 5.2 Loops don't count

This is the rule people trip over:

```
let x: f64;
repeat 2 { x = 1.0; }
return x;              // error: `x` may be read before it is assigned
```

and

```
while true { return p; }
                       // error: the end of the body is reachable without a `return`
```

Both look fine to a human: the `repeat` certainly runs, and `while true` never exits except by
`return`. The checker still rejects them: it treats every loop as "might run zero times, and
might exit normally".

*Why so strict?* Because every special case is a rule that every backend and the proofs must
agree on. `repeat n` with `n ≥ 1` is easy, but `while true` with no `break`, a `break` in an `if`
inside a `while true`, and so on, quickly become a small theorem prover. Java's own rules here
are subtle, and the other backends' compilers have their own different rules. The fix in the
source is always easy (assign before the loop, `return` after it), and the errors say so with a
**hint**:

- `UnassignedLocal` gets the hint `AssignedOnlyInLoop` when some loop earlier on the path does
  assign the local: "it is only assigned inside a loop, and loops don't count for definite
  assignment: assign it before the loop".
- `MissingReturn` gets the hint `EndsInLoop` when the body's last statement is a loop that
  contains a `return`: "the body ends in a loop that returns, but loops don't count: add a
  `return` after the loop".

### 5.3 Unreachable code is allowed

Statements after a `return` or `break` are allowed. They are still type-checked, but flow
reports nothing about them, and assignments there don't count.

```
return p;
let y: f64 = 1.0;      // fine
```

*Why?* Frontends and optimizers produce dead code all the time (an inlined function that returns
early, for example). Rejecting it, like Java does, would just push the cleanup work onto every
producer. The Java backend has to strip or guard such code itself.

### 5.4 Reporting

- `UnassignedLocal` is reported **once per local per function**, at its first unassigned read.
- `MissingReturn` is reported at the function itself.
- `BreakOutsideLoop` is reported at the `break`.

---

## 6. Scoping

Every unit carries lists of what it uses: `helpers`, `consts`, `tables`, `hosts` and `inputs`.
Every function carries a `hosts` list. Backends generate code **per unit** from these lists: the
helpers to emit, the constants and tables to pass in, the host inputs to wire up. A list that is
missing something produces generated code that doesn't compile, or worse, reads the wrong thing.
The scoping rules make sure the lists are complete.

For a unit `U`, let **`F(U)`** be its main function plus its helpers. For every function `f` in
`F(U)`:

### (a) Calls stay inside the unit

**Rule:** every function `f` calls is in `F(U)` too. (`HelperNotListed { unit, func }`)

**Why:** because this holds for *every* function in `F(U)`, `F(U)` contains everything `main`
can reach through calls. The backend can emit exactly the helpers in the list and know nothing
is missing.

### (b) Hosts are declared, by the function and by the unit

**Rule:** every host `f` reads is in `f.hosts` (`HostNotInFn { host }`), and everything in
`f.hosts` is in `U.hosts` (`HostNotInUnit { unit, host }`).

**Why:** host values are passed into generated functions as parameters. The function's list
decides its parameters, and the unit's list decides what the unit's caller must supply.

### (c) Tables are listed by the unit

**Rule:** every table `f` uses is in `U.tables`. (`TableNotListed { unit, table }`)

**Why:** tables are built once and handed to the unit. The list says which ones.

### (d) Constants are listed by the unit, including indirect ones

**Rule:** every constant `f` reads is in `U.consts`, and so is every constant those constants'
initializers read, transitively. (`ConstNotListed { unit, konst }`)

A direct read is reported at the reading expression. A constant needed only indirectly is
reported at the unit's `consts` list as a whole:

```
const k  = 1.0;
const k2 = (const(k) * 2.0);
fn m() -> f64 { return const(k2); }
unit mu { main m; consts [k2]; }      // error: const `k` isn't in the consts of unit `m`
```

**Why transitive:** the backend computes the unit's constants from the list. To compute `k2` it
needs `k`, so `k` must be there too.

### (e) Density inputs are read only by units that list them

**Rule:** every input `f` reads is in the `inputs` of `U` (`InputNotListed { unit, input }`).

**Why:** a density input is a buffer built before the unit's cells run, and only the inputs a
unit lists are built for it. Inputs are global, so a helper shared between units can read an
input as long as each of those units lists it. A helper whose units don't all list the input is
reported for each unit that doesn't, so a read can't find a missing buffer.

### Details

- A function that is in **no** unit is still type- and flow-checked. Only "every host it reads
  is in its own `hosts`" applies to it.
- Listing **more** than is used is never an error.
- Each violation is reported **once per (unit, offending id)**, at its first occurrence in walk
  order.
- A function listed by several units is checked against each of them.

---

## 7. Cycles and ranks

### 7.1 The rule

**No dependency cycles.** The checker builds a graph with a node for every function, constant,
input and unit, and these edges ("needs"):

| From | To |
| --- | --- |
| Function `f` | every function it calls, every constant it reads, every input it reads |
| Constant `c` | every constant (and, in a broken program, function or input) its initializer refers to |
| Input `i` | its source unit |
| Unit `U` | its `main`, every input it lists, and **every** constant in the program |

A cycle in this graph is an error, `Cycle { members }`, reported once per cycle under
"program", listing every member. Examples:

- a function calling itself, directly or through other functions (no recursion);
- a constant whose initializer reads itself, directly or indirectly;
- a unit reading its own output through an input (unit → input → unit).

**Why no recursion?** Evaluation must always terminate. Without cycles, the only construct that
can run unboundedly is `While`, and that is bounded by fuel. The same goes for constants and
inputs: a constant defined through itself, or a unit that needs its own output, has no value.

**Why does a unit need every constant?** Because constants are eager and global (SEMANTICS.md
§4): before any unit runs, every constant has been computed. The edge records that, so the rank
of a unit is above the rank of every constant.

### 7.2 Ranks

From the same graph, every node gets a **rank**:

- 0 if it needs nothing;
- otherwise 1 + the highest rank among the things it needs;
- *none* if it is on a cycle or needs something that is.

The rank is a number that strictly decreases along every edge. The interpreter uses it to prove
termination: every call, constant read, input build and unit evaluation checks that the target's
rank is strictly below the current rank (see EXEC.md §3). It computes ranks with this same
function, `acyclic::ranks`, so checker and interpreter can't disagree.

Ranks are computed by a simple repeated pass: give a rank to every node whose successors all
have one, and repeat until nothing changes. At most one pass per node is needed. There's no
recursion over the graph, which keeps the termination argument trivial.

Cycles are found by checking, for every node without a rank, whether it can reach itself. Its
cycle is then everything it reaches that can also reach it back. Cycles are reported in order of
their lowest member (functions first, then constants, inputs, units), and members in the same
order. A node that merely *depends on* a cycle has no rank but isn't reported itself: fixing the
cycle fixes it.

---

## 8. Grids

**Rule:** every entry's and every input's `dims` has each component at least 1
(`DimsTooSmall`), and the cell count `x * y * z` is at most `i32::MAX` (`GridTooLarge`). The
product is computed in checked 64-bit arithmetic, so huge dims are reported, not wrapped.

**Why:** a grid with a zero or negative size makes no sense, and the cell count must fit in an
`i32` because explicit density indices are `i32`s: every cell must be addressable by one. With
the limit checked statically, overflow in grid arithmetic is no longer a run-time concern, and
the interpreter's layout code can never overflow.

---

## 9. Checking a case

`check_case(p, case)` checks that a case fits the program:

| Rule | Error |
| --- | --- |
| There is exactly one host value per host | `HostCount { expected, found }` (reported alone) |
| Each host value has its host's declared type | `HostValue { host }` |

"Has its type" (`value_has_type`) means: the shapes match, an array has exactly the declared
length and every element has the element type, a record has its declaration's field count and
every field has its type, and a handle points at an existing table **of the right handle type**.

*Why check tables?* Host types can be anything, including handle types. A handle value that
actually points at a table of another handle type (a sampler where a permutation table is
expected) would be passed to an extern that expects the other type.

The origin needs no check: any three `f64`s are fine.

---

## 10. Reference: every error

Messages are as `tungsten-check` renders them; names come from its `LowerMap`.

### Well-formedness (internal: a lowering bug or a hand-built program)

| Kind | Meaning |
| --- | --- |
| `WfBadId { table, id }` | An id that points past the end of its table |
| `WfParams { params, locals }` | A function with more parameters than locals |
| `WfField { record, field }` | A field index the record doesn't have |

### Types

| Kind | Example message |
| --- | --- |
| `TypeMismatch { expected, found }` | ``fn `top` › stmt 0 › value: expected `f64`, found `i32` `` |
| `BadBinary { op, left, right }` | `` `+` can't be applied to `f64` and `i32` `` |
| `BadUnary { op, operand }` | `` `!` can't be applied to `f64` `` |
| `BadCast { from, to }` | ``can't cast `bool` to `f64` `` |
| `Arity { expected, found }` | `expected 1 arguments, found 2` |
| `ExternSignature { ext }` | ``the annotation of `sin` doesn't match its registry signature `(f64) -> f64` `` |
| `FieldAnnotation { annotated, actual }` | ``field annotated as `f32`, but it is `f64` `` |
| `BadFieldBase { sel, found }` | ``field `x` needs a `pos3` base, found `vec3` `` |
| `NotArray { found }` | ``indexing needs an array, found `vec3` `` |
| `RecordFieldMissing { record, field }` | ``field `b` of `Pair` is missing`` |
| `RecordFieldRepeated { record, field }` | ``field `a` of `Pair` is given more than once`` |
| `EmptyArray` | `empty array literal` |
| `ConstForbidden { what }` | A constant initializer uses a call, density read, ambient, host, extern call or table |
| `MainParams { params }` | ``unit `m` › main: a unit's main can't have parameters (it has 1)`` |

### Flow

| Kind | Example message |
| --- | --- |
| `UnassignedLocal { local, hint }` | `` `x` may be read before it is assigned `` (plus the loop hint, §5.2) |
| `MissingReturn { hint }` | ``fn `top`: the end of the body is reachable without a `return` `` (plus the loop hint) |
| `BreakOutsideLoop` | `` `break` outside a loop `` |

### Scoping

| Kind | Example message |
| --- | --- |
| `HelperNotListed { unit, func }` | `` `leaf` is called but isn't in the helpers of unit `top_main` `` |
| `HostNotInFn { host }` | ``host `h2` is read but isn't in this fn's `@hosts` `` |
| `HostNotInUnit { unit, host }` | ``host `h2` isn't in the hosts of unit `top_main` `` |
| `TableNotListed { unit, table }` | ``table `m` isn't in the tables of unit `top_main` `` |
| `ConstNotListed { unit, konst }` | ``const `k` isn't in the consts of unit `top_main` `` |
| `InputNotListed { unit, input }` | ``input `d` isn't in the inputs of unit `a_main` `` |

### Grids and cycles

| Kind | Example message |
| --- | --- |
| `DimsTooSmall` | ``entry 0 › dims: every dimension must be at least 1`` |
| `GridTooLarge` | ``entry 0 › dims: the grid has more than i32::MAX cells`` |
| `Cycle { members }` | ``program: dependency cycle through const `c` `` |

### Case

| Kind | Meaning |
| --- | --- |
| `HostCount { expected, found }` | The case has the wrong number of host values |
| `HostValue { host }` | A host value doesn't have the host's type |

---

## 11. Adding a rule

- Add the `CheckErrorKind` variant, its `Clone` arm in `check/error.rs`, and its message in
  `tungsten-check/src/check.rs`.
- Put the rule in the stage it belongs to, report it with a path, and make sure it can't
  cascade (no error about something already in error).
- If a new path step is needed, give it a place in `check/mod.rs::step_key` so the sort keeps
  walk order.
- Add a reject test in `tungsten-check/src/tests/check_errors.rs` that asserts the rendered
  message and location.
- If the rule closes a way to get `Stuck`, the interpreter keeps its own run-time check anyway:
  the interpreter never trusts the checker.
