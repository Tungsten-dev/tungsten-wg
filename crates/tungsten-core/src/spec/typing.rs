//! What it means for a program to be well typed (CHECK.md §4, §5), as the soundness theorem uses it:
//! only the rules no-`Stuck` depends on. Scoping and annotation checks are the checker's business
//! and are not part of `well_typed`.

use vstd::prelude::*;

use crate::ir::*;
use crate::spec::eval::*;
use crate::spec::layout::*;
use crate::spec::ops::*;
use crate::spec::ranks::*;
use crate::spec::value::*;
use crate::value::*;
use crate::wf::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub open spec fn spec_is_numeric(t: Type) -> bool {
    match t {
        Type::I8 | Type::I32 | Type::I64 | Type::F32 | Type::F64 => true,
        _ => false,
    }
}

pub open spec fn spec_is_integer(t: Type) -> bool {
    match t {
        Type::I8 | Type::I32 | Type::I64 => true,
        _ => false,
    }
}

pub open spec fn spec_lit_type(l: Lit) -> Type {
    match l {
        Lit::Bool(_) => Type::Bool,
        Lit::I8(_) => Type::I8,
        Lit::I32(_) => Type::I32,
        Lit::I64(_) => Type::I64,
        Lit::F32(_) => Type::F32,
        Lit::F64(_) => Type::F64,
    }
}

/// CHECK.md §4.2.
pub open spec fn spec_binary_type(op: BinOp, l: Type, r: Type) -> Option<Type> {
    match op {
        BinOp::Add | BinOp::Subtract | BinOp::Multiply | BinOp::Divide => {
            if spec_is_numeric(l) && l == r {
                Some(l)
            } else {
                match (l, r) {
                    (Type::Vec3, Type::Vec3) => Some(Type::Vec3),
                    (Type::Pos3, Type::Pos3) => Some(Type::Pos3),
                    (Type::Vec3, Type::Pos3) => Some(Type::Vec3),
                    (Type::Pos3, Type::Vec3) => Some(Type::Vec3),
                    _ => None,
                }
            }
        },
        BinOp::Equal | BinOp::NotEqual => if (l is Bool || spec_is_numeric(l)) && l == r { Some(Type::Bool) } else { None },
        BinOp::Less | BinOp::LessEqual | BinOp::Greater | BinOp::GreaterEqual => if spec_is_numeric(l) && l == r { Some(Type::Bool) } else { None },
        BinOp::And | BinOp::Or => if l is Bool && r is Bool { Some(Type::Bool) } else { None },
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => if (l is Bool || spec_is_integer(l)) && l == r { Some(l) } else { None },
        BinOp::ShiftLeft | BinOp::ShiftRight | BinOp::LogicalShiftRight => if spec_is_integer(l) && r is I32 { Some(l) } else { None },
    }
}

pub open spec fn spec_unary_type(op: UnOp, t: Type) -> Option<Type> {
    match op {
        UnOp::Not => if t is Bool || spec_is_integer(t) { Some(t) } else { None },
        UnOp::Negate => if t is Vec3 || t is Pos3 || spec_is_numeric(t) { Some(t) } else { None },
    }
}

/// CHECK.md §4.3.
pub open spec fn spec_cast_ok(from: Type, to: Type) -> bool {
    (spec_is_numeric(from) && spec_is_numeric(to))
        || (from is Bool && spec_is_integer(to))
        || (from is Pos3 && to is Vec3)
}

/// The return type of the unit an input reads.
pub open spec fn spec_density_type(p: Program, i: InputId) -> Option<Type> {
    if i.0 < p.inputs@.len() && p.inputs@[i.0 as int].source.0 < p.units@.len()
        && p.units@[p.inputs@[i.0 as int].source.0 as int].main.0 < p.functions@.len() {
        Some(p.functions@[p.units@[p.inputs@[i.0 as int].source.0 as int].main.0 as int].ret)
    } else {
        None
    }
}

pub open spec fn spec_field_type(p: Program, sel: FieldSel, base: Type) -> Option<Type> {
    match (sel, base) {
        (FieldSel::Vec3(_), Type::Vec3) => Some(Type::F64),
        (FieldSel::Pos3(_), Type::Pos3) => Some(Type::I32),
        (FieldSel::Record(r, i), Type::Record(s)) => {
            if r.0 == s.0 && r.0 < p.records@.len() && i < p.records@[r.0 as int].fields@.len() {
                Some(p.records@[r.0 as int].fields@[i as int])
            } else {
                None
            }
        },
        _ => None,
    }
}

/// Some written field is field `j`.
pub open spec fn spec_field_given(fields: Seq<FieldInit>, j: int) -> bool {
    exists|a: int| 0 <= a < fields.len() && #[trigger] fields[a].field == j
}

/// The type of `e`, or `None` if `e` or anything in it is ill typed (CHECK.md §4.1). `ctys` holds
/// each constant's derived type.
pub open spec fn spec_type_of(p: Program, locals: Seq<Type>, ctys: Seq<Option<Type>>, e: Expr) -> Option<Type>
    decreases e,
{
    match e {
        Expr::Lit(l) => Some(spec_lit_type(l)),
        Expr::Local(l) => if l.0 < locals.len() { Some(locals[l.0 as int]) } else { None },
        Expr::Const(c) => if c.0 < ctys.len() { ctys[c.0 as int] } else { None },
        Expr::Host(h) => if h.0 < p.hosts@.len() { Some(p.hosts@[h.0 as int].ty) } else { None },
        Expr::Table(t) => {
            if t.0 < p.tables@.len() {
                match p.tables@[t.0 as int].kind {
                    TableKind::Perm => Some(Type::PermTable),
                    TableKind::Sampler => Some(Type::Sampler),
                }
            } else {
                None
            }
        },
        Expr::Ambient(a) => match a {
            Ambient::Pos3 => Some(Type::Pos3),
            _ => Some(Type::Vec3),
        },
        Expr::Density { input, index } => match index {
            Some(ix) => if spec_type_of(p, locals, ctys, *ix) == Some(Type::I32) { spec_density_type(p, input) } else { None },
            None => spec_density_type(p, input),
        },
        Expr::Call { func, args } => {
            if func.0 < p.functions@.len() {
                let f = p.functions@[func.0 as int];
                if args@.len() == f.params
                    && (forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_type_of(p, locals, ctys, args@[k]) == Some(f.locals@[k])) {
                    Some(f.ret)
                } else {
                    None
                }
            } else {
                None
            }
        },
        Expr::ExternCall { ext, args, .. } => {
            if ext.0 < p.externs@.len() {
                let d = p.externs@[ext.0 as int];
                if args@.len() == d.params@.len()
                    && (forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_type_of(p, locals, ctys, args@[k]) == Some(d.params@[k])) {
                    Some(d.ret)
                } else {
                    None
                }
            } else {
                None
            }
        },
        Expr::Binary { op, left, right } => match (spec_type_of(p, locals, ctys, *left), spec_type_of(p, locals, ctys, *right)) {
            (Some(l), Some(r)) => spec_binary_type(op, l, r),
            _ => None,
        },
        Expr::Unary { op, operand } => match spec_type_of(p, locals, ctys, *operand) {
            Some(t) => spec_unary_type(op, t),
            None => None,
        },
        Expr::Field { base, sel, .. } => match spec_type_of(p, locals, ctys, *base) {
            Some(b) => spec_field_type(p, sel, b),
            None => None,
        },
        Expr::Index { array, index } => match (spec_type_of(p, locals, ctys, *array), spec_type_of(p, locals, ctys, *index)) {
            (Some(Type::Array(inner, _)), Some(Type::I32)) => Some(*inner),
            _ => None,
        },
        Expr::MakeVec3 { x, y, z } => {
            if spec_type_of(p, locals, ctys, *x) == Some(Type::F64) && spec_type_of(p, locals, ctys, *y) == Some(Type::F64)
                && spec_type_of(p, locals, ctys, *z) == Some(Type::F64) {
                Some(Type::Vec3)
            } else {
                None
            }
        },
        Expr::MakePos3 { x, y, z } => {
            if spec_type_of(p, locals, ctys, *x) == Some(Type::I32) && spec_type_of(p, locals, ctys, *y) == Some(Type::I32)
                && spec_type_of(p, locals, ctys, *z) == Some(Type::I32) {
                Some(Type::Pos3)
            } else {
                None
            }
        },
        Expr::MakeRecord { record, fields } => {
            if record.0 < p.records@.len() {
                let d = p.records@[record.0 as int];
                if (forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() ==> {
                        &&& fields@[k].field < d.fields@.len()
                        &&& spec_type_of(p, locals, ctys, fields@[k].value) == Some(d.fields@[fields@[k].field as int])
                    })
                    // Every field exactly once.
                    && (forall|a: int, b: int| #![trigger fields@[a], fields@[b]] 0 <= a < b < fields@.len() ==> fields@[a].field != fields@[b].field)
                    && (forall|j: int| #![trigger spec_field_given(fields@, j)] 0 <= j < d.fields@.len() ==> spec_field_given(fields@, j)) {
                    Some(Type::Record(record))
                } else {
                    None
                }
            } else {
                None
            }
        },
        Expr::ArrayLit(es) => {
            if es@.len() > 0 {
                match spec_type_of(p, locals, ctys, es@[0]) {
                    Some(f) => {
                        if forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() ==> spec_type_of(p, locals, ctys, es@[k]) == Some(f) {
                            Some(Type::Array(Box::new(f), es@.len() as usize))
                        } else {
                            None
                        }
                    },
                    None => None,
                }
            } else {
                None
            }
        },
        Expr::Cast { to, operand } => match spec_type_of(p, locals, ctys, *operand) {
            Some(t) => if spec_cast_ok(t, to) { Some(to) } else { None },
            None => None,
        },
    }
}

/// Every constant `e` reads has a derived type. The checker's "unknown" type is silent, so this is
/// what separates "no error reported" from "well typed".
pub open spec fn spec_reads_known(ctys: Seq<Option<Type>>, e: Expr) -> bool
    decreases e,
{
    match e {
        Expr::Lit(_) | Expr::Local(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => true,
        Expr::Const(c) => c.0 < ctys.len() && ctys[c.0 as int] is Some,
        Expr::Density { index, .. } => match index {
            Some(ix) => spec_reads_known(ctys, *ix),
            None => true,
        },
        Expr::Call { args, .. } => forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_reads_known(ctys, args@[k]),
        Expr::ExternCall { args, .. } => forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_reads_known(ctys, args@[k]),
        Expr::Binary { left, right, .. } => spec_reads_known(ctys, *left) && spec_reads_known(ctys, *right),
        Expr::Unary { operand, .. } => spec_reads_known(ctys, *operand),
        Expr::Field { base, .. } => spec_reads_known(ctys, *base),
        Expr::Index { array, index } => spec_reads_known(ctys, *array) && spec_reads_known(ctys, *index),
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => spec_reads_known(ctys, *x) && spec_reads_known(ctys, *y) && spec_reads_known(ctys, *z),
        Expr::MakeRecord { fields, .. } => forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() ==> spec_reads_known(ctys, fields@[k].value),
        Expr::ArrayLit(es) => forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() ==> spec_reads_known(ctys, es@[k]),
        Expr::Cast { operand, .. } => spec_reads_known(ctys, *operand),
    }
}

pub open spec fn spec_stmt_reads_known(ctys: Seq<Option<Type>>, s: Stmt) -> bool
    decreases s,
{
    match s {
        Stmt::Assign { value, .. } => spec_reads_known(ctys, value),
        Stmt::Return(e) => spec_reads_known(ctys, e),
        Stmt::If { cond, then_branch, else_branch } => spec_reads_known(ctys, cond)
            && (forall|k: int| #![trigger then_branch@[k]] 0 <= k < then_branch@.len() ==> spec_stmt_reads_known(ctys, then_branch@[k]))
            && (forall|k: int| #![trigger else_branch@[k]] 0 <= k < else_branch@.len() ==> spec_stmt_reads_known(ctys, else_branch@[k])),
        Stmt::While { cond, body } => spec_reads_known(ctys, cond)
            && (forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() ==> spec_stmt_reads_known(ctys, body@[k])),
        Stmt::Repeat { body, .. } => forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() ==> spec_stmt_reads_known(ctys, body@[k]),
        Stmt::Break => true,
    }
}

/// CHECK.md §4.4.
pub open spec fn spec_stmt_typed(p: Program, locals: Seq<Type>, ctys: Seq<Option<Type>>, ret: Type, s: Stmt) -> bool
    decreases s,
{
    match s {
        Stmt::Assign { target, value } => target.0 < locals.len() && spec_type_of(p, locals, ctys, value) == Some(locals[target.0 as int]),
        Stmt::Return(e) => spec_type_of(p, locals, ctys, e) == Some(ret),
        Stmt::If { cond, then_branch, else_branch } => spec_type_of(p, locals, ctys, cond) == Some(Type::Bool)
            && (forall|k: int| #![trigger then_branch@[k]] 0 <= k < then_branch@.len() ==> spec_stmt_typed(p, locals, ctys, ret, then_branch@[k]))
            && (forall|k: int| #![trigger else_branch@[k]] 0 <= k < else_branch@.len() ==> spec_stmt_typed(p, locals, ctys, ret, else_branch@[k])),
        Stmt::While { cond, body } => spec_type_of(p, locals, ctys, cond) == Some(Type::Bool)
            && (forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() ==> spec_stmt_typed(p, locals, ctys, ret, body@[k])),
        Stmt::Repeat { body, .. } => forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() ==> spec_stmt_typed(p, locals, ctys, ret, body@[k]),
        Stmt::Break => true,
    }
}

pub open spec fn spec_function_typed(p: Program, ctys: Seq<Option<Type>>, f: Function) -> bool {
    forall|k: int| #![trigger f.body@[k]] 0 <= k < f.body@.len() ==> spec_stmt_typed(p, f.locals@, ctys, f.ret, f.body@[k])
}

// ---------------------------------------------------------------------------------------------
// Flow (CHECK.md §5)
// ---------------------------------------------------------------------------------------------

/// `crate::check::flow::FState` without its reporting state.
pub struct SFlowState {
    pub reach: bool,
    pub assigned: Seq<bool>,
    pub in_loop: bool,
}

/// Every local `e` reads is definitely assigned.
pub open spec fn spec_reads_assigned(assigned: Seq<bool>, e: Expr) -> bool
    decreases e,
{
    match e {
        Expr::Local(l) => l.0 < assigned.len() && assigned[l.0 as int],
        Expr::Lit(_) | Expr::Const(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => true,
        Expr::Density { index, .. } => match index {
            Some(ix) => spec_reads_assigned(assigned, *ix),
            None => true,
        },
        Expr::Call { args, .. } => forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_reads_assigned(assigned, args@[k]),
        Expr::ExternCall { args, .. } => forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_reads_assigned(assigned, args@[k]),
        Expr::Binary { left, right, .. } => spec_reads_assigned(assigned, *left) && spec_reads_assigned(assigned, *right),
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => spec_reads_assigned(assigned, *operand),
        Expr::Field { base, .. } => spec_reads_assigned(assigned, *base),
        Expr::Index { array, index } => spec_reads_assigned(assigned, *array) && spec_reads_assigned(assigned, *index),
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => spec_reads_assigned(assigned, *x) && spec_reads_assigned(assigned, *y) && spec_reads_assigned(assigned, *z),
        Expr::MakeRecord { fields, .. } => forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() ==> spec_reads_assigned(assigned, fields@[k].value),
        Expr::ArrayLit(es) => forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() ==> spec_reads_assigned(assigned, es@[k]),
    }
}

pub open spec fn spec_expr_flows(st: SFlowState, e: Expr) -> bool {
    st.reach ==> spec_reads_assigned(st.assigned, e)
}

/// The assigned sets after an `if`. Only reachable branches count (CHECK.md §5.1).
pub open spec fn spec_flow_merge(st: SFlowState, t: SFlowState, e: SFlowState) -> SFlowState {
    if !st.reach {
        st
    } else if !t.reach && !e.reach {
        SFlowState { reach: false, ..st }
    } else if !t.reach {
        SFlowState { assigned: e.assigned, ..st }
    } else if !e.reach {
        SFlowState { assigned: t.assigned, ..st }
    } else {
        SFlowState { assigned: Seq::new(t.assigned.len(), |i: int| t.assigned[i] && i < e.assigned.len() && e.assigned[i]), ..st }
    }
}

/// `ss[i..]` from state `st`: the state after them, or `None` if flow rejects them.
pub open spec fn spec_flow_stmts(st: SFlowState, ss: Vec<Stmt>, i: int) -> Option<SFlowState>
    decreases ss, 1int, ss@.len() - i,
{
    if 0 <= i < ss@.len() {
        match spec_flow_stmt(st, ss@[i]) {
            Some(next) => spec_flow_stmts(next, ss, i + 1),
            None => None,
        }
    } else {
        Some(st)
    }
}

pub open spec fn spec_flow_stmt(st: SFlowState, s: Stmt) -> Option<SFlowState>
    decreases s, 0int, 0int,
{
    match s {
        Stmt::Assign { target, value } => {
            if !spec_expr_flows(st, value) {
                None
            } else if st.reach && target.0 < st.assigned.len() {
                Some(SFlowState { assigned: st.assigned.update(target.0 as int, true), ..st })
            } else {
                Some(st)
            }
        },
        Stmt::Return(e) => if spec_expr_flows(st, e) { Some(SFlowState { reach: false, ..st }) } else { None },
        Stmt::Break => if st.reach && !st.in_loop { None } else { Some(SFlowState { reach: false, ..st }) },
        Stmt::If { cond, then_branch, else_branch } => {
            if !spec_expr_flows(st, cond) {
                None
            } else {
                match (spec_flow_stmts(st, then_branch, 0), spec_flow_stmts(st, else_branch, 0)) {
                    (Some(t), Some(e)) => Some(spec_flow_merge(st, t, e)),
                    _ => None,
                }
            }
        },
        // Loops don't count: the state after one is the state before it (CHECK.md §5.2).
        Stmt::While { cond, body } => {
            if spec_expr_flows(st, cond) && spec_flow_stmts(SFlowState { in_loop: true, ..st }, body, 0) is Some {
                Some(st)
            } else {
                None
            }
        },
        Stmt::Repeat { body, .. } => {
            if spec_flow_stmts(SFlowState { in_loop: true, ..st }, body, 0) is Some {
                Some(st)
            } else {
                None
            }
        },
    }
}

/// At the start of a body the parameters are assigned, and nothing else.
pub open spec fn spec_flow_start(f: Function) -> SFlowState {
    SFlowState {
        reach: true,
        assigned: Seq::new(f.locals@.len(), |k: int| k < f.params),
        in_loop: false,
    }
}

/// Definite assignment, every path returns, `break` only in a loop.
pub open spec fn spec_flow_ok(f: Function) -> bool {
    match spec_flow_stmts(spec_flow_start(f), f.body, 0) {
        Some(end) => !end.reach,
        None => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Ranks: what the acyclicity check gives the interpreter
// ---------------------------------------------------------------------------------------------

pub open spec fn spec_below(t: Seq<Option<usize>>, id: usize, r: usize) -> bool {
    match spec_rank(t, id) {
        Some(q) => q < r,
        None => false,
    }
}

/// Every call, const read and density read in `e` goes to something ranked below `r`.
pub open spec fn spec_expr_ranked(ranks: SRanks, r: usize, e: Expr) -> bool
    decreases e,
{
    match e {
        Expr::Lit(_) | Expr::Local(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => true,
        Expr::Const(c) => spec_below(ranks.consts, c.0, r),
        Expr::Density { input, index } => spec_below(ranks.inputs, input.0, r) && match index {
            Some(ix) => spec_expr_ranked(ranks, r, *ix),
            None => true,
        },
        Expr::Call { func, args } => spec_below(ranks.fns, func.0, r)
            && (forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_expr_ranked(ranks, r, args@[k])),
        Expr::ExternCall { args, .. } => forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() ==> spec_expr_ranked(ranks, r, args@[k]),
        Expr::Binary { left, right, .. } => spec_expr_ranked(ranks, r, *left) && spec_expr_ranked(ranks, r, *right),
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => spec_expr_ranked(ranks, r, *operand),
        Expr::Field { base, .. } => spec_expr_ranked(ranks, r, *base),
        Expr::Index { array, index } => spec_expr_ranked(ranks, r, *array) && spec_expr_ranked(ranks, r, *index),
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => spec_expr_ranked(ranks, r, *x) && spec_expr_ranked(ranks, r, *y) && spec_expr_ranked(ranks, r, *z),
        Expr::MakeRecord { fields, .. } => forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() ==> spec_expr_ranked(ranks, r, fields@[k].value),
        Expr::ArrayLit(es) => forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() ==> spec_expr_ranked(ranks, r, es@[k]),
    }
}

pub open spec fn spec_stmt_ranked(ranks: SRanks, r: usize, s: Stmt) -> bool
    decreases s,
{
    match s {
        Stmt::Assign { value, .. } => spec_expr_ranked(ranks, r, value),
        Stmt::Return(e) => spec_expr_ranked(ranks, r, e),
        Stmt::If { cond, then_branch, else_branch } => spec_expr_ranked(ranks, r, cond)
            && (forall|k: int| #![trigger then_branch@[k]] 0 <= k < then_branch@.len() ==> spec_stmt_ranked(ranks, r, then_branch@[k]))
            && (forall|k: int| #![trigger else_branch@[k]] 0 <= k < else_branch@.len() ==> spec_stmt_ranked(ranks, r, else_branch@[k])),
        Stmt::While { cond, body } => spec_expr_ranked(ranks, r, cond)
            && (forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() ==> spec_stmt_ranked(ranks, r, body@[k])),
        Stmt::Repeat { body, .. } => forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() ==> spec_stmt_ranked(ranks, r, body@[k]),
        Stmt::Break => true,
    }
}

/// Every id has a rank, and every edge goes strictly down (CHECK.md §7).
pub open spec fn spec_ranks_valid(p: Program, ranks: SRanks) -> bool {
    &&& ranks.fns.len() == p.functions@.len()
    &&& ranks.consts.len() == p.consts@.len()
    &&& ranks.inputs.len() == p.inputs@.len()
    &&& ranks.units.len() == p.units@.len()
    &&& forall|k: int| #![trigger ranks.fns[k]] 0 <= k < ranks.fns.len() ==> ranks.fns[k] is Some && forall|j: int| #![trigger p.functions@[k].body@[j]]
        0 <= j < p.functions@[k].body@.len() ==> spec_stmt_ranked(ranks, ranks.fns[k]->0, p.functions@[k].body@[j])
    &&& forall|k: int| #![trigger ranks.consts[k]] 0 <= k < ranks.consts.len() ==> ranks.consts[k] is Some
        && ranks.consts[k]->0 < usize::MAX
        && spec_expr_ranked(ranks, ranks.consts[k]->0, p.consts@[k].init)
    &&& forall|k: int| #![trigger ranks.inputs[k]] 0 <= k < ranks.inputs.len() ==> ranks.inputs[k] is Some
        && spec_below(ranks.units, p.inputs@[k].source.0, ranks.inputs[k]->0)
    &&& forall|k: int| #![trigger ranks.units[k]] 0 <= k < ranks.units.len() ==> ranks.units[k] is Some && {
        &&& spec_below(ranks.fns, p.units@[k].main.0, ranks.units[k]->0)
        &&& forall|j: int| #![trigger p.units@[k].inputs@[j]] 0 <= j < p.units@[k].inputs@.len() ==> spec_below(ranks.inputs, p.units@[k].inputs@[j].0, ranks.units[k]->0)
    }
}

/// Every grid is non-empty and its cell count fits an `i32` (CHECK.md §8).
pub open spec fn spec_dims_ok(d: Dims) -> bool {
    d.x >= 1 && d.y >= 1 && d.z >= 1 && d.x * d.y * d.z <= 0x7fff_ffff
}

/// A program the checker accepts, given each constant's derived type.
pub open spec fn spec_well_typed_with(p: Program, ctys: Seq<Option<Type>>) -> bool {
    &&& wf_program(&p)
    &&& spec_ranks_valid(p, spec_ranks(p))
    &&& spec_consts_typed(p, ctys)
    &&& forall|k: int| 0 <= k < p.functions@.len() ==> spec_function_typed(p, ctys, #[trigger] p.functions@[k]) && spec_flow_ok(p.functions@[k])
    &&& forall|u: int| 0 <= u < p.units@.len() ==> p.functions@[(#[trigger] p.units@[u]).main.0 as int].params == 0
    &&& forall|i: int| 0 <= i < p.inputs@.len() ==> spec_dims_ok((#[trigger] p.inputs@[i]).dims)
    &&& forall|e: int| 0 <= e < p.entries@.len() ==> spec_dims_ok((#[trigger] p.entries@[e]).dims)
}

/// Every const has its derived type, and its init has that type.
pub open spec fn spec_consts_typed(p: Program, ctys: Seq<Option<Type>>) -> bool {
    &&& ctys.len() == p.consts@.len()
    &&& forall|c: int| 0 <= c < ctys.len() ==> #[trigger] ctys[c] is Some && spec_type_of(p, Seq::empty(), ctys, p.consts@[c].init) == ctys[c]
}

pub open spec fn spec_well_typed(p: Program) -> bool {
    exists|ctys: Seq<Option<Type>>| spec_well_typed_with(p, ctys)
}

/// A case the program accepts: one value of the declared type per host (CHECK.md §9).
pub open spec fn spec_case_fits(p: Program, hosts: Seq<SValue>) -> bool {
    &&& hosts.len() == p.hosts@.len()
    &&& forall|h: int| 0 <= h < hosts.len() ==> spec_value_has_type(p, #[trigger] hosts[h], p.hosts@[h].ty)
}

/// The node table fits `usize` with room to spare. Physically always true, since every element
/// takes memory; Verus only knows each table's length fits on its own.
pub open spec fn spec_program_fits(p: Program) -> bool {
    p.functions@.len() + p.consts@.len() + p.inputs@.len() + p.units@.len() < usize::MAX
}

} // verus!
