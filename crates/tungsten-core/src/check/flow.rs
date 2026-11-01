//! Flow (§6.4): definite assignment, every path returns, `Break` placement.
//!
//! A forward pass per function with a state `Reach | Unreachable` and an assigned set over
//! locals. Loops are conservative: the state after a loop is the state before it, whatever the
//! body does or the count is. Code after `return` / `break` is unreachable: it is still
//! type-checked elsewhere, but flow reports nothing for it and its assignments don't count.

use vstd::prelude::*;

use crate::check::error::*;
use crate::ir::*;
use crate::util::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub struct FState {
    pub reach: bool,
    pub assigned: Vec<bool>,
    /// Locals some loop body earlier on the path assigns (for the hint).
    pub loop_assigned: Vec<bool>,
    pub depth: usize,
}

/// Per function: what was already reported, so each local is reported once.
pub struct FCx {
    pub item: CoreItem,
    pub reported: Vec<bool>,
}

fn copy_state(s: &FState) -> FState {
    FState {
        reach: s.reach,
        assigned: copy_bools(&s.assigned),
        loop_assigned: copy_bools(&s.loop_assigned),
        depth: s.depth,
    }
}

fn union_into(dst: &mut Vec<bool>, src: &Vec<bool>) {
    let mut i: usize = 0;
    while i < src.len()
        invariant
            i <= src.len(),
        decreases src.len() - i,
    {
        if src[i] {
            set(dst, i, true);
        }
        i = i + 1;
    }
}

fn intersect(a: &Vec<bool>, b: &Vec<bool>) -> Vec<bool> {
    let mut out: Vec<bool> = Vec::new();
    let mut i: usize = 0;
    while i < a.len()
        invariant
            i <= a.len(),
        decreases a.len() - i,
    {
        out.push(a[i] && get_bool(b, i));
        i = i + 1;
    }
    out
}

#[verifier::loop_isolation(false)]
fn flow_expr(cx: &mut FCx, st: &FState, e: &Expr, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases e,
{
    match e {
        Expr::Local(l) => {
            if st.reach && !get_bool(&st.assigned, l.0) && !get_bool(&cx.reported, l.0) {
                let hint = if get_bool(&st.loop_assigned, l.0) {
                    Some(UnassignedHint::AssignedOnlyInLoop)
                } else {
                    None
                };
                report(errs, CheckErrorKind::UnassignedLocal { local: *l, hint }, cx.item, path);
                set(&mut cx.reported, l.0, true);
            }
        },
        Expr::Lit(_) | Expr::Const(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => {},
        Expr::Density { index, .. } => match index {
            Some(ix) => {
                path.push(CoreStep::Index);
                flow_expr(cx, st, ix, path, errs);
                let _ = path.pop();
            },
            None => {},
        },
        Expr::Call { args, .. } | Expr::ExternCall { args, .. } => {
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                path.push(CoreStep::Arg(i));
                flow_expr(cx, st, &args[i], path, errs);
                let _ = path.pop();
                i = i + 1;
            }
        },
        Expr::Binary { left, right, .. } => {
            path.push(CoreStep::Left);
            flow_expr(cx, st, left, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Right);
            flow_expr(cx, st, right, path, errs);
            let _ = path.pop();
        },
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => {
            path.push(CoreStep::Operand);
            flow_expr(cx, st, operand, path, errs);
            let _ = path.pop();
        },
        Expr::Field { base, .. } => {
            path.push(CoreStep::Base);
            flow_expr(cx, st, base, path, errs);
            let _ = path.pop();
        },
        Expr::Index { array, index } => {
            path.push(CoreStep::Base);
            flow_expr(cx, st, array, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Index);
            flow_expr(cx, st, index, path, errs);
            let _ = path.pop();
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            path.push(CoreStep::Arg(0));
            flow_expr(cx, st, x, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(1));
            flow_expr(cx, st, y, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(2));
            flow_expr(cx, st, z, path, errs);
            let _ = path.pop();
        },
        Expr::MakeRecord { record, fields } => {
            let mut i: usize = 0;
            while i < fields.len()
                invariant
                    i <= fields.len(),
                decreases fields.len() - i,
            {
                path.push(CoreStep::Field(*record, fields[i].field));
                flow_expr(cx, st, &fields[i].value, path, errs);
                let _ = path.pop();
                i = i + 1;
            }
        },
        Expr::ArrayLit(es) => {
            let mut i: usize = 0;
            while i < es.len()
                invariant
                    i <= es.len(),
                decreases es.len() - i,
            {
                path.push(CoreStep::Element(i));
                flow_expr(cx, st, &es[i], path, errs);
                let _ = path.pop();
                i = i + 1;
            }
        },
    }
}

/// Every `Assign` target anywhere in `ss` (syntactically).
#[verifier::loop_isolation(false)]
pub fn assigns_stmts(ss: &Vec<Stmt>, out: &mut Vec<bool>)
    decreases ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        assigns_stmt(&ss[i], out);
        i = i + 1;
    }
}

pub fn assigns_stmt(s: &Stmt, out: &mut Vec<bool>)
    decreases s,
{
    match s {
        Stmt::Assign { target, .. } => {
            set(out, target.0, true);
        },
        Stmt::If { then_branch, else_branch, .. } => {
            assigns_stmts(then_branch, out);
            assigns_stmts(else_branch, out);
        },
        Stmt::While { body, .. } | Stmt::Repeat { body, .. } => assigns_stmts(body, out),
        Stmt::Return(_) | Stmt::Break => {},
    }
}

/// `true` if `ss` contains a `Return` anywhere.
#[verifier::loop_isolation(false)]
pub fn has_return_stmts(ss: &Vec<Stmt>) -> bool
    decreases ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        if has_return_stmt(&ss[i]) {
            return true;
        }
        i = i + 1;
    }
    false
}

pub fn has_return_stmt(s: &Stmt) -> bool
    decreases s,
{
    match s {
        Stmt::Return(_) => true,
        Stmt::If { then_branch, else_branch, .. } => has_return_stmts(then_branch) || has_return_stmts(else_branch),
        Stmt::While { body, .. } | Stmt::Repeat { body, .. } => has_return_stmts(body),
        Stmt::Assign { .. } | Stmt::Break => false,
    }
}

fn flow_loop_body(cx: &mut FCx, st: &mut FState, body: &Vec<Stmt>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases body, 1int,
{
    let mut inner = copy_state(st);
    inner.depth = add_or_max(inner.depth, 1);
    flow_stmts(cx, &mut inner, body, StepKind::Body, path, errs);
    if st.reach {
        let mut assigned = bools(st.assigned.len(), false);
        assigns_stmts(body, &mut assigned);
        union_into(&mut st.loop_assigned, &assigned);
    }
}

#[verifier::loop_isolation(false)]
pub fn flow_stmts(cx: &mut FCx, st: &mut FState, ss: &Vec<Stmt>, kind: StepKind, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases ss, 0int,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        path.push(mk_step(kind, i));
        flow_stmt(cx, st, &ss[i], path, errs);
        let _ = path.pop();
        i = i + 1;
    }
}

pub fn flow_stmt(cx: &mut FCx, st: &mut FState, s: &Stmt, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases s, 0int,
{
    match s {
        Stmt::Assign { target, value } => {
            path.push(CoreStep::Value);
            flow_expr(cx, st, value, path, errs);
            let _ = path.pop();
            if st.reach {
                set(&mut st.assigned, target.0, true);
            }
        },
        Stmt::Return(e) => {
            path.push(CoreStep::Value);
            flow_expr(cx, st, e, path, errs);
            let _ = path.pop();
            st.reach = false;
        },
        Stmt::Break => {
            if st.reach && st.depth == 0 {
                report(errs, CheckErrorKind::BreakOutsideLoop, cx.item, path);
            }
            st.reach = false;
        },
        Stmt::If { cond, then_branch, else_branch } => {
            path.push(CoreStep::Cond);
            flow_expr(cx, st, cond, path, errs);
            let _ = path.pop();
            let mut t = copy_state(st);
            flow_stmts(cx, &mut t, then_branch, StepKind::Then, path, errs);
            let mut e = copy_state(st);
            flow_stmts(cx, &mut e, else_branch, StepKind::Else, path, errs);
            if st.reach {
                union_into(&mut st.loop_assigned, &t.loop_assigned);
                union_into(&mut st.loop_assigned, &e.loop_assigned);
                if !t.reach && !e.reach {
                    st.reach = false;
                } else if !t.reach {
                    st.assigned = e.assigned;
                } else if !e.reach {
                    st.assigned = t.assigned;
                } else {
                    st.assigned = intersect(&t.assigned, &e.assigned);
                }
            }
        },
        Stmt::While { cond, body } => {
            path.push(CoreStep::Cond);
            flow_expr(cx, st, cond, path, errs);
            let _ = path.pop();
            flow_loop_body(cx, st, body, path, errs);
        },
        Stmt::Repeat { body, .. } => {
            flow_loop_body(cx, st, body, path, errs);
        },
    }
}

fn ends_in_loop_with_return(body: &Vec<Stmt>) -> bool {
    if body.len() == 0 {
        return false;
    }
    match &body[body.len() - 1] {
        Stmt::While { body: b, .. } | Stmt::Repeat { body: b, .. } => has_return_stmts(b),
        _ => false,
    }
}

pub fn check_flow_fn(f: &Function, item: CoreItem, errs: &mut Vec<CheckError>) {
    let n = f.locals.len();
    let mut st = FState { reach: true, assigned: bools(n, false), loop_assigned: bools(n, false), depth: 0 };
    let mut k: usize = 0;
    while k < f.params && k < n
        invariant
            k <= n,
        decreases n - k,
    {
        set(&mut st.assigned, k, true);
        k = k + 1;
    }
    let mut cx = FCx { item, reported: bools(n, false) };
    let mut path: Vec<CoreStep> = Vec::new();
    flow_stmts(&mut cx, &mut st, &f.body, StepKind::Stmt, &mut path, errs);
    if st.reach {
        let hint = if ends_in_loop_with_return(&f.body) {
            Some(ReturnHint::EndsInLoop)
        } else {
            None
        };
        report(errs, CheckErrorKind::MissingReturn { hint }, item, &path);
    }
}

pub fn check_flow(p: &Program, errs: &mut Vec<CheckError>) {
    let mut k: usize = 0;
    while k < p.functions.len()
        invariant
            k <= p.functions.len(),
        decreases p.functions.len() - k,
    {
        check_flow_fn(&p.functions[k], CoreItem::Function(FnId(k)), errs);
        k = k + 1;
    }
}

} // verus!
