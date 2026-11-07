//! Scoping (§6.5). For every unit `U`, `F(U) = {U.main} ∪ U.helpers`, and for every `f ∈ F(U)`:
//! (a) calls stay in `F(U)`, (b) hosts are in `f.hosts ⊆ U.hosts`, (c) tables in `U.tables`,
//! (d) consts (and everything their inits reach) in `U.consts`, (e) density reads only of
//! inputs in `U.inputs`.
//!
//! One pre-order walk per function covers every unit that lists it. Each violation is reported
//! once per `(unit, offending id)`, at its first occurrence in walk order.

use vstd::prelude::*;

use crate::check::acyclic::*;
use crate::check::error::*;
use crate::ir::*;
use crate::util::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

#[derive(Clone, Copy)]
pub enum SeenKind {
    Helper,
    HostFn,
    HostUnit,
    Table,
    Const,
    Input,
}

#[derive(Clone, Copy)]
pub struct Seen {
    pub kind: SeenKind,
    pub unit: usize,
    pub id: usize,
}

fn same_kind(a: SeenKind, b: SeenKind) -> bool {
    match (a, b) {
        (SeenKind::Helper, SeenKind::Helper) => true,
        (SeenKind::HostFn, SeenKind::HostFn) => true,
        (SeenKind::HostUnit, SeenKind::HostUnit) => true,
        (SeenKind::Table, SeenKind::Table) => true,
        (SeenKind::Const, SeenKind::Const) => true,
        (SeenKind::Input, SeenKind::Input) => true,
        _ => false,
    }
}

/// Records `(kind, unit, id)`; `true` if it is new.
fn first(seen: &mut Vec<Seen>, kind: SeenKind, unit: usize, id: usize) -> bool {
    let mut i: usize = 0;
    while i < seen.len()
        invariant
            i <= seen.len(),
        decreases seen.len() - i,
    {
        let s = seen[i];
        if same_kind(s.kind, kind) && s.unit == unit && s.id == id {
            return false;
        }
        i = i + 1;
    }
    seen.push(Seen { kind, unit, id });
    true
}

pub fn in_unit(u: &Unit, f: FnId) -> bool {
    u.main.0 == f.0 || contains_fn(&u.helpers, f)
}

/// The units whose `F(U)` contains `f`, in id order.
pub fn units_of(p: &Program, f: FnId) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut u: usize = 0;
    while u < p.units.len()
        invariant
            u <= p.units.len(),
        decreases p.units.len() - u,
    {
        if in_unit(&p.units[u], f) {
            out.push(u);
        }
        u = u + 1;
    }
    out
}

pub struct SCx<'a> {
    pub p: &'a Program,
    pub func: FnId,
    pub f: &'a Function,
    pub units: Vec<usize>,
}

/// The checks at one node, for every unit in `cx.units`.
fn scope_node(cx: &SCx, e: &Expr, seen: &mut Vec<Seen>, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>) {
    let item = CoreItem::Function(cx.func);
    match e {
        Expr::Host(h) => {
            if !contains_host(&cx.f.hosts, *h) && first(seen, SeenKind::HostFn, cx.func.0, h.0) {
                report(errs, CheckErrorKind::HostNotInFn { host: *h }, item, path);
            }
        },
        _ => {},
    }
    let mut k: usize = 0;
    while k < cx.units.len()
        invariant
            k <= cx.units.len(),
        decreases cx.units.len() - k,
    {
        let uid = cx.units[k];
        match get(&cx.p.units, uid) {
            Some(u) => match e {
                Expr::Call { func, .. } => {
                    if !in_unit(u, *func) && first(seen, SeenKind::Helper, uid, func.0) {
                        report(errs, CheckErrorKind::HelperNotListed { unit: UnitId(uid), func: *func }, item, path);
                    }
                },
                Expr::Table(t) => {
                    if !contains_table(&u.tables, *t) && first(seen, SeenKind::Table, uid, t.0) {
                        report(errs, CheckErrorKind::TableNotListed { unit: UnitId(uid), table: *t }, item, path);
                    }
                },
                Expr::Const(c) => {
                    if !contains_const(&u.consts, *c) && first(seen, SeenKind::Const, uid, c.0) {
                        report(errs, CheckErrorKind::ConstNotListed { unit: UnitId(uid), konst: *c }, item, path);
                    }
                },
                Expr::Density { input, .. } => {
                    if !contains_input(&u.inputs, *input) && first(seen, SeenKind::Input, uid, input.0) {
                        report(errs, CheckErrorKind::InputNotListed { unit: UnitId(uid), input: *input }, item, path);
                    }
                },
                _ => {},
            },
            None => {},
        }
        k = k + 1;
    }
}

#[verifier::loop_isolation(false)]
fn scope_expr(cx: &SCx, e: &Expr, seen: &mut Vec<Seen>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases e,
{
    scope_node(cx, e, seen, path, errs);
    match e {
        Expr::Lit(_) | Expr::Local(_) | Expr::Const(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => {},
        Expr::Density { index, .. } => match index {
            Some(ix) => {
                path.push(CoreStep::Index);
                scope_expr(cx, ix, seen, path, errs);
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
                scope_expr(cx, &args[i], seen, path, errs);
                let _ = path.pop();
                i = i + 1;
            }
        },
        Expr::Binary { left, right, .. } => {
            path.push(CoreStep::Left);
            scope_expr(cx, left, seen, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Right);
            scope_expr(cx, right, seen, path, errs);
            let _ = path.pop();
        },
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => {
            path.push(CoreStep::Operand);
            scope_expr(cx, operand, seen, path, errs);
            let _ = path.pop();
        },
        Expr::Field { base, .. } => {
            path.push(CoreStep::Base);
            scope_expr(cx, base, seen, path, errs);
            let _ = path.pop();
        },
        Expr::Index { array, index } => {
            path.push(CoreStep::Base);
            scope_expr(cx, array, seen, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Index);
            scope_expr(cx, index, seen, path, errs);
            let _ = path.pop();
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            path.push(CoreStep::Arg(0));
            scope_expr(cx, x, seen, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(1));
            scope_expr(cx, y, seen, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(2));
            scope_expr(cx, z, seen, path, errs);
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
                scope_expr(cx, &fields[i].value, seen, path, errs);
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
                scope_expr(cx, &es[i], seen, path, errs);
                let _ = path.pop();
                i = i + 1;
            }
        },
    }
}

#[verifier::loop_isolation(false)]
fn scope_stmts(cx: &SCx, ss: &Vec<Stmt>, kind: StepKind, seen: &mut Vec<Seen>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        path.push(mk_step(kind, i));
        scope_stmt(cx, &ss[i], seen, path, errs);
        let _ = path.pop();
        i = i + 1;
    }
}

fn scope_stmt(cx: &SCx, s: &Stmt, seen: &mut Vec<Seen>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    decreases s,
{
    match s {
        Stmt::Assign { value, .. } => {
            path.push(CoreStep::Value);
            scope_expr(cx, value, seen, path, errs);
            let _ = path.pop();
        },
        Stmt::Return(e) => {
            path.push(CoreStep::Value);
            scope_expr(cx, e, seen, path, errs);
            let _ = path.pop();
        },
        Stmt::If { cond, then_branch, else_branch } => {
            path.push(CoreStep::Cond);
            scope_expr(cx, cond, seen, path, errs);
            let _ = path.pop();
            scope_stmts(cx, then_branch, StepKind::Then, seen, path, errs);
            scope_stmts(cx, else_branch, StepKind::Else, seen, path, errs);
        },
        Stmt::While { cond, body } => {
            path.push(CoreStep::Cond);
            scope_expr(cx, cond, seen, path, errs);
            let _ = path.pop();
            scope_stmts(cx, body, StepKind::Body, seen, path, errs);
        },
        Stmt::Repeat { body, .. } => scope_stmts(cx, body, StepKind::Body, seen, path, errs),
        Stmt::Break => {},
    }
}

/// Per-function rules, then the transitive const rule per unit.
pub fn check_scope(p: &Program, errs: &mut Vec<CheckError>) {
    let mut seen: Vec<Seen> = Vec::new();
    let mut k: usize = 0;
    while k < p.functions.len()
        invariant
            k <= p.functions.len(),
        decreases p.functions.len() - k,
    {
        let f = &p.functions[k];
        let cx = SCx { p, func: FnId(k), f, units: units_of(p, FnId(k)) };
        let mut path: Vec<CoreStep> = Vec::new();
        // (b), unit half: f.hosts ⊆ U.hosts.
        let mut j: usize = 0;
        while j < f.hosts.len()
            invariant
                j <= f.hosts.len(),
            decreases f.hosts.len() - j,
        {
            let h = f.hosts[j];
            let mut q: usize = 0;
            while q < cx.units.len()
                invariant
                    q <= cx.units.len(),
                decreases cx.units.len() - q,
            {
                let uid = cx.units[q];
                match get(&p.units, uid) {
                    Some(u) => {
                        if !contains_host(&u.hosts, h) && first(&mut seen, SeenKind::HostUnit, uid, h.0) {
                            path.push(CoreStep::List(ScopeList::Hosts, j));
                            report(errs, CheckErrorKind::HostNotInUnit { unit: UnitId(uid), host: h }, CoreItem::Function(FnId(k)), &path);
                            let _ = path.pop();
                        }
                    },
                    None => {},
                }
                q = q + 1;
            }
            j = j + 1;
        }
        scope_stmts(&cx, &f.body, StepKind::Stmt, &mut seen, &mut path, errs);
        k = k + 1;
    }
    // (d), transitive: every const reachable from a directly read one must be listed too.
    let nc = p.consts.len();
    let mut u: usize = 0;
    while u < p.units.len()
        invariant
            u <= p.units.len(),
            nc == p.consts.len(),
        decreases p.units.len() - u,
    {
        let unit = &p.units[u];
        let mut direct = bools(nc, false);
        let mut f: usize = 0;
        while f < p.functions.len()
            invariant
                f <= p.functions.len(),
            decreases p.functions.len() - f,
        {
            if in_unit(unit, FnId(f)) {
                let mut r = empty_refs();
                refs_stmts(&p.functions[f].body, &mut r);
                let mut i: usize = 0;
                while i < r.consts.len()
                    invariant
                        i <= r.consts.len(),
                    decreases r.consts.len() - i,
                {
                    set(&mut direct, r.consts[i], true);
                    i = i + 1;
                }
            }
            f = f + 1;
        }
        let mut need = copy_bools(&direct);
        let rounds = add_or_max(nc, 1);
        let mut round: usize = 0;
        while round < rounds
            invariant
                round <= rounds,
                nc == p.consts.len(),
            decreases rounds - round,
        {
            let mut changed = false;
            let mut c: usize = 0;
            while c < nc
                invariant
                    c <= nc,
                    nc == p.consts.len(),
                decreases nc - c,
            {
                if get_bool(&need, c) {
                    let mut r = empty_refs();
                    refs_expr(&p.consts[c].init, &mut r);
                    let mut i: usize = 0;
                    while i < r.consts.len()
                        invariant
                            i <= r.consts.len(),
                        decreases r.consts.len() - i,
                    {
                        let d = r.consts[i];
                        if d < nc && !get_bool(&need, d) {
                            set(&mut need, d, true);
                            changed = true;
                        }
                        i = i + 1;
                    }
                }
                c = c + 1;
            }
            if !changed {
                break;
            }
            round = round + 1;
        }
        let mut c: usize = 0;
        while c < nc
            invariant
                c <= nc,
            decreases nc - c,
        {
            if get_bool(&need, c) && !get_bool(&direct, c) && !contains_const(&unit.consts, ConstId(c))
                && first(&mut seen, SeenKind::Const, u, c) {
                let mut path: Vec<CoreStep> = Vec::new();
                path.push(CoreStep::WholeList(ScopeList::Consts));
                report(errs, CheckErrorKind::ConstNotListed { unit: UnitId(u), konst: ConstId(c) }, CoreItem::Unit(UnitId(u)), &path);
            }
            c = c + 1;
        }
        u = u + 1;
    }
}

} // verus!
