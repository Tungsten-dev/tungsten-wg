//! Memo and fuel properties (stand-ins for the later proofs), over the checker-accepted corpus
//! and its mutants:
//!
//! - `memo: true` and `memo: false` give identical `CaseResult`s.
//! - For `f ≤ f2`, every cell that isn't `OutOfFuel` at `f` is equal at `f2`.

use proptest::prelude::*;
use tungsten_core::EvalError;
use tungsten_core::check::check_case;

use super::stuck_mutation::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn memo_off_is_identical(prog in 0usize..16, seed in any::<u64>(), steps in 0usize..3) {
        let progs = accepted();
        let p = if steps == 0 { Some(progs[prog % progs.len()].clone()) } else { mutated(&progs, prog, seed, steps) };
        if let Some(p) = p {
            let case = rand_case(&p, &mut Rng(seed));
            if check_case(&p, &case).is_ok() {
                prop_assert_eq!(eval(&p, &case, 50, true), eval(&p, &case, 50, false));
            }
        }
    }

    #[test]
    fn fuel_is_monotone(prog in 0usize..16, seed in any::<u64>(), steps in 0usize..3, f in 0u64..40, extra in 0u64..200) {
        let progs = accepted();
        let p = if steps == 0 { Some(progs[prog % progs.len()].clone()) } else { mutated(&progs, prog, seed, steps) };
        if let Some(p) = p {
            let case = rand_case(&p, &mut Rng(seed));
            if check_case(&p, &case).is_ok() {
                let a = eval(&p, &case, f, true);
                let b = eval(&p, &case, f + extra, true);
                for (ea, eb) in a.entries.iter().zip(&b.entries) {
                    for (ca, cb) in ea.iter().zip(eb) {
                        if *ca != Err(EvalError::OutOfFuel) {
                            prop_assert_eq!(ca, cb);
                        }
                    }
                }
            }
        }
    }
}
