mod common;

use common::*;

#[test]
fn summary_example_is_accepted_by_core() {
    let p = checked(summary_example());
    println!("{}", tungsten_parse::print(&p));
}

/// The usage example of the root `README.md`.
#[test]
fn readme_usage() {
    use tungsten_builder::prelude::*;
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("height", F64);
    f.ret(pos3().y().cast(F64) * 0.5);
    let unit = pb.finish_unit(f);
    pb.entry(unit, Grid::new((16, 1, 16)));
    checked(pb);
}
