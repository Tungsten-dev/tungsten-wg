//! Marched slices equal the full grid (`specs/refinement.md` §9), on the interpreter: each refine
//! step of `refine/march_slices_full_grid.tw` is one y layer of the `full` entry, by bits.

use tungsten_check::Value;
use tungsten_testsuite::corpus::{corpus_root, load_source};
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::oracle::run_all;

fn bits(v: &Value) -> u64 {
    match v {
        Value::F64(b) => *b,
        other => panic!("{other:?}"),
    }
}

#[test]
fn marched_slices_equal_full_grid() {
    let env = TestEnv::new();
    let path = corpus_root().join("refine/march_slices_full_grid.tw");
    let src = std::fs::read_to_string(&path).unwrap();
    let tf = load_source(&src, &env).unwrap();
    let run = run_all(&tf, &env).unwrap();
    let (dx, dy, dz) = tf.program.entry_points[0].dimensions;
    let full = &run.results[0].entries[0];
    let offsets: Vec<i32> = tf.cases[0].refines.iter().map(|c| c.offset[1]).collect();
    assert_eq!(offsets, [3, 2, 1, 0]);
    assert_eq!(run.steps[0].len(), offsets.len());
    for (step, y) in run.steps[0].iter().zip(offsets) {
        let slice = &step.entries[0];
        assert_eq!(slice.len(), (dx * dz) as usize);
        for z in 0..dz {
            for x in 0..dx {
                let want = full[(x + dx * (y + dy * z)) as usize].as_ref().unwrap();
                let got = slice[(x + dx * z) as usize].as_ref().unwrap();
                assert_eq!(bits(got), bits(want), "cell ({x}, {y}, {z})");
            }
        }
    }
}
