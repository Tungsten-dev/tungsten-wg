//! From `tungsten-mc-compile/src/transform_spmt/density.rs` (`lower_density`,
//! `lower_density_input`): a small `Density` enum lowered recursively. A cached density opens a
//! new unit while the caller's function is still open, inputs are memoized by density, and an
//! `if`/`else` lowers into both branches with the program builder. Twin: `builder/recursive.tw`.

use std::collections::HashMap;

use tungsten_builder::prelude::*;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Density {
    /// `pos3.x` as f64.
    X,
    Const(i64),
    Add(Box<Density>, Box<Density>),
    Mul(Box<Density>, Box<Density>),
    /// `if c > 0 { a } else { b }`.
    IfPositive(Box<Density>, Box<Density>, Box<Density>),
    /// Computed by its own unit on the current grid, and read as an input.
    Cached(Box<Density>),
}

use Density::*;

fn b(d: Density) -> Box<Density> {
    Box::new(d)
}

struct Lowering {
    pb: ProgramBuilder,
    grid: Grid,
    /// The frontend's memo: one unit per cached density. Equal inputs merge in the builder.
    units: HashMap<Density, Unit>,
}

impl Lowering {
    fn lower(&mut self, f: &mut FnBuilder, d: &Density) -> Expr {
        match d {
            X => pos3().x().cast(F64),
            Const(c) => (*c as f64).into(),
            Add(a, c) => self.lower(f, a) + self.lower(f, c),
            Mul(a, c) => self.lower(f, a) * self.lower(f, c),
            IfPositive(c, a, other) => {
                let r = f.temp("r", F64);
                build!(f, {
                    if (self.lower(f, c).gt(0.0)) {
                        r = self.lower(f, a);
                    } else {
                        r = self.lower(f, other);
                    }
                });
                r.into()
            }
            Cached(inner) => {
                let unit = match self.units.get(inner.as_ref()) {
                    Some(u) => *u,
                    None => {
                        // `f` stays open while the nested unit is built.
                        let mut g = FnBuilder::new(self.pb.fresh_name("cached"), F64);
                        let value = self.lower(&mut g, inner);
                        g.ret(value);
                        let u = self.pb.finish_unit(g);
                        self.units.insert(inner.as_ref().clone(), u);
                        u
                    }
                };
                self.pb.input(unit, self.grid).read()
            }
        }
    }
}

fn build() -> Result<OptimizedProgram, BuildErrors> {
    let doubled = Cached(b(Mul(b(Cached(b(X))), b(Const(2)))));
    let tree = Add(
        b(doubled.clone()),
        b(IfPositive(b(Add(b(X), b(Const(-1)))), b(doubled), b(Const(3)))),
    );
    let grid = Grid::new((4, 1, 1));
    let mut l = Lowering {
        pb: ProgramBuilder::new(),
        grid,
        units: HashMap::new(),
    };
    let mut f = FnBuilder::new("main", F64);
    let value = l.lower(&mut f, &tree);
    f.ret(value);
    let main = l.pb.finish_unit(f);
    l.pb.entry(main, grid);
    l.pb.build()
}

#[test]
fn recursive() {
    super::check_slice(build(), "builder/recursive");
}
