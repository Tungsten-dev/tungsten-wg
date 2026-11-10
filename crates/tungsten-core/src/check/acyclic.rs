//! Ranks and cycles (§6.6). Both the checker and the interpreter use [`ranks`].
//!
//! Dependency edges: fn → fn (`Call`), fn → const (`Const`), fn → input (`Density`); const →
//! const; input → its source unit; unit → its main, every input it lists, and every const (eager
//! constants). `rank(x) = 1 + max(rank of successors)`, 0 for leaves, `None` if `x` is on or
//! reaches a cycle. Out-of-range ids (only in non-wf programs) are not edges.
//!
//! Everything is a bounded iterative fixpoint, with no recursion over the graph.

use vstd::prelude::*;

use crate::check::error::*;
use crate::ir::*;
use crate::util::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub struct Ranks {
    pub fns: Vec<Option<usize>>,
    pub consts: Vec<Option<usize>>,
    pub inputs: Vec<Option<usize>>,
    pub units: Vec<Option<usize>>,
}

/// The ids an expression or statement list refers to through edges.
pub struct Refs {
    pub fns: Vec<usize>,
    pub consts: Vec<usize>,
    pub inputs: Vec<usize>,
}

pub fn empty_refs() -> Refs {
    Refs { fns: Vec::new(), consts: Vec::new(), inputs: Vec::new() }
}

#[verifier::loop_isolation(false)]
pub fn refs_expr(e: &Expr, r: &mut Refs)
    decreases e,
{
    match e {
        Expr::Lit(_) | Expr::Local(_) | Expr::Host(_) | Expr::Table(_) | Expr::Ambient(_) => {},
        Expr::Const(c) => r.consts.push(c.0),
        Expr::Density { input, index } => {
            r.inputs.push(input.0);
            match index {
                Some(ix) => refs_expr(ix, r),
                None => {},
            }
        },
        Expr::Call { func, args } => {
            r.fns.push(func.0);
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                refs_expr(&args[i], r);
                i = i + 1;
            }
        },
        Expr::ExternCall { args, .. } => {
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                refs_expr(&args[i], r);
                i = i + 1;
            }
        },
        Expr::Binary { left, right, .. } => {
            refs_expr(left, r);
            refs_expr(right, r);
        },
        Expr::Unary { operand, .. } => refs_expr(operand, r),
        Expr::Field { base, .. } => refs_expr(base, r),
        Expr::Index { array, index } => {
            refs_expr(array, r);
            refs_expr(index, r);
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            refs_expr(x, r);
            refs_expr(y, r);
            refs_expr(z, r);
        },
        Expr::MakeRecord { fields, .. } => {
            let mut i: usize = 0;
            while i < fields.len()
                invariant
                    i <= fields.len(),
                decreases fields.len() - i,
            {
                refs_expr(&fields[i].value, r);
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
                refs_expr(&es[i], r);
                i = i + 1;
            }
        },
        Expr::Cast { operand, .. } => refs_expr(operand, r),
    }
}

#[verifier::loop_isolation(false)]
pub fn refs_stmts(ss: &Vec<Stmt>, r: &mut Refs)
    decreases ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        refs_stmt(&ss[i], r);
        i = i + 1;
    }
}

pub fn refs_stmt(s: &Stmt, r: &mut Refs)
    decreases s,
{
    match s {
        Stmt::Assign { value, .. } => refs_expr(value, r),
        Stmt::Return(e) => refs_expr(e, r),
        Stmt::If { cond, then_branch, else_branch } => {
            refs_expr(cond, r);
            refs_stmts(then_branch, r);
            refs_stmts(else_branch, r);
        },
        Stmt::While { cond, body } => {
            refs_expr(cond, r);
            refs_stmts(body, r);
        },
        Stmt::Repeat { body, .. } => refs_stmts(body, r),
        Stmt::Break => {},
    }
}

/// The dependency graph over nodes `fns ++ consts ++ inputs ++ units`.
pub struct Graph {
    pub nf: usize,
    pub nc: usize,
    pub ni: usize,
    pub nu: usize,
    pub succ: Vec<Vec<usize>>,
}

pub fn graph_len(g: &Graph) -> usize {
    add_or_max(add_or_max(g.nf, g.nc), add_or_max(g.ni, g.nu))
}

fn push_if_lt(out: &mut Vec<usize>, base: usize, id: usize, n: usize) {
    if id < n {
        out.push(add_or_max(base, id));
    }
}

fn push_all(out: &mut Vec<usize>, base: usize, ids: &Vec<usize>, n: usize) {
    let mut i: usize = 0;
    while i < ids.len()
        invariant
            i <= ids.len(),
        decreases ids.len() - i,
    {
        push_if_lt(out, base, ids[i], n);
        i = i + 1;
    }
}

pub fn graph(p: &Program) -> Graph {
    let nf = p.functions.len();
    let nc = p.consts.len();
    let ni = p.inputs.len();
    let nu = p.units.len();
    let bc = nf;
    let bi = add_or_max(nf, nc);
    let bu = add_or_max(bi, ni);
    let mut succ: Vec<Vec<usize>> = Vec::new();
    let mut k: usize = 0;
    while k < nf
        invariant
            k <= nf,
            nf == p.functions.len(),
        decreases nf - k,
    {
        let mut r = empty_refs();
        refs_stmts(&p.functions[k].body, &mut r);
        let mut out: Vec<usize> = Vec::new();
        push_all(&mut out, 0, &r.fns, nf);
        push_all(&mut out, bc, &r.consts, nc);
        push_all(&mut out, bi, &r.inputs, ni);
        succ.push(out);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < nc
        invariant
            k <= nc,
            nc == p.consts.len(),
        decreases nc - k,
    {
        let mut r = empty_refs();
        refs_expr(&p.consts[k].init, &mut r);
        let mut out: Vec<usize> = Vec::new();
        push_all(&mut out, 0, &r.fns, nf);
        push_all(&mut out, bc, &r.consts, nc);
        push_all(&mut out, bi, &r.inputs, ni);
        succ.push(out);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < ni
        invariant
            k <= ni,
            ni == p.inputs.len(),
        decreases ni - k,
    {
        let mut out: Vec<usize> = Vec::new();
        push_if_lt(&mut out, bu, p.inputs[k].source.0, nu);
        succ.push(out);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < nu
        invariant
            k <= nu,
            nu == p.units.len(),
        decreases nu - k,
    {
        let u = &p.units[k];
        let mut out: Vec<usize> = Vec::new();
        push_if_lt(&mut out, 0, u.main.0, nf);
        let mut j: usize = 0;
        while j < u.inputs.len()
            invariant
                j <= u.inputs.len(),
            decreases u.inputs.len() - j,
        {
            push_if_lt(&mut out, bi, u.inputs[j].0, ni);
            j = j + 1;
        }
        let mut c: usize = 0;
        while c < nc
            invariant
                c <= nc,
            decreases nc - c,
        {
            push_if_lt(&mut out, bc, c, nc);
            c = c + 1;
        }
        succ.push(out);
        k = k + 1;
    }
    Graph { nf, nc, ni, nu, succ }
}

/// One node's rank from its successors' current ranks, if they are all known.
fn node_rank(succ: &Vec<usize>, rank: &Vec<Option<usize>>) -> Option<usize> {
    let mut max: usize = 0;
    let mut any = false;
    let mut i: usize = 0;
    while i < succ.len()
        invariant
            i <= succ.len(),
        decreases succ.len() - i,
    {
        match get_rank(rank, succ[i]) {
            Some(r) => {
                if !any || r > max {
                    max = r;
                }
                any = true;
            },
            None => { return None; },
        }
        i = i + 1;
    }
    if any {
        max.checked_add(1)
    } else {
        Some(0)
    }
}

/// Per-node ranks over [`graph`].
pub fn node_ranks(g: &Graph) -> Vec<Option<usize>> {
    let n = g.succ.len();
    let mut rank = nones(n);
    let rounds = add_or_max(n, 1);
    let mut round: usize = 0;
    while round < rounds
        invariant
            round <= rounds,
            n == g.succ.len(),
        decreases rounds - round,
    {
        let mut changed = false;
        let mut x: usize = 0;
        while x < n
            invariant
                x <= n,
                n == g.succ.len(),
            decreases n - x,
        {
            if get_rank(&rank, x).is_none() {
                match node_rank(&g.succ[x], &rank) {
                    Some(r) => {
                        set(&mut rank, x, Some(r));
                        changed = true;
                    },
                    None => {},
                }
            }
            x = x + 1;
        }
        if !changed {
            break;
        }
        round = round + 1;
    }
    rank
}

fn slice_ranks(rank: &Vec<Option<usize>>, base: usize, n: usize) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = Vec::new();
    let mut i: usize = 0;
    while i < n
        invariant
            i <= n,
        decreases n - i,
    {
        out.push(get_rank(rank, add_or_max(base, i)));
        i = i + 1;
    }
    out
}

pub fn ranks(p: &Program) -> Ranks {
    let g = graph(p);
    let r = node_ranks(&g);
    let bi = add_or_max(g.nf, g.nc);
    let bu = add_or_max(bi, g.ni);
    Ranks {
        fns: slice_ranks(&r, 0, g.nf),
        consts: slice_ranks(&r, g.nf, g.nc),
        inputs: slice_ranks(&r, bi, g.ni),
        units: slice_ranks(&r, bu, g.nu),
    }
}

/// The nodes reachable from `x` in one or more steps, following `succ` (or its reverse).
fn reach(succ: &Vec<Vec<usize>>, x: usize, reverse: bool) -> Vec<bool> {
    let n = succ.len();
    let mut seen = bools(n, false);
    // Seed: the direct successors (or predecessors) of x.
    let mut y: usize = 0;
    while y < n
        invariant
            y <= n,
            n == succ.len(),
        decreases n - y,
    {
        if reverse {
            if contains_usize(&succ[y], x) {
                set(&mut seen, y, true);
            }
        } else if y == x {
            let s = &succ[y];
            let mut k: usize = 0;
            while k < s.len()
                invariant
                    k <= s.len(),
                decreases s.len() - k,
            {
                set(&mut seen, s[k], true);
                k = k + 1;
            }
        }
        y = y + 1;
    }
    let rounds = add_or_max(n, 1);
    let mut round: usize = 0;
    while round < rounds
        invariant
            round <= rounds,
            n == succ.len(),
        decreases rounds - round,
    {
        let mut changed = false;
        let mut a: usize = 0;
        while a < n
            invariant
                a <= n,
                n == succ.len(),
            decreases n - a,
        {
            let s = &succ[a];
            let mut k: usize = 0;
            while k < s.len()
                invariant
                    k <= s.len(),
                decreases s.len() - k,
            {
                let b = s[k];
                // Forward: seen(a) ⇒ seen(b). Reverse: seen(b) ⇒ seen(a).
                if reverse {
                    if get_bool(&seen, b) && !get_bool(&seen, a) {
                        set(&mut seen, a, true);
                        changed = true;
                    }
                } else if get_bool(&seen, a) && !get_bool(&seen, b) {
                    set(&mut seen, b, true);
                    changed = true;
                }
                k = k + 1;
            }
            a = a + 1;
        }
        if !changed {
            break;
        }
        round = round + 1;
    }
    seen
}

fn member(g: &Graph, x: usize) -> CycleMember {
    let bi = add_or_max(g.nf, g.nc);
    let bu = add_or_max(bi, g.ni);
    if x < g.nf {
        CycleMember::Function(FnId(x))
    } else if x < bi {
        CycleMember::Const(ConstId(x - g.nf))
    } else if x < bu {
        CycleMember::Input(InputId(x - bi))
    } else {
        CycleMember::Unit(UnitId(if x >= bu { x - bu } else { 0 }))
    }
}

/// Every cycle (strongly connected component with an edge), each once, ordered by its lowest
/// member in node order (fns, consts, inputs, units). Members are in node order.
pub fn cycles(p: &Program) -> Vec<Vec<CycleMember>> {
    let g = graph(p);
    let rank = node_ranks(&g);
    let n = g.succ.len();
    let mut done = bools(n, false);
    let mut out: Vec<Vec<CycleMember>> = Vec::new();
    let mut x: usize = 0;
    while x < n
        invariant
            x <= n,
            n == g.succ.len(),
        decreases n - x,
    {
        if get_rank(&rank, x).is_none() && !get_bool(&done, x) {
            let fwd = reach(&g.succ, x, false);
            if get_bool(&fwd, x) {
                let back = reach(&g.succ, x, true);
                let mut members: Vec<CycleMember> = Vec::new();
                let mut y: usize = 0;
                while y < n
                    invariant
                        y <= n,
                    decreases n - y,
                {
                    if get_bool(&fwd, y) && get_bool(&back, y) {
                        set(&mut done, y, true);
                        members.push(member(&g, y));
                    }
                    y = y + 1;
                }
                out.push(members);
            }
        }
        x = x + 1;
    }
    out
}

} // verus!
