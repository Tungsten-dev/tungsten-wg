//! Memo and fuel properties, over the checker-accepted corpus and its mutants:
//!
//! - `memo: true` and `memo: false` give identical `CaseResult`s. For cells this is proven
//!   (`tungsten-core`'s T1), but `failures`' discovery order is only tested here.
//! - For `f ≤ f2`, every cell that isn't `OutOfFuel` at `f` is equal at `f2`. Also proven (T2);
//!   this runs it with the real primitives and externs.

use proptest::prelude::*;
use tungsten_core::check::check_case;
use tungsten_core::*;

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

/// A cell's result can't change kind with fuel. Found by the T2 proof: with a const that runs out of
/// fuel, an entry with no layout was `Stuck(Layout)`, but with enough fuel and a unit on a cycle the
/// same cell was `Stuck(Rank)`, because `eval_entry` checked the rank first.
#[test]
fn no_layout_is_stuck_layout_at_every_fuel() {
    let loop_fn = Function {
        params: 0,
        locals: vec![Type::I32],
        ret: Type::I32,
        hosts: vec![],
        body: vec![
            Stmt::Assign { target: LocalId(0), value: Expr::Lit(Lit::I32(0)) },
            Stmt::While {
                cond: Expr::Binary {
                    op: BinOp::Less,
                    left: Box::new(Expr::Local(LocalId(0))),
                    right: Box::new(Expr::Lit(Lit::I32(1))),
                },
                body: vec![Stmt::Assign {
                    target: LocalId(0),
                    value: Expr::Binary {
                        op: BinOp::Add,
                        left: Box::new(Expr::Local(LocalId(0))),
                        right: Box::new(Expr::Lit(Lit::I32(1))),
                    },
                }],
            },
            Stmt::Return(Expr::Local(LocalId(0))),
        ],
    };
    // Calls itself, so its unit has no rank.
    let cyclic = Function {
        params: 0,
        locals: vec![],
        ret: Type::I32,
        hosts: vec![],
        body: vec![Stmt::Return(Expr::Call { func: FnId(1), args: vec![] })],
    };
    let zero = F64x3 { x: 0, y: 0, z: 0 };
    let p = Program {
        units: vec![Unit { main: FnId(1), inputs: vec![], tables: vec![], hosts: vec![], helpers: vec![], consts: vec![] }],
        functions: vec![loop_fn, cyclic],
        consts: vec![ConstDecl { init: Expr::Call { func: FnId(0), args: vec![] } }],
        hosts: vec![],
        tables: vec![],
        inputs: vec![],
        entries: vec![Entry {
            unit: UnitId(0),
            dims: Dims { x: -1, y: 1, z: 1 },
            origin_scale: zero.clone(),
            position_scale: zero.clone(),
        }],
        externs: vec![],
        records: vec![],
    };
    let case = Case { origin: zero, hosts: vec![] };
    let layout = vec![Err(EvalError::Stuck(StuckReason::Layout))];
    assert_eq!(eval(&p, &case, 0, true).entries[0], layout, "the const runs out of fuel");
    assert_eq!(eval(&p, &case, 10, true).entries[0], layout, "the const is fine");
}
