//! `check_wf`: the exec twin of `crate::wf::wf_program`, clause by clause, reporting every
//! violation with a path. Keep it in sync with `wf.rs`.

use vstd::prelude::*;

use crate::check::error::*;
use crate::ir::*;
use crate::registry::*;
use crate::util::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

struct W {
    item: CoreItem,
    path: Vec<CoreStep>,
    errs: Vec<CheckError>,
}

fn bad(w: &mut W, table: IdTable, id: usize) {
    report(&mut w.errs, CheckErrorKind::WfBadId { table, id }, w.item, &w.path);
}

fn pop(w: &mut W) {
    let _ = w.path.pop();
}

fn wf_type(p: &Program, t: &Type, w: &mut W)
    decreases t,
{
    match t {
        Type::Array(inner, _) => wf_type(p, inner, w),
        Type::Record(r) => {
            if r.0 >= p.records.len() {
                bad(w, IdTable::Record, r.0);
            }
        },
        _ => {},
    }
}

fn wf_types(p: &Program, ts: &Vec<Type>, w: &mut W, step: StepKind) {
    let mut i: usize = 0;
    while i < ts.len()
        invariant
            i <= ts.len(),
        decreases ts.len() - i,
    {
        w.path.push(mk_step(step, i));
        wf_type(p, &ts[i], w);
        pop(w);
        i = i + 1;
    }
}

fn wf_sel(p: &Program, sel: &FieldSel, w: &mut W) {
    match sel {
        FieldSel::Record(r, i) => match get(&p.records, r.0) {
            Some(d) => {
                if *i >= d.fields.len() {
                    report(&mut w.errs, CheckErrorKind::WfField { record: *r, field: *i }, w.item, &w.path);
                }
            },
            None => bad(w, IdTable::Record, r.0),
        },
        _ => {},
    }
}

#[verifier::loop_isolation(false)]
fn wf_expr(p: &Program, nlocals: usize, e: &Expr, w: &mut W)
    decreases e,
{
    match e {
        Expr::Lit(_) => {},
        Expr::Local(l) => {
            if l.0 >= nlocals {
                bad(w, IdTable::Local, l.0);
            }
        },
        Expr::Const(c) => {
            if c.0 >= p.consts.len() {
                bad(w, IdTable::Const, c.0);
            }
        },
        Expr::Host(h) => {
            if h.0 >= p.hosts.len() {
                bad(w, IdTable::Host, h.0);
            }
        },
        Expr::Table(t) => {
            if t.0 >= p.tables.len() {
                bad(w, IdTable::Table, t.0);
            }
        },
        Expr::Ambient(_) => {},
        Expr::Density { input, index } => {
            if input.0 >= p.inputs.len() {
                bad(w, IdTable::Input, input.0);
            }
            match index {
                Some(ix) => {
                    w.path.push(CoreStep::Index);
                    wf_expr(p, nlocals, ix, w);
                    pop(w);
                },
                None => {},
            }
        },
        Expr::Call { func, args } => {
            if func.0 >= p.functions.len() {
                bad(w, IdTable::Function, func.0);
            }
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                w.path.push(CoreStep::Arg(i));
                wf_expr(p, nlocals, &args[i], w);
                pop(w);
                i = i + 1;
            }
        },
        Expr::ExternCall { ext, params, ret, args } => {
            if ext.0 >= p.externs.len() {
                bad(w, IdTable::Extern, ext.0);
            }
            w.path.push(CoreStep::Type);
            wf_types(p, params, w, StepKind::ParamType);
            wf_type(p, ret, w);
            pop(w);
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                w.path.push(CoreStep::Arg(i));
                wf_expr(p, nlocals, &args[i], w);
                pop(w);
                i = i + 1;
            }
        },
        Expr::Binary { op: _, left, right } => {
            w.path.push(CoreStep::Left);
            wf_expr(p, nlocals, left, w);
            pop(w);
            w.path.push(CoreStep::Right);
            wf_expr(p, nlocals, right, w);
            pop(w);
        },
        Expr::Unary { op: _, operand } => {
            w.path.push(CoreStep::Operand);
            wf_expr(p, nlocals, operand, w);
            pop(w);
        },
        Expr::Field { base, sel, ty } => {
            wf_sel(p, sel, w);
            w.path.push(CoreStep::Type);
            wf_type(p, ty, w);
            pop(w);
            w.path.push(CoreStep::Base);
            wf_expr(p, nlocals, base, w);
            pop(w);
        },
        Expr::Index { array, index } => {
            w.path.push(CoreStep::Base);
            wf_expr(p, nlocals, array, w);
            pop(w);
            w.path.push(CoreStep::Index);
            wf_expr(p, nlocals, index, w);
            pop(w);
        },
        Expr::MakeVec3 { x, y, z } => {
            w.path.push(CoreStep::Arg(0));
            wf_expr(p, nlocals, x, w);
            pop(w);
            w.path.push(CoreStep::Arg(1));
            wf_expr(p, nlocals, y, w);
            pop(w);
            w.path.push(CoreStep::Arg(2));
            wf_expr(p, nlocals, z, w);
            pop(w);
        },
        Expr::MakePos3 { x, y, z } => {
            w.path.push(CoreStep::Arg(0));
            wf_expr(p, nlocals, x, w);
            pop(w);
            w.path.push(CoreStep::Arg(1));
            wf_expr(p, nlocals, y, w);
            pop(w);
            w.path.push(CoreStep::Arg(2));
            wf_expr(p, nlocals, z, w);
            pop(w);
        },
        Expr::MakeRecord { record, fields } => {
            let nfields: Option<usize> = match get(&p.records, record.0) {
                Some(d) => Some(d.fields.len()),
                None => None,
            };
            if nfields.is_none() {
                bad(w, IdTable::Record, record.0);
            }
            let mut i: usize = 0;
            while i < fields.len()
                invariant
                    i <= fields.len(),
                decreases fields.len() - i,
            {
                w.path.push(CoreStep::Field(*record, fields[i].field));
                match nfields {
                    Some(n) => {
                        if fields[i].field >= n {
                            report(&mut w.errs, CheckErrorKind::WfField { record: *record, field: fields[i].field }, w.item, &w.path);
                        }
                    },
                    None => {},
                }
                wf_expr(p, nlocals, &fields[i].value, w);
                pop(w);
                i = i + 1;
            }
        },
        Expr::ArrayLit(elems) => {
            let mut i: usize = 0;
            while i < elems.len()
                invariant
                    i <= elems.len(),
                decreases elems.len() - i,
            {
                w.path.push(CoreStep::Element(i));
                wf_expr(p, nlocals, &elems[i], w);
                pop(w);
                i = i + 1;
            }
        },
        Expr::Cast { to, operand } => {
            w.path.push(CoreStep::Type);
            wf_type(p, to, w);
            pop(w);
            w.path.push(CoreStep::Operand);
            wf_expr(p, nlocals, operand, w);
            pop(w);
        },
    }
}

#[verifier::loop_isolation(false)]
fn wf_stmts(p: &Program, nlocals: usize, ss: &Vec<Stmt>, w: &mut W, step: StepKind)
    decreases ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        w.path.push(mk_step(step, i));
        wf_stmt(p, nlocals, &ss[i], w);
        pop(w);
        i = i + 1;
    }
}

fn wf_stmt(p: &Program, nlocals: usize, s: &Stmt, w: &mut W)
    decreases s,
{
    match s {
        Stmt::Assign { target, value } => {
            if target.0 >= nlocals {
                bad(w, IdTable::Local, target.0);
            }
            w.path.push(CoreStep::Value);
            wf_expr(p, nlocals, value, w);
            pop(w);
        },
        Stmt::Return(e) => {
            w.path.push(CoreStep::Value);
            wf_expr(p, nlocals, e, w);
            pop(w);
        },
        Stmt::If { cond, then_branch, else_branch } => {
            w.path.push(CoreStep::Cond);
            wf_expr(p, nlocals, cond, w);
            pop(w);
            wf_stmts(p, nlocals, then_branch, w, StepKind::Then);
            wf_stmts(p, nlocals, else_branch, w, StepKind::Else);
        },
        Stmt::While { cond, body } => {
            w.path.push(CoreStep::Cond);
            wf_expr(p, nlocals, cond, w);
            pop(w);
            wf_stmts(p, nlocals, body, w, StepKind::Body);
        },
        Stmt::Repeat { count: _, body } => {
            wf_stmts(p, nlocals, body, w, StepKind::Body);
        },
        Stmt::Break => {},
    }
}

fn wf_function(p: &Program, f: &Function, w: &mut W) {
    if f.params > f.locals.len() {
        report(&mut w.errs, CheckErrorKind::WfParams { params: f.params, locals: f.locals.len() }, w.item, &w.path);
    }
    wf_types(p, &f.locals, w, StepKind::Local);
    w.path.push(CoreStep::Ret);
    wf_type(p, &f.ret, w);
    pop(w);
    let mut k: usize = 0;
    while k < f.hosts.len()
        invariant
            k <= f.hosts.len(),
        decreases f.hosts.len() - k,
    {
        if f.hosts[k].0 >= p.hosts.len() {
            w.path.push(CoreStep::List(ScopeList::Hosts, k));
            bad(w, IdTable::Host, f.hosts[k].0);
            pop(w);
        }
        k = k + 1;
    }
    wf_stmts(p, f.locals.len(), &f.body, w, StepKind::Stmt);
}

fn wf_unit(p: &Program, u: &Unit, w: &mut W) {
    if u.main.0 >= p.functions.len() {
        w.path.push(CoreStep::Main);
        bad(w, IdTable::Function, u.main.0);
        pop(w);
    }
    let mut k: usize = 0;
    while k < u.inputs.len()
        invariant
            k <= u.inputs.len(),
        decreases u.inputs.len() - k,
    {
        if u.inputs[k].0 >= p.inputs.len() {
            w.path.push(CoreStep::List(ScopeList::Inputs, k));
            bad(w, IdTable::Input, u.inputs[k].0);
            pop(w);
        }
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < u.tables.len()
        invariant
            k <= u.tables.len(),
        decreases u.tables.len() - k,
    {
        if u.tables[k].0 >= p.tables.len() {
            w.path.push(CoreStep::List(ScopeList::Tables, k));
            bad(w, IdTable::Table, u.tables[k].0);
            pop(w);
        }
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < u.hosts.len()
        invariant
            k <= u.hosts.len(),
        decreases u.hosts.len() - k,
    {
        if u.hosts[k].0 >= p.hosts.len() {
            w.path.push(CoreStep::List(ScopeList::Hosts, k));
            bad(w, IdTable::Host, u.hosts[k].0);
            pop(w);
        }
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < u.helpers.len()
        invariant
            k <= u.helpers.len(),
        decreases u.helpers.len() - k,
    {
        if u.helpers[k].0 >= p.functions.len() {
            w.path.push(CoreStep::List(ScopeList::Helpers, k));
            bad(w, IdTable::Function, u.helpers[k].0);
            pop(w);
        }
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < u.consts.len()
        invariant
            k <= u.consts.len(),
        decreases u.consts.len() - k,
    {
        if u.consts[k].0 >= p.consts.len() {
            w.path.push(CoreStep::List(ScopeList::Consts, k));
            bad(w, IdTable::Const, u.consts[k].0);
            pop(w);
        }
        k = k + 1;
    }
}

fn set_item(w: &mut W, item: CoreItem) {
    w.item = item;
    w.path = Vec::new();
}

/// Every wf violation, in `wf_program` clause order. Empty iff the program is wf.
pub fn check_wf(p: &Program) -> Vec<CheckError> {
    let mut w = W { item: CoreItem::Program, path: Vec::new(), errs: Vec::new() };
    let mut k: usize = 0;
    while k < p.units.len()
        invariant
            k <= p.units.len(),
        decreases p.units.len() - k,
    {
        set_item(&mut w, CoreItem::Unit(UnitId(k)));
        wf_unit(p, &p.units[k], &mut w);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.functions.len()
        invariant
            k <= p.functions.len(),
        decreases p.functions.len() - k,
    {
        set_item(&mut w, CoreItem::Function(FnId(k)));
        wf_function(p, &p.functions[k], &mut w);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.consts.len()
        invariant
            k <= p.consts.len(),
        decreases p.consts.len() - k,
    {
        set_item(&mut w, CoreItem::Const(ConstId(k)));
        w.path.push(CoreStep::Value);
        wf_expr(p, 0, &p.consts[k].init, &mut w);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.hosts.len()
        invariant
            k <= p.hosts.len(),
        decreases p.hosts.len() - k,
    {
        set_item(&mut w, CoreItem::Host(HostId(k)));
        w.path.push(CoreStep::Type);
        wf_type(p, &p.hosts[k].ty, &mut w);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.inputs.len()
        invariant
            k <= p.inputs.len(),
        decreases p.inputs.len() - k,
    {
        set_item(&mut w, CoreItem::Input(InputId(k)));
        if p.inputs[k].owner.0 >= p.units.len() {
            w.path.push(CoreStep::Unit);
            bad(&mut w, IdTable::Unit, p.inputs[k].owner.0);
            pop(&mut w);
        }
        if p.inputs[k].source.0 >= p.units.len() {
            w.path.push(CoreStep::Source);
            bad(&mut w, IdTable::Unit, p.inputs[k].source.0);
            pop(&mut w);
        }
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.entries.len()
        invariant
            k <= p.entries.len(),
        decreases p.entries.len() - k,
    {
        set_item(&mut w, CoreItem::Entry(k));
        if p.entries[k].unit.0 >= p.units.len() {
            w.path.push(CoreStep::Unit);
            bad(&mut w, IdTable::Unit, p.entries[k].unit.0);
            pop(&mut w);
        }
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.externs.len()
        invariant
            k <= p.externs.len(),
        decreases p.externs.len() - k,
    {
        set_item(&mut w, CoreItem::Extern(ExternId(k)));
        wf_types(p, &p.externs[k].params, &mut w, StepKind::ParamType);
        w.path.push(CoreStep::Ret);
        wf_type(p, &p.externs[k].ret, &mut w);
        pop(&mut w);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < p.records.len()
        invariant
            k <= p.records.len(),
        decreases p.records.len() - k,
    {
        set_item(&mut w, CoreItem::Record(RecordId(k)));
        wf_types(p, &p.records[k].fields, &mut w, StepKind::Element);
        k = k + 1;
    }
    // Input links, both directions.
    let mut i: usize = 0;
    while i < p.inputs.len()
        invariant
            i <= p.inputs.len(),
        decreases p.inputs.len() - i,
    {
        let owner = p.inputs[i].owner;
        let listed = match get(&p.units, owner.0) {
            Some(u) => contains_input(&u.inputs, InputId(i)),
            None => false,
        };
        if !listed {
            set_item(&mut w, CoreItem::Input(InputId(i)));
            report(&mut w.errs, CheckErrorKind::WfInputLink { input: InputId(i), unit: owner }, w.item, &w.path);
        }
        i = i + 1;
    }
    let mut u: usize = 0;
    while u < p.units.len()
        invariant
            u <= p.units.len(),
        decreases p.units.len() - u,
    {
        let unit = &p.units[u];
        let mut k: usize = 0;
        while k < unit.inputs.len()
            invariant
                k <= unit.inputs.len(),
            decreases unit.inputs.len() - k,
        {
            let ok = match get(&p.inputs, unit.inputs[k].0) {
                Some(d) => d.owner.0 == u,
                None => false,
            };
            if !ok {
                set_item(&mut w, CoreItem::Unit(UnitId(u)));
                w.path.push(CoreStep::List(ScopeList::Inputs, k));
                report(&mut w.errs, CheckErrorKind::WfInputLink { input: unit.inputs[k], unit: UnitId(u) }, w.item, &w.path);
            }
            k = k + 1;
        }
        u = u + 1;
    }
    w.errs
}

} // verus!
