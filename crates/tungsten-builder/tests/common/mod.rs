#![allow(dead_code)]

use tungsten_builder::prelude::*;
use tungsten_check::{Registry, RegistryBuilder};

pub fn empty_registry() -> Registry {
    RegistryBuilder::new().build().unwrap()
}

/// Builds and runs the core checker, panicking with every message on failure.
pub fn checked(pb: ProgramBuilder) -> OptimizedProgram {
    let p = pb.build().unwrap_or_else(|e| panic!("build failed:\n{e}"));
    if let Err(e) = tungsten_check::prepare(&p, &empty_registry()) {
        panic!("core rejected the program:\n{e}\n\n{}", tungsten_parse::print(&p));
    }
    p
}

/// Parses and elaborates a `.tw` program with no externs or records.
pub fn parse(src: &str) -> OptimizedProgram {
    let file = tungsten_parse::FileId(0);
    match tungsten_parse::load(src, file, &tungsten_parse::NoEnv) {
        Ok(t) => t.program,
        Err(d) => panic!("{d:?}\n{src}"),
    }
}

/// The spec §1 example.
pub fn summary_example() -> ProgramBuilder {
    let mut pb = ProgramBuilder::new();

    let mut f = FnBuilder::new("base", F64);
    f.ret(pos3().x().cast(F64) * 10.0);
    let base = pb.finish_unit(f);

    let grid = Grid::new((4, 1, 1));
    let d = pb.input(base, grid);
    let mut h = FnBuilder::new("shared", F64);
    h.ret(d.read());
    let shared = pb.finish_helper(h);

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
    pb
}
