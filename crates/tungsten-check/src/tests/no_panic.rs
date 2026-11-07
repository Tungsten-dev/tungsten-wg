//! `lower` never panics: corpus programs with random arena slots removed, or a `Ref` swapped for
//! one from another program. Swaps (and removing a referenced entity) must give `Err`.

use proptest::prelude::*;

use super::determinism::corpus;
use super::*;

fn nth<T>(a: &Arena<T>, k: usize) -> Option<Ref<T>> {
    let refs: Vec<Ref<T>> = a.iter().map(|(r, _)| r).collect();
    (!refs.is_empty()).then(|| refs[k % refs.len()])
}

fn remove_nth<T>(a: &mut Arena<T>, k: usize) -> bool {
    match nth(a, k) {
        Some(r) => a.remove(r).is_some(),
        None => false,
    }
}

/// Applies mutation `kind`. Returns whether the result must be `Err`.
fn mutate(p: &mut OptimizedProgram, other: &OptimizedProgram, kind: usize, k: usize) -> bool {
    match kind {
        // Random removals: may or may not leave a dangling ref, so only "no panic" is checked.
        0 => {
            remove_nth(&mut p.functions, k);
            false
        }
        1 => {
            remove_nth(&mut p.compute_units, k);
            false
        }
        2 => {
            remove_nth(&mut p.constants, k);
            false
        }
        3 => {
            remove_nth(&mut p.host_inputs, k);
            false
        }
        4 => {
            remove_nth(&mut p.permutation_tables, k);
            false
        }
        5 => {
            if let Some(f) = nth(&p.functions, k) {
                remove_nth(&mut p.functions[f].variables, k / 7);
            }
            false
        }
        6 => {
            remove_nth(&mut p.density_inputs, k);
            false
        }
        // Targeted removals of referenced entities.
        7 => match p.entry_points.first() {
            Some(e) => p.compute_units.remove(e.density_function).is_some(),
            None => false,
        },
        8 => match nth(&p.compute_units, k) {
            Some(u) => {
                let ComputeUnit::DensityFunction { function, .. } = &p.compute_units[u];
                let f = *function;
                p.functions.remove(f).is_some()
            }
            None => false,
        },
        // Swaps with refs from another program.
        9 => match (p.entry_points.first_mut(), nth(&other.compute_units, k)) {
            (Some(e), Some(u)) => {
                e.density_function = u;
                true
            }
            _ => false,
        },
        10 => match (nth(&p.compute_units, k), nth(&other.functions, k)) {
            (Some(u), Some(f)) => {
                let ComputeUnit::DensityFunction { function, .. } = &mut p.compute_units[u];
                *function = f;
                true
            }
            _ => false,
        },
        11 => {
            let foreign = nth(&other.functions, k)
                .and_then(|f| nth(&other.functions[f].variables, k / 7));
            match (nth(&p.functions, k), foreign) {
                (Some(f), Some(v)) => {
                    p.functions[f].body.insert(
                        0,
                        Statement::Assign {
                            target: v,
                            value: Expression::Double(0.0),
                        },
                    );
                    true
                }
                _ => false,
            }
        }
        _ => match (nth(&p.functions, k), nth(&other.constants, k)) {
            (Some(f), Some(c)) => {
                p.functions[f]
                    .body
                    .insert(0, Statement::Return(Expression::Constant(c)));
                true
            }
            _ => false,
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn lower_never_panics(prog in 0usize..64, other in 0usize..64, kind in 0usize..13, k in any::<usize>(), steps in 1usize..4) {
        let progs = corpus();
        let mut p = progs[prog % progs.len()].clone();
        // A separately loaded program: every arena has its own tag.
        let other = corpus().swap_remove(other % progs.len());
        let mut must_fail = false;
        for s in 0..steps {
            must_fail |= mutate(&mut p, &other, (kind + s) % 13, k.wrapping_add(s * 31));
        }
        for mode in [&registry(), &registry()] {
            let r = crate::lower(&p, mode);
            if must_fail {
                prop_assert!(r.is_err(), "expected an error");
            }
            if let Ok(l) = r {
                prop_assert!(tungsten_core::check::wf::check_wf(&l.program).is_empty());
                // The mirror agrees on the generator's output too.
                prop_assert!(wf_mirror::is_wf(&l.program));
            }
        }
    }
}
