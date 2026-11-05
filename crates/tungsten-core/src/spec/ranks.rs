//! The dependency graph and ranks of a program (CHECK.md §7): the same steps as
//! `crate::check::acyclic`, with the bounded fixpoint as recursion. Out-of-range ids are not edges.

use vstd::prelude::*;

use crate::ir::*;
use crate::spec::eval::*;
use crate::spec::value::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// [`crate::check::acyclic::Refs`].
pub struct SRefs {
    pub fns: Seq<usize>,
    pub consts: Seq<usize>,
    pub inputs: Seq<usize>,
}

pub open spec fn spec_empty_refs() -> SRefs {
    SRefs { fns: Seq::empty(), consts: Seq::empty(), inputs: Seq::empty() }
}

pub open spec fn spec_refs_exprs(es: Vec<Expr>, i: int, r: SRefs) -> SRefs
    decreases es, es@.len() - i,
{
    if 0 <= i < es@.len() {
        spec_refs_exprs(es, i + 1, spec_refs_expr(es@[i], r))
    } else {
        r
    }
}

pub open spec fn spec_refs_fields(fs: Vec<FieldInit>, i: int, r: SRefs) -> SRefs
    decreases fs, fs@.len() - i,
{
    if 0 <= i < fs@.len() {
        spec_refs_fields(fs, i + 1, spec_refs_expr(fs@[i].value, r))
    } else {
        r
    }
}

/// Every call, const read and density read in `e`, added to `r` in walk order.
pub open spec fn spec_refs_expr(e: Expr, r: SRefs) -> SRefs
    decreases e, 0int,
{
    match e {
        Expr::Lit(_) | Expr::Local(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => r,
        Expr::Const(c) => SRefs { consts: r.consts.push(c.0), ..r },
        Expr::Density { input, index } => {
            let r1 = SRefs { inputs: r.inputs.push(input.0), ..r };
            match index {
                Some(ix) => spec_refs_expr(*ix, r1),
                None => r1,
            }
        },
        Expr::Call { func, args } => spec_refs_exprs(args, 0, SRefs { fns: r.fns.push(func.0), ..r }),
        Expr::ExternCall { args, .. } => spec_refs_exprs(args, 0, r),
        Expr::Binary { left, right, .. } => spec_refs_expr(*right, spec_refs_expr(*left, r)),
        Expr::Unary { operand, .. } => spec_refs_expr(*operand, r),
        Expr::Field { base, .. } => spec_refs_expr(*base, r),
        Expr::Index { array, index } => spec_refs_expr(*index, spec_refs_expr(*array, r)),
        Expr::MakeVec3 { x, y, z } => spec_refs_expr(*z, spec_refs_expr(*y, spec_refs_expr(*x, r))),
        Expr::MakePos3 { x, y, z } => spec_refs_expr(*z, spec_refs_expr(*y, spec_refs_expr(*x, r))),
        Expr::MakeRecord { fields, .. } => spec_refs_fields(fields, 0, r),
        Expr::ArrayLit(es) => spec_refs_exprs(es, 0, r),
        Expr::Cast { operand, .. } => spec_refs_expr(*operand, r),
    }
}

pub open spec fn spec_refs_stmts(ss: Vec<Stmt>, i: int, r: SRefs) -> SRefs
    decreases ss, ss@.len() - i,
{
    if 0 <= i < ss@.len() {
        spec_refs_stmts(ss, i + 1, spec_refs_stmt(ss@[i], r))
    } else {
        r
    }
}

pub open spec fn spec_refs_stmt(s: Stmt, r: SRefs) -> SRefs
    decreases s, 0int,
{
    match s {
        Stmt::Assign { value, .. } => spec_refs_expr(value, r),
        Stmt::Return(e) => spec_refs_expr(e, r),
        Stmt::If { cond, then_branch, else_branch } =>
            spec_refs_stmts(else_branch, 0, spec_refs_stmts(then_branch, 0, spec_refs_expr(cond, r))),
        Stmt::While { cond, body } => spec_refs_stmts(body, 0, spec_refs_expr(cond, r)),
        Stmt::Repeat { body, .. } => spec_refs_stmts(body, 0, r),
        Stmt::Break => r,
    }
}

pub open spec fn spec_add_or_max(a: usize, b: usize) -> usize {
    if a + b > usize::MAX { usize::MAX } else { (a + b) as usize }
}

pub open spec fn spec_push_if_lt(out: Seq<usize>, base: usize, id: usize, n: usize) -> Seq<usize> {
    if id < n { out.push(spec_add_or_max(base, id)) } else { out }
}

pub open spec fn spec_push_all(out: Seq<usize>, base: usize, ids: Seq<usize>, n: usize, i: int) -> Seq<usize>
    decreases ids.len() - i,
{
    if 0 <= i < ids.len() {
        spec_push_all(spec_push_if_lt(out, base, ids[i], n), base, ids, n, i + 1)
    } else {
        out
    }
}

pub open spec fn spec_push_inputs(out: Seq<usize>, base: usize, ids: Seq<InputId>, n: usize, i: int) -> Seq<usize>
    decreases ids.len() - i,
{
    if 0 <= i < ids.len() {
        spec_push_inputs(spec_push_if_lt(out, base, ids[i].0, n), base, ids, n, i + 1)
    } else {
        out
    }
}

pub open spec fn spec_push_consts(out: Seq<usize>, base: usize, n: usize, c: int) -> Seq<usize>
    decreases n - c,
{
    if 0 <= c < n {
        spec_push_consts(spec_push_if_lt(out, base, c as usize, n), base, n, c + 1)
    } else {
        out
    }
}

/// Where each kind of node starts: `fns ++ consts ++ inputs ++ units`.
pub open spec fn spec_nf(p: Program) -> usize { p.functions@.len() as usize }
pub open spec fn spec_nc(p: Program) -> usize { p.consts@.len() as usize }
pub open spec fn spec_ni(p: Program) -> usize { p.inputs@.len() as usize }
pub open spec fn spec_nu(p: Program) -> usize { p.units@.len() as usize }
pub open spec fn spec_bi(p: Program) -> usize { spec_add_or_max(spec_nf(p), spec_nc(p)) }
pub open spec fn spec_bu(p: Program) -> usize { spec_add_or_max(spec_bi(p), spec_ni(p)) }

/// The edges out of a body's references: functions, then consts, then inputs.
pub open spec fn spec_ref_edges(p: Program, r: SRefs) -> Seq<usize> {
    let out = spec_push_all(Seq::empty(), 0, r.fns, spec_nf(p), 0);
    let out = spec_push_all(out, spec_nf(p), r.consts, spec_nc(p), 0);
    spec_push_all(out, spec_bi(p), r.inputs, spec_ni(p), 0)
}

pub open spec fn spec_unit_edges(p: Program, u: Unit) -> Seq<usize> {
    let out = spec_push_if_lt(Seq::empty(), 0, u.main.0, spec_nf(p));
    let out = spec_push_inputs(out, spec_bi(p), u.inputs@, spec_ni(p), 0);
    spec_push_consts(out, spec_nf(p), spec_nc(p), 0)
}

/// Node `j`'s successors.
pub open spec fn spec_succ(p: Program, j: int) -> Seq<usize> {
    let (nf, nc, ni) = (p.functions@.len() as int, p.consts@.len() as int, p.inputs@.len() as int);
    if j < nf {
        spec_ref_edges(p, spec_refs_stmts(p.functions@[j].body, 0, spec_empty_refs()))
    } else if j < nf + nc {
        spec_ref_edges(p, spec_refs_expr(p.consts@[j - nf].init, spec_empty_refs()))
    } else if j < nf + nc + ni {
        spec_push_if_lt(Seq::empty(), spec_bu(p), p.inputs@[j - nf - nc].source.0, spec_nu(p))
    } else {
        spec_unit_edges(p, p.units@[j - nf - nc - ni])
    }
}

pub open spec fn spec_node_count(p: Program) -> nat {
    (p.functions@.len() + p.consts@.len() + p.inputs@.len() + p.units@.len()) as nat
}

pub open spec fn spec_graph(p: Program) -> Seq<Seq<usize>> {
    Seq::new(spec_node_count(p), |j: int| spec_succ(p, j))
}

/// One node's rank from its successors' current ranks, if they are all known: `succ[i..]` with the
/// maximum so far.
pub open spec fn spec_node_rank(s: Seq<usize>, rank: Seq<Option<usize>>, i: int, max: usize, any: bool) -> Option<usize>
    decreases s.len() - i,
{
    if 0 <= i < s.len() {
        match spec_rank(rank, s[i]) {
            Some(r) => spec_node_rank(s, rank, i + 1, if !any || r > max { r } else { max }, true),
            None => None,
        }
    } else if any {
        if max + 1 > usize::MAX { None } else { Some((max + 1) as usize) }
    } else {
        Some(0)
    }
}

/// One pass over nodes `x..`, in order, giving a rank to every node that can have one now.
pub open spec fn spec_pass(g: Seq<Seq<usize>>, rank: Seq<Option<usize>>, x: int, changed: bool) -> (Seq<Option<usize>>, bool)
    decreases g.len() - x,
{
    if 0 <= x < g.len() {
        if rank[x] is None {
            match spec_node_rank(g[x], rank, 0, 0, false) {
                Some(r) => spec_pass(g, rank.update(x, Some(r)), x + 1, true),
                None => spec_pass(g, rank, x + 1, changed),
            }
        } else {
            spec_pass(g, rank, x + 1, changed)
        }
    } else {
        (rank, changed)
    }
}

/// Passes until nothing changes, at most `rounds - round` more.
pub open spec fn spec_rounds(g: Seq<Seq<usize>>, rank: Seq<Option<usize>>, round: int, rounds: int) -> Seq<Option<usize>>
    decreases rounds - round,
{
    if round < rounds {
        let (next, changed) = spec_pass(g, rank, 0, false);
        if !changed { next } else { spec_rounds(g, next, round + 1, rounds) }
    } else {
        rank
    }
}

pub open spec fn spec_none_ranks(n: nat) -> Seq<Option<usize>> {
    Seq::new(n, |k: int| None)
}

pub open spec fn spec_node_ranks(g: Seq<Seq<usize>>) -> Seq<Option<usize>> {
    spec_rounds(g, spec_none_ranks(g.len()), 0, spec_add_or_max(g.len() as usize, 1) as int)
}

pub open spec fn spec_slice_ranks(rank: Seq<Option<usize>>, base: usize, n: usize) -> Seq<Option<usize>> {
    Seq::new(n as nat, |i: int| spec_rank(rank, spec_add_or_max(base, i as usize)))
}

/// The rank of every function, const, input and unit. A property of the program alone.
pub open spec fn spec_ranks(p: Program) -> SRanks {
    let r = spec_node_ranks(spec_graph(p));
    SRanks {
        fns: spec_slice_ranks(r, 0, spec_nf(p)),
        consts: spec_slice_ranks(r, spec_nf(p), spec_nc(p)),
        inputs: spec_slice_ranks(r, spec_bi(p), spec_ni(p)),
        units: spec_slice_ranks(r, spec_bu(p), spec_nu(p)),
    }
}

} // verus!
