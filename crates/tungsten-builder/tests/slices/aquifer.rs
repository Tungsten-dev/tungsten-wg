//! From `tungsten-mc-compile/src/transform_spmt/aquifer.rs` (`aquifer_random` and the surface
//! sampling loop): helpers with parameters, a static host and a record host, a constant array,
//! a body assembled from several `body!` blocks, and `as_mut_ref`. Twin: `builder/aquifer.tw`.
//!
//! The frontend's random-number externs aren't in the toy runtime, so the cell position is mixed
//! with plain arithmetic (`cell_to_block`, `pack`), and the splitter is the registry's `Pair`.

use tungsten_builder::prelude::*;

fn build() -> Result<OptimizedProgram, BuildErrors> {
    let mut pb = ProgramBuilder::new();
    let start_y = pb.host("start_y", I32);
    let splitter = pb.host("splitter", vt!("Pair"));
    let offsets = pb.constant("surface_sample_offsets", array![0, 0, -2, -1, -1, -1]);

    let mut h = FnBuilder::new("cell_to_block", I32);
    let c = h.param("c", I32);
    let r = h.param("r", I32);
    h.ret(c * 4 + r);
    let cell_to_block = pb.finish_helper(h);

    let mut h = FnBuilder::new("pack", I64);
    let x = h.param("x", I32);
    let y = h.param("y", I32);
    let z = h.param("z", I32);
    h.ret((x * 100 + y * 10 + z).cast(I64));
    let pack = pb.finish_helper(h);

    let mut f = FnBuilder::new("aquifer_random", I64);
    let x = f.local("x", I32);
    let y = f.local("y", I32);
    let k = f.local("k", I32);
    let acc = f.local("acc", I64);

    // As in the frontend, the body is assembled from separately built blocks.
    let head = body!(f, {
        x = pos3().x();
        y = start_y + pos3().y();
        acc = 0i64;
        k = 0;
    });
    let samples = body!(f, {
        while (k.lt(3)) {
            let ox: I32 = offsets.index(k * 2);
            let oz: I32 = offsets.index(k * 2 + 1);
            let block_x = cell_to_block.call([x + ox, splitter.field("b", I32)]);
            acc += pack.call([block_x, y.into(), oz.into()]);
            k += 1;
        }
    });
    let tail = body!(f, {
        if (splitter.field("a", F64).gt(1.0)) {
            acc += 1i64;
        }
        return acc;
    });
    f.append(head);
    f.append(samples);
    f.append(tail);
    let unit = pb.finish_unit(f);
    pb.entry(unit, Grid::new((1, 1, 1)));
    pb.build()
}

#[test]
fn aquifer() {
    super::check_slice(build(), "builder/aquifer");
}

/// `as_mut_ref`, as the frontend passes its random state to `random_next_int`. Core treats a
/// reference as its operand, and no toy extern takes one, so this is checked as IR and text
/// only: it gives `&mut (e)`, has its operand's type, and prints and parses back.
#[test]
fn as_mut_ref_passes_through() {
    extern_fns! {
        random_next_int(r: "Pair", bound: I32): I32,
    }
    let mut pb = ProgramBuilder::new();
    let rnd = pb.host("rnd", vt!("Pair"));
    let mut f = FnBuilder::new("m", I32);
    let state = f.local("state", vt!("Pair"));
    let e = state.as_mut_ref();
    assert_eq!(e.ty(), Some(&vt!("Pair")));
    build!(f, {
        state = rnd;
        return random_next_int(e, 10) + random_next_int(state.as_mut_ref(), 9);
    });
    pb.finish_unit(f);
    let p = pb.build().unwrap_or_else(|e| panic!("{e}"));
    let text = tungsten_parse::print(&p);
    assert!(text.contains("ext random_next_int::<(extern Pair, i32) -> i32>(&mut (state), 10)"), "{text}");
    let env = tungsten_testsuite::env::TestEnv::new();
    let back = tungsten_testsuite::corpus::load_source(&text, &env).unwrap();
    assert!(tungsten_ir::isomorphic(&p, &back.program));
}
