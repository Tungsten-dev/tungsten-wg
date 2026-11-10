//! A plain-Rust mirror of `tungsten_core::wf`, clause by clause, and the agreement test against
//! the exec twin `tungsten_core::check::wf::check_wf` (which `ok()` now runs).

use tungsten_core::*;

fn wf_type(p: &Program, t: &Type) -> bool {
    match t {
        Type::Array(inner, _) => wf_type(p, inner),
        Type::Record(r) => r.0 < p.records.len(),
        _ => true,
    }
}

fn wf_types(p: &Program, ts: &[Type]) -> bool {
    ts.iter().all(|t| wf_type(p, t))
}

fn wf_sel(p: &Program, sel: &FieldSel) -> bool {
    match sel {
        FieldSel::Record(r, i) => r.0 < p.records.len() && *i < p.records[r.0].fields.len(),
        _ => true,
    }
}

fn wf_expr(p: &Program, nlocals: usize, e: &Expr) -> bool {
    let w = |e: &Expr| wf_expr(p, nlocals, e);
    match e {
        Expr::Lit(_) | Expr::Ambient(_) => true,
        Expr::Local(l) => l.0 < nlocals,
        Expr::Const(c) => c.0 < p.consts.len(),
        Expr::Host(h) => h.0 < p.hosts.len(),
        Expr::Table(t) => t.0 < p.tables.len(),
        Expr::Density { input, index } => {
            input.0 < p.inputs.len() && index.as_deref().is_none_or(w)
        }
        Expr::Call { func, args } => func.0 < p.functions.len() && args.iter().all(w),
        Expr::ExternCall {
            ext,
            params,
            ret,
            args,
        } => {
            ext.0 < p.externs.len()
                && wf_types(p, params)
                && wf_type(p, ret)
                && args.iter().all(w)
        }
        Expr::Binary { left, right, .. } => w(left) && w(right),
        Expr::Unary { operand, .. } => w(operand),
        Expr::Field { base, sel, ty } => w(base) && wf_sel(p, sel) && wf_type(p, ty),
        Expr::Index { array, index } => w(array) && w(index),
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => w(x) && w(y) && w(z),
        Expr::MakeRecord { record, fields } => {
            record.0 < p.records.len()
                && fields
                    .iter()
                    .all(|f| f.field < p.records[record.0].fields.len() && w(&f.value))
        }
        Expr::ArrayLit(elems) => elems.iter().all(w),
        Expr::Cast { to, operand } => wf_type(p, to) && w(operand),
    }
}

fn wf_stmt(p: &Program, nlocals: usize, s: &Stmt) -> bool {
    let all = |b: &[Stmt]| b.iter().all(|s| wf_stmt(p, nlocals, s));
    match s {
        Stmt::Assign { target, value } => target.0 < nlocals && wf_expr(p, nlocals, value),
        Stmt::Return(e) => wf_expr(p, nlocals, e),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
        } => wf_expr(p, nlocals, cond) && all(then_branch) && all(else_branch),
        Stmt::While { cond, body } => wf_expr(p, nlocals, cond) && all(body),
        Stmt::Repeat { body, .. } => all(body),
        Stmt::Break => true,
    }
}

fn wf_function(p: &Program, f: &Function) -> bool {
    f.params <= f.locals.len()
        && wf_types(p, &f.locals)
        && wf_type(p, &f.ret)
        && f.hosts.iter().all(|h| h.0 < p.hosts.len())
        && f.body.iter().all(|s| wf_stmt(p, f.locals.len(), s))
}

fn wf_unit(p: &Program, u: &Unit) -> bool {
    u.main.0 < p.functions.len()
        && u.inputs.iter().all(|i| i.0 < p.inputs.len())
        && u.tables.iter().all(|t| t.0 < p.tables.len())
        && u.hosts.iter().all(|h| h.0 < p.hosts.len())
        && u.helpers.iter().all(|f| f.0 < p.functions.len())
        && u.consts.iter().all(|c| c.0 < p.consts.len())
}

fn wf_input_links(p: &Program) -> bool {
    p.inputs.iter().enumerate().all(|(i, d)| {
        p.units
            .get(d.owner.0)
            .is_some_and(|u| u.inputs.contains(&InputId(i)))
    }) && p.units.iter().enumerate().all(|(u, unit)| {
        unit.inputs
            .iter()
            .all(|i| p.inputs.get(i.0).is_some_and(|d| d.owner == UnitId(u)))
    })
}

/// `wf_program(p)`.
pub fn is_wf(p: &Program) -> bool {
    p.units.iter().all(|u| wf_unit(p, u))
        && p.functions.iter().all(|f| wf_function(p, f))
        && p.consts.iter().all(|c| wf_expr(p, 0, &c.init))
        && p.hosts.iter().all(|h| wf_type(p, &h.ty))
        && p.inputs.iter().all(|d| d.owner.0 < p.units.len() && d.source.0 < p.units.len())
        && p.entries.iter().all(|e| e.unit.0 < p.units.len())
        && p.externs.iter().all(|d| wf_types(p, &d.params) && wf_type(p, &d.ret))
        && p.records.iter().all(|d| wf_types(p, &d.fields))
        && wf_input_links(p)
}

#[test]
fn mirror_rejects_out_of_range_ids() {
    let p = Program {
        units: vec![],
        functions: vec![Function {
            params: 0,
            locals: vec![],
            ret: Type::F64,
            hosts: vec![],
            body: vec![Stmt::Return(Expr::Local(LocalId(0)))],
        }],
        consts: vec![],
        hosts: vec![],
        tables: vec![],
        inputs: vec![],
        entries: vec![],
        externs: vec![],
        records: vec![],
    };
    assert!(!wf_function(&p, &p.functions[0]));
    assert!(!wf_expr(&p, 0, &Expr::Field {
        base: Box::new(Expr::Lit(Lit::Bool(true))),
        sel: FieldSel::Record(RecordId(0), 0),
        ty: Type::F64,
    }));
    assert!(!wf_type(&p, &Type::Array(Box::new(Type::Record(RecordId(3))), 1)));
}

/// `check_wf` (the exec twin in tungsten-core) agrees with this mirror on lowered programs and
/// on programs with ids broken at random.
mod agreement {
    use proptest::prelude::*;
    use tungsten_core::check::wf::check_wf;
    use tungsten_core::*;

    use super::super::stuck_mutation::{Rng, accepted};
    use super::is_wf;

    fn break_ids(p: &mut Program, rng: &mut Rng) {
        let big = 7 + rng.below(3);
        match rng.below(9) {
            0 => {
                if let Some(f) = p.functions.first_mut() {
                    f.params = f.locals.len() + 1;
                }
            }
            1 => {
                if let Some(f) = p.functions.last_mut() {
                    f.body.push(Stmt::Return(Expr::Local(LocalId(f.locals.len() + big))));
                }
            }
            2 => {
                if let Some(u) = p.units.first_mut() {
                    u.main = FnId(big + 100);
                }
            }
            3 => {
                let n = p.units.len();
                if let (true, Some(d)) = (n > 1, p.inputs.first_mut()) {
                    d.owner = UnitId((d.owner.0 + 1) % n);
                }
            }
            4 => p.hosts.push(HostDecl { ty: Type::Array(Box::new(Type::Record(RecordId(big))), 1) }),
            5 => p.entries.push(Entry {
                unit: UnitId(p.units.len() + big),
                dims: Dims { x: 1, y: 1, z: 1 },
                origin_scale: F64x3 { x: 0, y: 0, z: 0 },
                position_scale: F64x3 { x: 0, y: 0, z: 0 },
            }),
            6 => p.consts.push(ConstDecl {
                init: Expr::Field {
                    base: Box::new(Expr::Lit(Lit::I32(0))),
                    sel: FieldSel::Record(RecordId(0), big),
                    ty: Type::F64,
                },
            }),
            7 => {
                if let Some(u) = p.units.last_mut() {
                    u.consts.push(ConstId(p.consts.len() + big));
                }
            }
            _ => {
                if let Some(f) = p.functions.first_mut() {
                    f.hosts.push(HostId(p.hosts.len()));
                }
            }
        }
    }

    #[test]
    fn lowered_programs_agree() {
        for p in accepted() {
            assert!(is_wf(&p));
            assert!(check_wf(&p).is_empty());
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn broken_programs_agree(prog in 0usize..16, seed in any::<u64>(), steps in 0usize..3) {
            let progs = accepted();
            let mut p = progs[prog % progs.len()].clone();
            let mut rng = Rng(seed);
            for _ in 0..steps {
                break_ids(&mut p, &mut rng);
            }
            prop_assert_eq!(is_wf(&p), check_wf(&p).is_empty(), "{:?}", p);
        }
    }
}
