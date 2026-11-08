# tungsten-builder

Builds `tungsten-ir` `OptimizedProgram`s from Rust. It is meant for frontends that lower a source
language, and it replaces the legacy SPMT builder. The design is in `specs/builder.md`.

It depends only on `tungsten-ir`. The facade re-exports it as `tungsten_wg::builder`.

## A first program

```rust
use tungsten_wg::builder::prelude::*;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut pb = ProgramBuilder::new();

// A unit is finished before anything reads it.
let mut f = FnBuilder::new("base", F64);
f.ret(pos3().x().cast(F64) * 10.0);
let base = pb.finish_unit(f);

// An input of that unit, read by a helper.
let grid = Grid::new((4, 1, 1));
let d = pb.input(base, grid);
let mut h = FnBuilder::new("shared", F64);
h.ret(d.read());
let shared = pb.finish_helper(h);

// `build!` and plain methods build the same statements.
let mut f = FnBuilder::new("main", F64);
let i = f.local("i", I32);
build!(f, {
    i = 0;
    let total: F64 = shared.call([]);
    while (i.lt(3)) {
        total += d.at(i);
        i += 1;
    }
    return total;
});
let main = pb.finish_unit(f);
pb.entry(main, grid);

let program: OptimizedProgram = pb.build()?;
# Ok(())
# }
```

## The pieces

| Type | What it is |
| --- | --- |
| `ProgramBuilder` | Owns the program. Hands out `Copy` handles: `Unit`, `Helper`, `Input`, `Host`, `Table`, `Constant` |
| `FnBuilder` | One function, owned and borrowing nothing, so several can be open at once. `finish_unit` or `finish_helper` decides what it becomes |
| `Local` | A parameter or local of one function, from `param`, `local` (unique name), `temp` (name hint) or `anon` |
| `Expr` | An IR expression with its type and any mistakes found so far. Handles and literals convert into it |
| `Grid`, `InputSpec` | Dims and scales, and everything that describes an input |
| `Decls` | Extern and record declarations from `extern_fns!` and `record_decls!` |

**Recursive lowering** needs nothing special. The program and the current function are separate
values, so a lowering function takes both, and can open a new `FnBuilder` while another is open:

```rust
# use tungsten_wg::builder::prelude::*;
# enum Density { X, Add(Box<Density>, Box<Density>), Cached(Box<Density>, Grid) }
fn lower(pb: &mut ProgramBuilder, f: &mut FnBuilder, d: &Density) -> Expr {
    match d {
        Density::Add(a, b) => lower(pb, f, a) + lower(pb, f, b),
        Density::Cached(inner, grid) => {
            let mut g = FnBuilder::new(pb.fresh_name("cached"), F64);
            let r = lower(pb, &mut g, inner);
            g.ret(r);
            let unit = pb.finish_unit(g);
            pb.input(unit, *grid).read()
        }
        // ...
#       Density::X => pos3().x().cast(F64),
    }
}
```

Only finished units and helpers have handles, so unit and call cycles can't be written.

## Types and errors

Every `Expr` is typed as it is built, by the rules of `tungsten-core/CHECK.md` §4.1 to §4.3. There
are no implicit conversions (`x + 1` with `x: f64` is an error), no scalar broadcast (`vec * 2.0`),
indexes are `i32`, and `==` is for scalars only.

Operators can't return `Result`, so a mistake is recorded in the `Expr` instead, with the Rust
location that made it. Its type becomes unknown where it isn't clear, and an unknown operand raises
no further error, so one mistake gives one error. Statements move an `Expr`'s errors into the
function, `finish_*` moves them into the program, and `build` returns all of them in order:

```text
src/lower.rs:42:17: fn `main`: `+` can't be applied to `f64` and `i32`
src/lower.rs:57:9: fn `main`: returned value: expected `f64`, found `i32`
```

Nothing panics on an ill-typed expression or a misused handle. A local of another function, or a
handle from another `ProgramBuilder`, is `ForeignHandle`.

The builder doesn't check flow (missing returns, unassigned reads, `break` placement), grids, or
externs and records against a registry. `Compiler::compile` does that. What the builder
guarantees is that every ref is valid and every list matches the bodies, so a built program always
lowers past `DanglingRef`.

## Derived lists

`finish_unit` and `finish_helper` fill every list from the bodies. There is no way to list
anything by hand.

| List | Contents | Order |
| --- | --- | --- |
| `Function.host_inputs` | Hosts the function reads itself | First use |
| Unit helpers | Everything reachable from `main` | Callees before callers |
| Unit hosts, tables, inputs | Read by `main` or any helper | First use |
| Unit constants | Constants read, and those their values reach | Dependencies first |

First use follows evaluation order and enters a callee at its first call, after the call's
arguments. A read decides: a read in dead code still lists the input, and an input nobody reads is
never listed.

## Merging and names

| Item | Merged when | On a match |
| --- | --- | --- |
| Input | Same source, dims, and scales by bits | The existing input. The first name wins |
| Host | Same name | The existing host. Another type is `HostTypeConflict` |
| Table | Equal `PermutationTableInput` | The existing table |
| Constant, helper, unit | Never | |

So two units that each ask for the same input share it. Scales compare by bits, so `0.0` and `-0.0`
give two inputs. Names are required, may repeat (backends make them unique), and `fresh_name`
gives deterministic ones. `source_hash` defaults to 0.

## Macros

`build!(f, { … })` appends to `f`, and `body!(f, { … })` returns a `Block` for `f.append`. `f`
must be a variable name: each nested block is a closure whose parameter shadows it, so code inside
a block can use `f` and other `&mut` values, such as the program builder. Without the macros,
capture each branch with `f.block` and join them with `f.if_blocks(cond, then, other)` when both
branches need `&mut ProgramBuilder`.

| Form | Means |
| --- | --- |
| `x = e;`, `x += e;`, `-=`, `*=`, `/=` | `f.assign(x, …)` |
| `let x: T = e;` | `f.temp("x", T)`, then assign, binding `x` until the end of the block |
| `let x = e;` | A plain Rust `let` |
| `return e;`, `break;` | `f.ret(e)`, `f.break_()` |
| `if (c) { } else { }`, `while (c) { }`, `repeat (n) { }` | The control-flow methods |

Comparisons stay methods (`.lt()`, `.ge()`, `.eq_expr()`, …), as do the bit operators (`.bitand()`,
`.shl()`, …).

## Externs and records

Each signature is written once and gives both a typed call function and a declaration:

```rust
# use tungsten_wg::builder::prelude::*;
# use tungsten_wg::{Compiler, RustBackend, RustConfig};
extern_fns! {
    group surface_externs;
    floor(x: F64): F64,
    checked_div(a: F64, b: F64): F64 @domain_error,
}
record_decls! {
    group records;
    Pair { a: F64, b: I32 },
}

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut pb = ProgramBuilder::new();
# let mut f = FnBuilder::new("m", F64);
# f.ret(floor(checked_div(7.0, 2.0)));
# let u = pb.finish_unit(f);
# pb.entry(u, Grid::new((1, 1, 1)));
# let program = pb.build()?;
let registry = tungsten_wg::registry_from([surface_externs(), records()])?;
let out = Compiler::new(program, registry, RustBackend::new(RustConfig::default())).compile()?;
# Ok(())
# }
```

Generated call functions check their arguments' types. There is deliberately no untyped extern
call. `registry_from` (facade) drops identical declarations, so groups can
overlap, and leaves conflicts for the registry to report.

## Tests

| File | What it checks |
| --- | --- |
| `tests/typing.rs` | Every binary operator, unary operator and cast over the scalars, `vec3` and `pos3` agrees with core, through generated `.tw` programs and `check::prepare` |
| `tests/derive.rs` | Lists match a hand-written twin, and removing any entry makes core reject the program |
| `tests/merge.rs` | Each merging rule, `-0.0`, the first name, `HostTypeConflict`, and `MultipleUses` on the inliner |
| `tests/errors.rs` | One error per mistake at the right line, every error kind, `ForeignHandle`, no panics |
| `tests/macros.rs` | Each macro form against its method equivalent, and recursion inside macro blocks |
| `tests/determinism.rs` | Building twice gives the same program and Rust code, and duplicate names stay unique |
| `tests/slices/` | Real frontend patterns, each `isomorphic` to a twin in `tungsten-testsuite/corpus` that runs on every backend. `REVIEW.md` lists what they found |

```text
cargo test -p tungsten-builder
cargo test -p tungsten-testsuite -- builder/    # the twins on the interpreter and backends
```
