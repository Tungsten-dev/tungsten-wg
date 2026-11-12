//! Each `body!`/`build!` form gives the same IR as its method-style equivalent (spec §10).

mod common;

use common::*;
use tungsten_builder::prelude::*;
use tungsten_ir::isomorphic;

fn finish(pb: &mut ProgramBuilder, f: FnBuilder) {
    let u = pb.finish_unit(f);
    pb.entry(u, Grid::new((1, 1, 1)));
}

fn with_macros() -> Program {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let x = f.local("x", F64);
    let i = f.local("i", I32);
    build!(f, {
        x = 1.0;
        i = 0;
        x += 2.0;
        x -= 0.5;
        x *= 3.0;
        x /= 2.0;
        let y: F64 = x * 2.0;
        let two = 2.0;
        if (y.gt(two)) {
            x = y;
        } else {
            x = two;
        }
        if (x.lt(0.0)) {
            return x;
        }
        while (i.lt(10)) {
            i += 1;
            if (i.eq_expr(5)) {
                break;
            }
        }
        repeat (3) {
            x += 1.0;
        }
    });
    let tail = body!(f, {
        let a: [F64; 2] = array![x, x];
        return a.index(1) + x;
    });
    f.append(tail);
    finish(&mut pb, f);
    checked(pb)
}

fn with_methods() -> Program {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let x = f.local("x", F64);
    let i = f.local("i", I32);
    f.assign(x, 1.0);
    f.assign(i, 0);
    f.assign(x, x + 2.0);
    f.assign(x, x - 0.5);
    f.assign(x, x * 3.0);
    f.assign(x, x / 2.0);
    let y = f.temp("y", F64);
    f.assign(y, x * 2.0);
    f.if_else(y.gt(2.0), |f| f.assign(x, y), |f| f.assign(x, 2.0));
    f.if_then(x.lt(0.0), |f| f.ret(x));
    f.while_(i.lt(10), |f| {
        f.assign(i, i + 1);
        f.if_then(i.eq_expr(5), |f| f.break_());
    });
    f.repeat(3, |f| f.assign(x, x + 1.0));
    let tail = f.block(|f| {
        let a = f.temp("a", VariableType::Array(Box::new(F64), 2));
        f.assign(a, array![x, x]);
        f.ret(a.index(1) + x);
    });
    f.append(tail);
    finish(&mut pb, f);
    checked(pb)
}

#[test]
fn macros_and_methods_build_the_same_ir() {
    let (a, b) = (with_macros(), with_methods());
    assert!(
        isomorphic(&a, &b),
        "macros:\n{}\nmethods:\n{}",
        tungsten_parse::print(&a),
        tungsten_parse::print(&b)
    );
}

/// The same macro block can run twice in one function, as in recursive lowering: `let x: T`
/// uses `temp`, so there is no `DuplicateLocal`. Both branches of an `if`/`else` can use the
/// program builder.
#[test]
fn recursion_inside_macro_blocks() {
    fn lower(pb: &mut ProgramBuilder, f: &mut FnBuilder, depth: u32) -> Expr {
        if depth == 0 {
            let mut g = FnBuilder::new(pb.fresh_name("leaf"), F64);
            g.ret(pos3().x().cast(F64));
            let unit = pb.finish_unit(g);
            return pb.input(unit, Grid::new((1, 1, 1))).read();
        }
        let r = f.temp("r", F64);
        build!(f, {
            let t: F64 = lower(pb, f, depth - 1);
            if (t.gt(0.0)) {
                r = lower(pb, f, depth - 1);
            } else {
                r = lower(pb, f, depth - 1) * 2.0;
            }
        });
        r.into()
    }
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let e = lower(&mut pb, &mut f, 2);
    f.ret(e);
    finish(&mut pb, f);
    let p = checked(pb);
    // leaf_0 … leaf_8: one unit per depth-0 call (3 × 3), plus main, all with distinct names.
    assert_eq!(p.compute_units.iter().count(), 10);
    let names: std::collections::HashSet<_> =
        p.functions.values().filter_map(|f| f.canonical_name.clone()).collect();
    assert!(names.contains("leaf_8"), "{names:?}");
}

fn input_of(pb: &mut ProgramBuilder, name: &str, value: f64) -> Input {
    let mut g = FnBuilder::new(name, F64);
    g.ret(value);
    let unit = pb.finish_unit(g);
    pb.input(unit, Grid::new((1, 1, 1)))
}

#[test]
fn if_blocks_can_use_the_program_builder_in_both_branches() {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let x = f.local("x", F64);
    f.assign(x, 1.0);
    let then = f.block(|f| {
        let read = input_of(&mut pb, "a", 2.0).read();
        f.assign(x, read);
    });
    let other = f.block(|f| {
        let read = input_of(&mut pb, "b", 3.0).read();
        f.assign(x, read);
    });
    f.if_blocks(x.gt(0.0), then, other);
    f.ret(x);
    finish(&mut pb, f);
    let methods = checked(pb);

    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    let x = f.local("x", F64);
    build!(f, {
        x = 1.0;
        if (x.gt(0.0)) {
            x = input_of(&mut pb, "a", 2.0).read();
        } else {
            x = input_of(&mut pb, "b", 3.0).read();
        }
        return x;
    });
    finish(&mut pb, f);
    let macros = checked(pb);

    assert!(
        isomorphic(&methods, &macros),
        "methods:\n{}\nmacros:\n{}",
        tungsten_parse::print(&methods),
        tungsten_parse::print(&macros)
    );
}
