//! P7, the part that doesn't need an interpreter: `permute` rewrites every ref consistently, and
//! the permuted program is still a valid program that round-trips. The interpreter / backend
//! comparison belongs to the testsuite, next to L5.

mod common;

use common::*;
use tungsten_ir::{compact, isomorphic, permute};
use tungsten_parse::*;

/// Slot-order-independent summary: sorted multiset of per-function shapes.
fn shape(p: &tungsten_ir::OptimizedProgram) -> Vec<String> {
    let mut v: Vec<String> = p
        .functions
        .values()
        .map(|f| {
            format!(
                "{:?}|{:?}|{}|{}|{}|{}",
                f.canonical_name,
                f.return_type,
                f.variables.iter().count(),
                f.parameters.len(),
                f.host_inputs.len(),
                f.body.len()
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn permuting_keeps_the_program_valid() {
    let mut moved = 0;
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let m = gen_program(&mut rng, &Options::default());
        let mut q = compact(&m);
        permute(&mut q, seed ^ 0xABCD);

        // same sizes, same per-function shapes
        assert_eq!(q.functions.iter().count(), m.functions.iter().count());
        assert_eq!(q.constants.iter().count(), m.constants.iter().count());
        assert_eq!(q.host_inputs.iter().count(), m.host_inputs.iter().count());
        assert_eq!(
            q.compute_units.iter().count(),
            m.compute_units.iter().count()
        );
        assert_eq!(q.entry_points.len(), m.entry_points.len());
        assert_eq!(shape(&q), shape(&m));

        // every ref resolves: printing panics on a dangling one
        let text = print(&q);
        let t = load(&text, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{text}"));
        assert!(isomorphic(&t.program, &q), "seed {seed}");

        // same seed, same result
        let mut q2 = compact(&m);
        permute(&mut q2, seed ^ 0xABCD);
        assert!(isomorphic(&q, &q2));

        if !isomorphic(&q, &m) {
            moved += 1;
        }
    }
    assert!(moved > 100, "permute rarely changed anything ({moved}/200)");
}

#[test]
fn isomorphic_is_not_vacuous() {
    let mut rng = Rng::new(5);
    let a = gen_program(&mut rng, &Options::default());
    let mut b = compact(&a);
    assert!(isomorphic(&a, &b));
    b.entry_points.push(
        b.entry_points
            .first()
            .cloned()
            .unwrap_or(tungsten_ir::EntryPoint {
                density_function: b.compute_units.iter().next().unwrap().0,
                dimensions: (1, 1, 1),
                scaled_origin: (0.0, 0.0, 0.0),
                scaled_position: (0.0, 0.0, 0.0),
            }),
    );
    assert!(!isomorphic(&a, &b));
    // a different ref target with identical values is not isomorphic either
    let mut c = compact(&a);
    let mut d = compact(&a);
    if let Some(f) = c.functions.values_mut().next() {
        f.source_hash ^= 1;
    }
    assert!(!isomorphic(&c, &d));
    d.functions.values_mut().next().unwrap().source_hash ^= 1;
    assert!(isomorphic(&c, &d));
}

#[test]
fn isomorphic_compares_floats_by_bits() {
    let src = |c: &str| format!("program {{ const c = {c}; }}");
    let load = |s: String| load(&s, FileId(0), &NoEnv).unwrap().program;
    assert!(!isomorphic(
        &load(src("nan")),
        &load(src("nan(0x7ff8000000000001)"))
    ));
    assert!(!isomorphic(&load(src("0.0")), &load(src("-0.0"))));
    assert!(isomorphic(
        &load(src("nan")),
        &load(src("nan(0x7ff8000000000000)"))
    ));
}
