# Slice review (builder spec §10, milestone B3)

Each slice is a frontend pattern from `tungsten-frontends/tungsten-mc-compile/src/transform_spmt/`
rewritten with the builder. Its twin in `crates/tungsten-testsuite/corpus` runs on the interpreter
and on Rust, Java and CUDA, original and inlined, against values computed by hand.

| Slice | Frontend code | Twin | What it exercises |
| --- | --- | --- | --- |
| `noise.rs` | `noise.rs`, `lower_normal_noise` | `builder/noise.tw` | Statements from octave loops, two tables per octave (merged across the two lowerings), the `as_density` choice between `finish_unit` and `finish_helper` |
| `recursive.rs` | `density.rs`, `lower_density` and `lower_density_input` | `builder/recursive.tw` | Recursive lowering of a `Density` enum, a unit opened while the caller is open, the frontend's memo plus input merging, lowering into both branches of an `if`/`else` |
| `top_surface.rs` | `find_top_surface.rs` | `builder/top_surface.tw` | `build!` with `while` and `break`, an indexed read of a layered input from a flat unit, first-use input order |
| `aquifer.rs` | `aquifer.rs`, `aquifer_random` and the surface sample loop | `builder/aquifer.tw` | Helpers with parameters, a static and a record host, a constant array, a body from three `body!` blocks, `as_mut_ref` (IR and text only) |
| `shared.rs` | program-level-inputs §2 | `density/shared_helper.tw` | A helper reading an input from two units |

## Frontend code that doesn't type-check

The builder reports these where they're written. They're the kind of finding §9 expects of a port.

- **`noise.rs`:** `rpos3 * freq` multiplies a `vec3` by an `f64`. Core has no scalar broadcast, so
  the slice writes `rpos3 * vec3!(f, f, f)`.
- **`find_top_surface.rs`:** `i = floor(upper / h) * h` assigns an `f64` to the `i32` `i`, and
  `r = i` assigns an `i32` to the `f64` `r`. The slice casts both.
- **`aquifer.rs`:** `origin` is declared `pos3` but read as `origin.field("x", F64)`. It only worked
  because conversion matched variables named `origin` to the ambient. The slice uses `origin()`.

## Friction points

1. **`if_else` with two closures can't both use `&mut ProgramBuilder`.** Rust won't let two live
   closures borrow the same value mutably, so the method form can't lower into both branches.
   `build!`'s `if`/`else` works, because it builds each branch with `block` in turn and then joins
   them with two hidden methods (`__condition`, `__if`). Lowering code that doesn't use the macro
   has no way to do the same. **Proposed:** a public `if_blocks(cond, then: Block, other: Block)`.
   Not added yet, as it's an API decision.
2. **Mixed-type call arguments need `.into()`.** `helper.call([a, b])` takes an array of `Expr`, so
   arguments of different handle types must each be converted (`pack.call([x, y.into(),
   oz.into()])`). Generated extern functions don't have this problem, as they take
   `impl Into<Expr>` per parameter. **Possible:** a `call!` macro, or `call` taking tuples.
3. **Statement locations inside `build!`.** An error in a statement that `build!` generates points
   at the `build!` line, while an error in an operator inside it points at its own line
   (`errors::locations_inside_build`). `#[track_caller]` sees the macro's call site. A proc macro
   (§12) could fix it.
4. **`let` inside `build!` is local to the block.** A `let x: T` in one `build!` can't be read after
   it, as in legacy. In `aquifer.rs` and the macro tests, locals used across blocks are declared
   with `f.local` first.
5. **Literals have no methods.** `true.cast(F64)` doesn't compile; it needs `Expr::from(true)`. It
   only came up in tests.
6. **`as_mut_ref` can't be tested by value.** Core treats a reference as its operand, and the toy
   runtime has no extern that takes one, so the slice checks only its IR, type and text.

## Toy substitutions

The toy runtime has no Perlin noise, random-number externs or `preliminary_surface_index`.
`noise.rs` reads its tables through `perm_get(t, 7) - perm_get(t, 263)`, which is 0 for any
permutation. `top_surface.rs` writes out the index arithmetic. `aquifer.rs` mixes positions with
plain arithmetic. Octave frequencies and amplitudes are exact binary numbers, so the values can be
checked by hand.
