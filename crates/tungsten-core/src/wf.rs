//! Well-formedness: index bounds, as `spec fn`s only.
//!
//! `wf_program(p)` holds iff every id anywhere is in range of its table (a `LocalId` against the
//! enclosing function's `locals`, and constant initializers with no locals at all), every
//! function has `params <= locals.len()`, every type is wf (`Record(r)` in range, recursively
//! through `Array`), and every record field selector and record construction names an existing
//! field.
//!
//! Nothing type-related is checked here: types, exactly-once fields, scoping, acyclicity, dims,
//! definite assignment and density-read legality are checker rules.
//!
//! `check/wf.rs` is the exec twin (`check_wf`), and `tungsten-check`'s tests also mirror these
//! predicates in plain Rust (`src/tests/wf_mirror.rs`). Keep all three in sync.

use vstd::prelude::*;

use crate::ir::*;
use crate::registry::*;

verus! {

// ---------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------

pub open spec fn wf_type(p: &Program, t: &Type) -> bool
    decreases t,
{
    match t {
        Type::Array(inner, _) => wf_type(p, &**inner),
        Type::Record(r) => r.0 < p.records.len(),
        _ => true,
    }
}

pub open spec fn wf_types(p: &Program, ts: Seq<Type>) -> bool {
    forall|k: int| #![trigger ts[k]] 0 <= k < ts.len() ==> wf_type(p, &ts[k])
}

// ---------------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------------

pub open spec fn wf_sel(p: &Program, sel: &FieldSel) -> bool {
    match sel {
        FieldSel::Record(r, i) => r.0 < p.records.len() && *i < p.records[r.0 as int].fields.len(),
        _ => true,
    }
}

/// `nlocals` is the enclosing function's `locals.len()`, or 0 in a constant initializer.
pub open spec fn wf_expr(p: &Program, nlocals: nat, e: &Expr) -> bool
    decreases e,
{
    match e {
        Expr::Lit(_) => true,
        Expr::Local(l) => l.0 < nlocals,
        Expr::Const(c) => c.0 < p.consts.len(),
        Expr::Host(h) => h.0 < p.hosts.len(),
        Expr::Table(t) => t.0 < p.tables.len(),
        Expr::Ambient(_) => true,
        Expr::Density { input, index } => {
            &&& input.0 < p.inputs.len()
            &&& match index {
                Some(i) => wf_expr(p, nlocals, &**i),
                None => true,
            }
        },
        Expr::Call { func, args } => {
            &&& func.0 < p.functions.len()
            &&& forall|k: int| #![trigger args[k]] 0 <= k < args.len() ==> wf_expr(p, nlocals, &args[k])
        },
        Expr::ExternCall { ext, params, ret, args } => {
            &&& ext.0 < p.externs.len()
            &&& wf_types(p, params@)
            &&& wf_type(p, ret)
            &&& forall|k: int| #![trigger args[k]] 0 <= k < args.len() ==> wf_expr(p, nlocals, &args[k])
        },
        Expr::Binary { op: _, left, right } => {
            &&& wf_expr(p, nlocals, &**left)
            &&& wf_expr(p, nlocals, &**right)
        },
        Expr::Unary { op: _, operand } => wf_expr(p, nlocals, &**operand),
        Expr::Field { base, sel, ty } => {
            &&& wf_expr(p, nlocals, &**base)
            &&& wf_sel(p, sel)
            &&& wf_type(p, ty)
        },
        Expr::Index { array, index } => {
            &&& wf_expr(p, nlocals, &**array)
            &&& wf_expr(p, nlocals, &**index)
        },
        Expr::MakeVec3 { x, y, z } => {
            &&& wf_expr(p, nlocals, &**x)
            &&& wf_expr(p, nlocals, &**y)
            &&& wf_expr(p, nlocals, &**z)
        },
        Expr::MakePos3 { x, y, z } => {
            &&& wf_expr(p, nlocals, &**x)
            &&& wf_expr(p, nlocals, &**y)
            &&& wf_expr(p, nlocals, &**z)
        },
        Expr::MakeRecord { record, fields } => {
            &&& record.0 < p.records.len()
            &&& forall|k: int| #![trigger fields[k]] 0 <= k < fields.len() ==> {
                &&& fields[k].field < p.records[record.0 as int].fields.len()
                &&& wf_expr(p, nlocals, &fields[k].value)
            }
        },
        Expr::ArrayLit(elems) => {
            forall|k: int| #![trigger elems[k]] 0 <= k < elems.len() ==> wf_expr(p, nlocals, &elems[k])
        },
        Expr::Cast { to, operand } => {
            &&& wf_type(p, to)
            &&& wf_expr(p, nlocals, &**operand)
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Statements and functions
// ---------------------------------------------------------------------------------------------

pub open spec fn wf_stmt(p: &Program, nlocals: nat, s: &Stmt) -> bool
    decreases s,
{
    match s {
        Stmt::Assign { target, value } => {
            &&& target.0 < nlocals
            &&& wf_expr(p, nlocals, value)
        },
        Stmt::Return(e) => wf_expr(p, nlocals, e),
        Stmt::If { cond, then_branch, else_branch } => {
            &&& wf_expr(p, nlocals, cond)
            &&& forall|k: int| #![trigger then_branch[k]] 0 <= k < then_branch.len() ==> wf_stmt(p, nlocals, &then_branch[k])
            &&& forall|k: int| #![trigger else_branch[k]] 0 <= k < else_branch.len() ==> wf_stmt(p, nlocals, &else_branch[k])
        },
        Stmt::While { cond, body } => {
            &&& wf_expr(p, nlocals, cond)
            &&& forall|k: int| #![trigger body[k]] 0 <= k < body.len() ==> wf_stmt(p, nlocals, &body[k])
        },
        Stmt::Repeat { count: _, body } => {
            forall|k: int| #![trigger body[k]] 0 <= k < body.len() ==> wf_stmt(p, nlocals, &body[k])
        },
        Stmt::Break => true,
    }
}

pub open spec fn wf_stmts(p: &Program, nlocals: nat, s: Seq<Stmt>) -> bool {
    forall|k: int| #![trigger s[k]] 0 <= k < s.len() ==> wf_stmt(p, nlocals, &s[k])
}

pub open spec fn wf_function(p: &Program, f: &Function) -> bool {
    &&& f.params <= f.locals.len()
    &&& wf_types(p, f.locals@)
    &&& wf_type(p, &f.ret)
    &&& forall|k: int| #![trigger f.hosts[k]] 0 <= k < f.hosts.len() ==> f.hosts[k].0 < p.hosts.len()
    &&& wf_stmts(p, f.locals.len() as nat, f.body@)
}

// ---------------------------------------------------------------------------------------------
// Units, inputs, entries, declarations
// ---------------------------------------------------------------------------------------------

pub open spec fn wf_unit(p: &Program, u: &Unit) -> bool {
    &&& u.main.0 < p.functions.len()
    &&& forall|k: int| #![trigger u.inputs[k]] 0 <= k < u.inputs.len() ==> u.inputs[k].0 < p.inputs.len()
    &&& forall|k: int| #![trigger u.tables[k]] 0 <= k < u.tables.len() ==> u.tables[k].0 < p.tables.len()
    &&& forall|k: int| #![trigger u.hosts[k]] 0 <= k < u.hosts.len() ==> u.hosts[k].0 < p.hosts.len()
    &&& forall|k: int| #![trigger u.helpers[k]] 0 <= k < u.helpers.len() ==> u.helpers[k].0 < p.functions.len()
    &&& forall|k: int| #![trigger u.consts[k]] 0 <= k < u.consts.len() ==> u.consts[k].0 < p.consts.len()
}

pub open spec fn wf_input(p: &Program, i: &InputDecl) -> bool {
    i.source.0 < p.units.len()
}

pub open spec fn wf_entry(p: &Program, e: &Entry) -> bool {
    e.unit.0 < p.units.len()
}

pub open spec fn wf_extern(p: &Program, d: &ExternDecl) -> bool {
    &&& wf_types(p, d.params@)
    &&& wf_type(p, &d.ret)
}

pub open spec fn wf_record(p: &Program, d: &RecordDecl) -> bool {
    wf_types(p, d.fields@)
}

pub open spec fn wf_program(p: &Program) -> bool {
    &&& forall|k: int| #![trigger p.units[k]] 0 <= k < p.units.len() ==> wf_unit(p, &p.units[k])
    &&& forall|k: int| #![trigger p.functions[k]] 0 <= k < p.functions.len() ==> wf_function(p, &p.functions[k])
    &&& forall|k: int| #![trigger p.consts[k]] 0 <= k < p.consts.len() ==> wf_expr(p, 0, &p.consts[k].init)
    &&& forall|k: int| #![trigger p.hosts[k]] 0 <= k < p.hosts.len() ==> wf_type(p, &p.hosts[k].ty)
    &&& forall|k: int| #![trigger p.inputs[k]] 0 <= k < p.inputs.len() ==> wf_input(p, &p.inputs[k])
    &&& forall|k: int| #![trigger p.entries[k]] 0 <= k < p.entries.len() ==> wf_entry(p, &p.entries[k])
    &&& forall|k: int| #![trigger p.externs[k]] 0 <= k < p.externs.len() ==> wf_extern(p, &p.externs[k])
    &&& forall|k: int| #![trigger p.records[k]] 0 <= k < p.records.len() ==> wf_record(p, &p.records[k])
}

} // verus!
