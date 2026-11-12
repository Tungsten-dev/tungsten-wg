//! From `tungsten-mc-compile/src/transform_spmt/find_top_surface.rs`: `build!` with `while` and
//! `break`, an indexed read of a layered input from a flat unit, and an unindexed upper bound.
//! Twin: `builder/top_surface.tw`.
//!
//! The frontend's `preliminary_surface_index` extern is replaced by its arithmetic, and its
//! types are fixed: `i` is an `i32`, so `floor(…)` is cast, and `r` is assigned `i as f64`.

use tungsten_builder::prelude::*;

extern_fns! {
    group surface_externs;
    floor(x: F64): F64,
}

struct Columns {
    dims: (i32, i32, i32),
}

#[allow(clippy::too_many_arguments)] // the frontend's own signature
fn find_top_surface(
    pb: &mut ProgramBuilder,
    work: &Columns,
    density: Unit,
    upper_bound: Unit,
    cell_height: i32,
    lower_bound: i32,
    height: i32,
    min_y: i32,
) -> Unit {
    let y_dim = height / cell_height;
    let layers = Grid::new((work.dims.0, y_dim, work.dims.2))
        .position_scale((1.0, cell_height as f64, 1.0));
    let density = pb.input(density, layers);
    let upper = pb.input(upper_bound, Grid::new(work.dims));

    let mut f = FnBuilder::new("find_top_surface", F64);
    let i = f.anon(I32);
    let j = f.anon(I32);
    let r = f.anon(F64);
    let upper_bound = upper.read();
    build!(f, {
        i = floor(upper_bound / (cell_height as f64)).cast(I32) * cell_height;
        if (i.le(lower_bound)) {
            i = lower_bound;
        } else {
            j = i / cell_height;
            i = lower_bound;
            while (j.ge(lower_bound / cell_height)) {
                let sample = density.at(pos3().x() + (j - min_y) * work.dims.0);
                if (sample.gt(0.0)) {
                    i = j * cell_height;
                    break;
                }
                j = j - 1;
            }
        }
        r = i.cast(F64);
        return r;
    });
    pb.finish_unit(f)
}

fn build() -> Result<Program, BuildErrors> {
    let mut pb = ProgramBuilder::new();
    let work = Columns { dims: (3, 1, 1) };

    let mut f = FnBuilder::new("density3d", F64);
    f.ret((pos3().x() - pos3().y()).cast(F64) + 0.5);
    let density = pb.finish_unit(f);

    let mut f = FnBuilder::new("upper", F64);
    f.ret(7.0);
    let upper = pb.finish_unit(f);

    let top = find_top_surface(&mut pb, &work, density, upper, 2, 0, 8, 0);
    pb.entry(top, Grid::new(work.dims));
    pb.build()
}

#[test]
fn top_surface() {
    super::check_slice(build(), "builder/top_surface");
    assert_eq!(surface_externs().externs[0].name, "floor");
}
