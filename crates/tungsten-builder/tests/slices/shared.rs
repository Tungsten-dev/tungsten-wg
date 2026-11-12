//! Program-level-inputs §2: a helper reading an input from two units, which the legacy frontend
//! can't express. Twin: `density/shared_helper.tw`.

use tungsten_builder::prelude::*;

fn build() -> Result<Program, BuildErrors> {
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("base_main", I32);
    f.ret(pos3().x() * 10 + pos3().y());
    let base = pb.finish_unit(f);

    let grid = Grid::new((4, 1, 1));
    let b = pb.input_with(InputSpec::new(base, grid).named("b"));

    let mut h = FnBuilder::new("shared", I32);
    h.ret(b);
    let shared = pb.finish_helper(h);

    let mut f = FnBuilder::new("left_main", I32);
    f.ret(shared.call([]));
    let left = pb.finish_unit(f);

    let mut f = FnBuilder::new("right_main", I32);
    f.ret(shared.call([]) + b);
    let right = pb.finish_unit(f);

    pb.entry(left, grid);
    pb.entry(right, grid);
    pb.build()
}

#[test]
fn shared() {
    super::check_slice(build(), "density/shared_helper");
}
