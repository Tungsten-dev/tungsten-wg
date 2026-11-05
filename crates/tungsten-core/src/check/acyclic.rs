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
use crate::proofs::ranks::*;
use crate::spec::eval::*;
use crate::spec::ranks::*;
use crate::spec::typing::*;
use crate::spec::value::*;
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

pub open spec fn view_refs(r: Refs) -> SRefs {
    SRefs { fns: r.fns@, consts: r.consts@, inputs: r.inputs@ }
}

pub open spec fn view_graph(succ: Seq<Vec<usize>>) -> Seq<Seq<usize>> {
    Seq::new(succ.len(), |j: int| succ[j]@)
}

pub fn empty_refs() -> (r: Refs)
    ensures
        view_refs(r) == spec_empty_refs(),
{
    Refs { fns: Vec::new(), consts: Vec::new(), inputs: Vec::new() }
}

#[verifier::loop_isolation(false)]
fn refs_exprs(es: &Vec<Expr>, r: &mut Refs)
    ensures
        view_refs(*final(r)) == spec_refs_exprs(*es, 0, view_refs(*old(r))),
    decreases es,
{
    let ghost start = view_refs(*r);
    let mut i: usize = 0;
    while i < es.len()
        invariant
            i <= es.len(),
            spec_refs_exprs(*es, 0, start) == spec_refs_exprs(*es, i as int, view_refs(*r)),
        decreases es.len() - i,
    {
        refs_expr(&es[i], r);
        i = i + 1;
    }
}

#[verifier::loop_isolation(false)]
fn refs_fields(fs: &Vec<FieldInit>, r: &mut Refs)
    ensures
        view_refs(*final(r)) == spec_refs_fields(*fs, 0, view_refs(*old(r))),
    decreases fs,
{
    let ghost start = view_refs(*r);
    let mut i: usize = 0;
    while i < fs.len()
        invariant
            i <= fs.len(),
            spec_refs_fields(*fs, 0, start) == spec_refs_fields(*fs, i as int, view_refs(*r)),
        decreases fs.len() - i,
    {
        refs_expr(&fs[i].value, r);
        i = i + 1;
    }
}

pub fn refs_expr(e: &Expr, r: &mut Refs)
    ensures
        view_refs(*final(r)) == spec_refs_expr(*e, view_refs(*old(r))),
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
            refs_exprs(args, r);
        },
        Expr::ExternCall { args, .. } => refs_exprs(args, r),
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
        Expr::MakeRecord { fields, .. } => refs_fields(fields, r),
        Expr::ArrayLit(es) => refs_exprs(es, r),
        Expr::Cast { operand, .. } => refs_expr(operand, r),
    }
}

#[verifier::loop_isolation(false)]
pub fn refs_stmts(ss: &Vec<Stmt>, r: &mut Refs)
    ensures
        view_refs(*final(r)) == spec_refs_stmts(*ss, 0, view_refs(*old(r))),
    decreases ss,
{
    let ghost start = view_refs(*r);
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
            spec_refs_stmts(*ss, 0, start) == spec_refs_stmts(*ss, i as int, view_refs(*r)),
        decreases ss.len() - i,
    {
        refs_stmt(&ss[i], r);
        i = i + 1;
    }
}

pub fn refs_stmt(s: &Stmt, r: &mut Refs)
    ensures
        view_refs(*final(r)) == spec_refs_stmt(*s, view_refs(*old(r))),
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

fn push_if_lt(out: &mut Vec<usize>, base: usize, id: usize, n: usize)
    ensures
        final(out)@ == spec_push_if_lt(old(out)@, base, id, n),
{
    if id < n {
        out.push(add_or_max(base, id));
    }
}

fn push_all(out: &mut Vec<usize>, base: usize, ids: &Vec<usize>, n: usize)
    ensures
        final(out)@ == spec_push_all(old(out)@, base, ids@, n, 0),
{
    let ghost start = out@;
    let mut i: usize = 0;
    while i < ids.len()
        invariant
            i <= ids.len(),
            spec_push_all(start, base, ids@, n, 0) == spec_push_all(out@, base, ids@, n, i as int),
        decreases ids.len() - i,
    {
        push_if_lt(out, base, ids[i], n);
        i = i + 1;
    }
}

/// The edges out of a body's references.
fn ref_edges(p: &Program, r: &Refs) -> (out: Vec<usize>)
    ensures
        out@ == spec_ref_edges(*p, view_refs(*r)),
{
    let nf = p.functions.len();
    let nc = p.consts.len();
    let ni = p.inputs.len();
    let bi = add_or_max(nf, nc);
    let mut out: Vec<usize> = Vec::new();
    proof {
        assert(out@ =~= Seq::<usize>::empty());
    }
    push_all(&mut out, 0, &r.fns, nf);
    push_all(&mut out, nf, &r.consts, nc);
    push_all(&mut out, bi, &r.inputs, ni);
    out
}

fn unit_edges(p: &Program, u: &Unit) -> (out: Vec<usize>)
    ensures
        out@ == spec_unit_edges(*p, *u),
{
    let nf = p.functions.len();
    let nc = p.consts.len();
    let ni = p.inputs.len();
    let bi = add_or_max(nf, nc);
    let mut out: Vec<usize> = Vec::new();
    proof {
        assert(out@ =~= Seq::<usize>::empty());
    }
    push_if_lt(&mut out, 0, u.main.0, nf);
    let ghost a = out@;
    let mut j: usize = 0;
    while j < u.inputs.len()
        invariant
            j <= u.inputs.len(),
            spec_push_inputs(a, spec_bi(*p), u.inputs@, spec_ni(*p), 0)
                == spec_push_inputs(out@, spec_bi(*p), u.inputs@, spec_ni(*p), j as int),
            bi == spec_bi(*p),
            ni == spec_ni(*p),
        decreases u.inputs.len() - j,
    {
        push_if_lt(&mut out, bi, u.inputs[j].0, ni);
        j = j + 1;
    }
    let ghost b = out@;
    let mut c: usize = 0;
    while c < nc
        invariant
            c <= nc,
            spec_push_consts(b, nf, nc, 0) == spec_push_consts(out@, nf, nc, c as int),
        decreases nc - c,
    {
        push_if_lt(&mut out, nf, c, nc);
        c = c + 1;
    }
    out
}

pub fn graph(p: &Program) -> (g: Graph)
    ensures
        g.nf == p.functions@.len(),
        g.nc == p.consts@.len(),
        g.ni == p.inputs@.len(),
        g.nu == p.units@.len(),
        view_graph(g.succ@) == spec_graph(*p),
{
    let nf = p.functions.len();
    let nc = p.consts.len();
    let ni = p.inputs.len();
    let nu = p.units.len();
    let bi = add_or_max(nf, nc);
    let bu = add_or_max(bi, ni);
    let mut succ: Vec<Vec<usize>> = Vec::new();
    let mut k: usize = 0;
    while k < nf
        invariant
            k <= nf,
            nf == p.functions.len(),
            succ@.len() == k,
            forall|j: int| 0 <= j < k ==> (#[trigger] succ@[j])@ == spec_succ(*p, j),
        decreases nf - k,
    {
        let mut r = empty_refs();
        refs_stmts(&p.functions[k].body, &mut r);
        succ.push(ref_edges(p, &r));
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < nc
        invariant
            k <= nc,
            nf == p.functions.len(),
            nc == p.consts.len(),
            succ@.len() == nf + k,
            forall|j: int| 0 <= j < nf + k ==> (#[trigger] succ@[j])@ == spec_succ(*p, j),
        decreases nc - k,
    {
        let mut r = empty_refs();
        refs_expr(&p.consts[k].init, &mut r);
        succ.push(ref_edges(p, &r));
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < ni
        invariant
            k <= ni,
            nf == p.functions.len(),
            nc == p.consts.len(),
            ni == p.inputs.len(),
            nu == p.units.len(),
            bu == spec_bu(*p),
            succ@.len() == nf + nc + k,
            forall|j: int| 0 <= j < nf + nc + k ==> (#[trigger] succ@[j])@ == spec_succ(*p, j),
        decreases ni - k,
    {
        let mut out: Vec<usize> = Vec::new();
        proof {
            assert(out@ =~= Seq::<usize>::empty());
        }
        push_if_lt(&mut out, bu, p.inputs[k].source.0, nu);
        succ.push(out);
        k = k + 1;
    }
    let mut k: usize = 0;
    while k < nu
        invariant
            k <= nu,
            nf == p.functions.len(),
            nc == p.consts.len(),
            ni == p.inputs.len(),
            nu == p.units.len(),
            succ@.len() == nf + nc + ni + k,
            forall|j: int| 0 <= j < nf + nc + ni + k ==> (#[trigger] succ@[j])@ == spec_succ(*p, j),
        decreases nu - k,
    {
        succ.push(unit_edges(p, &p.units[k]));
        k = k + 1;
    }
    proof {
        assert(view_graph(succ@) =~= spec_graph(*p));
    }
    Graph { nf, nc, ni, nu, succ }
}

/// One node's rank from its successors' current ranks, if they are all known.
fn node_rank(succ: &Vec<usize>, rank: &Vec<Option<usize>>) -> (r: Option<usize>)
    ensures
        r == spec_node_rank(succ@, rank@, 0, 0, false),
{
    let mut max: usize = 0;
    let mut any = false;
    let mut i: usize = 0;
    while i < succ.len()
        invariant
            i <= succ.len(),
            spec_node_rank(succ@, rank@, 0, 0, false) == spec_node_rank(succ@, rank@, i as int, max, any),
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

/// One pass over all nodes. Returns whether any node got a rank.
fn pass(g: &Graph, rank: &mut Vec<Option<usize>>) -> (changed: bool)
    requires
        old(rank)@.len() == g.succ@.len(),
    ensures
        (final(rank)@, changed) == spec_pass(view_graph(g.succ@), old(rank)@, 0, false),
        final(rank)@.len() == g.succ@.len(),
{
    let n = g.succ.len();
    let ghost vg = view_graph(g.succ@);
    let ghost start = rank@;
    let mut changed = false;
    let mut x: usize = 0;
    while x < n
        invariant
            x <= n,
            n == g.succ.len(),
            vg == view_graph(g.succ@),
            rank@.len() == n,
            spec_pass(vg, start, 0, false) == spec_pass(vg, rank@, x as int, changed),
        decreases n - x,
    {
        if get_rank(rank, x).is_none() {
            match node_rank(&g.succ[x], rank) {
                Some(r) => {
                    set(rank, x, Some(r));
                    changed = true;
                },
                None => {},
            }
        }
        x = x + 1;
    }
    changed
}

/// Per-node ranks over [`graph`].
pub fn node_ranks(g: &Graph) -> (out: Vec<Option<usize>>)
    ensures
        out@ == spec_node_ranks(view_graph(g.succ@)),
{
    let n = g.succ.len();
    let ghost vg = view_graph(g.succ@);
    let mut rank = nones(n);
    let rounds = add_or_max(n, 1);
    let mut round: usize = 0;
    proof {
        assert(rank@ =~= spec_none_ranks(vg.len()));
    }
    while round < rounds
        invariant
            round <= rounds,
            n == g.succ.len(),
            vg == view_graph(g.succ@),
            rank@.len() == n,
            spec_node_ranks(vg) == spec_rounds(vg, rank@, round as int, rounds as int),
        decreases rounds - round,
    {
        let changed = pass(g, &mut rank);
        if !changed {
            return rank;
        }
        round = round + 1;
    }
    rank
}

fn slice_ranks(rank: &Vec<Option<usize>>, base: usize, n: usize) -> (out: Vec<Option<usize>>)
    ensures
        out@ == spec_slice_ranks(rank@, base, n),
{
    let mut out: Vec<Option<usize>> = Vec::new();
    let mut i: usize = 0;
    while i < n
        invariant
            i <= n,
            out@.len() == i,
            forall|j: int| 0 <= j < i ==> out@[j] == spec_rank(rank@, spec_add_or_max(base, j as usize)),
        decreases n - i,
    {
        out.push(get_rank(rank, add_or_max(base, i)));
        i = i + 1;
    }
    proof {
        assert(out@ =~= spec_slice_ranks(rank@, base, n));
    }
    out
}

pub fn ranks(p: &Program) -> (r: Ranks)
    ensures
        r.fns@.len() == p.functions@.len(),
        r.consts@.len() == p.consts@.len(),
        r.inputs@.len() == p.inputs@.len(),
        r.units@.len() == p.units@.len(),
        view_ranks(r) == spec_ranks(*p),
{
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

pub open spec fn count_true(s: Seq<bool>, n: int) -> int
    decreases n,
{
    if n <= 0 { 0 } else { count_true(s, n - 1) + (if s[n - 1] { 1int } else { 0int }) }
}

proof fn lemma_count_true_le(s: Seq<bool>, n: int)
    requires
        n <= s.len(),
    ensures
        0 <= count_true(s, n) <= if n <= 0 { 0 } else { n },
    decreases n,
{
    if n > 0 {
        lemma_count_true_le(s, n - 1);
    }
}

/// Setting a `false` slot adds one.
proof fn lemma_count_true_set(s: Seq<bool>, x: int, n: int)
    requires
        0 <= x < s.len(),
        !s[x],
        n <= s.len(),
    ensures
        count_true(s.update(x, true), n) == count_true(s, n) + (if x < n { 1int } else { 0int }),
    decreases n,
{
    if n > 0 {
        lemma_count_true_set(s, x, n - 1);
    }
}

/// The nodes reachable from `x` in one or more steps, following `succ` (or its reverse). Forward,
/// over a graph whose edges are all in range, the result is closed under successors.
#[verifier::loop_isolation(false)]
fn reach(succ: &Vec<Vec<usize>>, x: usize, reverse: bool) -> (seen: Vec<bool>)
    ensures
        seen@.len() == succ@.len(),
        !reverse && x < succ@.len() && g_in_range(view_graph(succ@)) && succ@.len() < usize::MAX
            ==> reach_closed(view_graph(succ@), x as int, seen@),
{
    let n = succ.len();
    let ghost vg = view_graph(succ@);
    let ghost hyp = !reverse && x < succ@.len() && g_in_range(vg) && succ@.len() < usize::MAX;
    let mut seen = bools(n, false);
    // Seed: the direct successors (or predecessors) of x.
    let mut y: usize = 0;
    while y < n
        invariant
            y <= n,
            n == succ.len(),
            seen@.len() == n,
            hyp && y > x ==> forall|k: int| 0 <= k < vg[x as int].len() ==> seen@[#[trigger] vg[x as int][k] as int],
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
                    seen@.len() == n,
                    y == x,
                    n == succ.len(),
                    x < n,
                    *s == succ@[x as int],
                    hyp ==> forall|j: int| 0 <= j < k ==> seen@[#[trigger] vg[x as int][j] as int],
                decreases s.len() - k,
            {
                proof {
                    if hyp {
                        assert(vg[x as int] == s@);
                        assert(vg[x as int][k as int] < vg.len());
                    }
                }
                set(&mut seen, s[k], true);
                proof {
                    if hyp {
                        assert(vg[x as int] == s@);
                    }
                }
                k = k + 1;
            }
        }
        y = y + 1;
    }
    proof {
        lemma_count_true_le(seen@, n as int);
    }
    let rounds = add_or_max(n, 1);
    let mut round: usize = 0;
    while round < rounds
        invariant
            round <= rounds,
            n == succ.len(),
            seen@.len() == n,
            hyp ==> rounds == n + 1,
            hyp ==> round <= count_true(seen@, n as int),
            hyp ==> forall|k: int| 0 <= k < vg[x as int].len() ==> seen@[#[trigger] vg[x as int][k] as int],
        decreases rounds - round,
    {
        let ghost start = seen@;
        let mut changed = false;
        let mut a: usize = 0;
        while a < n
            invariant
                a <= n,
                n == succ.len(),
                seen@.len() == n,
                hyp ==> {
                    &&& forall|j: int| 0 <= j < n && start[j] ==> seen@[j]
                    &&& changed ==> count_true(seen@, n as int) >= count_true(start, n as int) + 1
                    &&& count_true(seen@, n as int) >= count_true(start, n as int)
                    &&& !changed ==> seen@ == start
                    &&& !changed ==> forall|p: int, q: int| #![trigger vg[p][q]] 0 <= p < a && 0 <= q < vg[p].len() && seen@[p] ==> seen@[vg[p][q] as int]
                },
            decreases n - a,
        {
            let s = &succ[a];
            let ghost at_a = seen@;
            let mut k: usize = 0;
            while k < s.len()
                invariant
                    k <= s.len(),
                    a < n,
                    n == succ.len(),
                    *s == succ@[a as int],
                    seen@.len() == n,
                    hyp ==> {
                        &&& forall|j: int| 0 <= j < n && start[j] ==> seen@[j]
                        &&& changed ==> count_true(seen@, n as int) >= count_true(start, n as int) + 1
                        &&& count_true(seen@, n as int) >= count_true(start, n as int)
                        &&& !changed ==> seen@ == start
                        &&& !changed ==> forall|p: int, q: int| #![trigger vg[p][q]] 0 <= p < a && 0 <= q < vg[p].len() && seen@[p] ==> seen@[vg[p][q] as int]
                        &&& !changed ==> forall|q: int| 0 <= q < k && seen@[a as int] ==> seen@[#[trigger] vg[a as int][q] as int]
                    },
                decreases s.len() - k,
            {
                let b = s[k];
                proof {
                    if hyp {
                        assert(vg[a as int] == s@);
                        assert(vg[a as int][k as int] < vg.len());
                    }
                }
                // Forward: seen(a) ⇒ seen(b). Reverse: seen(b) ⇒ seen(a).
                if reverse {
                    if get_bool(&seen, b) && !get_bool(&seen, a) {
                        set(&mut seen, a, true);
                        changed = true;
                    }
                } else if get_bool(&seen, a) && !get_bool(&seen, b) {
                    let ghost before = seen@;
                    let ghost was_changed = changed;
                    set(&mut seen, b, true);
                    proof {
                        if hyp {
                            lemma_count_true_set(before, b as int, n as int);
                        }
                    }
                    changed = true;
                }
                k = k + 1;
            }
            a = a + 1;
        }
        if !changed {
            proof {
                if hyp {
                    assert(reach_closed(vg, x as int, seen@));
                }
            }
            return seen;
        }
        round = round + 1;
    }
    proof {
        if hyp {
            lemma_count_true_le(seen@, n as int);
            assert(false);
        }
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
/// The nodes reachable both ways from a node on a cycle, marked as done.
fn cycle_members(g: &Graph, fwd: &Vec<bool>, back: &Vec<bool>, done: &mut Vec<bool>) -> (members: Vec<CycleMember>)
    ensures
        final(done)@.len() == old(done)@.len(),
{
    let n = g.succ.len();
    let mut members: Vec<CycleMember> = Vec::new();
    let mut y: usize = 0;
    let ghost d0 = done@.len();
    while y < n
        invariant
            y <= n,
            n == g.succ.len(),
            done@.len() == d0,
            d0 == old(done)@.len(),
        decreases n - y,
    {
        if get_bool(fwd, y) && get_bool(back, y) {
            set(done, y, true);
            members.push(member(g, y));
        }
        y = y + 1;
    }
    members
}

pub fn cycles(p: &Program) -> (out: Vec<Vec<CycleMember>>)
    ensures
        out@.len() == 0 && spec_program_fits(*p) ==> forall|x: int| 0 <= x < spec_graph(*p).len() && (#[trigger] spec_node_ranks(spec_graph(*p))[x]) is None
            ==> no_self_reach(spec_graph(*p), x),
{
    let g = graph(p);
    let rank = node_ranks(&g);
    let ghost vg = view_graph(g.succ@);
    let ghost fits = spec_program_fits(*p);
    proof {
        if fits {
            lemma_graph_in_range(*p);
            lemma_node_ranks(vg);
        }
    }
    let ghost sg = spec_graph(*p);
    proof {
        assert(sg == vg);
    }
    let n = g.succ.len();
    let mut done = bools(n, false);
    let mut out: Vec<Vec<CycleMember>> = Vec::new();
    let mut x: usize = 0;
    while x < n
        invariant
            x <= n,
            n == g.succ.len(),
            vg == view_graph(g.succ@),
            rank@ == spec_node_ranks(vg),
            done@.len() == n,
            fits ==> g_in_range(vg) && vg.len() < usize::MAX && rank@.len() == vg.len(),
            out@.len() == 0 ==> forall|j: int| 0 <= j < n ==> !done@[j],
            out@.len() == 0 && fits ==> forall|j: int| 0 <= j < x && (#[trigger] rank@[j]) is None ==> no_self_reach(vg, j),
        decreases n - x,
    {
        if get_rank(&rank, x).is_none() && !get_bool(&done, x) {
            let fwd = reach(&g.succ, x, false);
            if get_bool(&fwd, x) {
                let back = reach(&g.succ, x, true);
                let members = cycle_members(&g, &fwd, &back, &mut done);
                out.push(members);
            } else {
                proof {
                    if fits {
                        assert(reach_closed(vg, x as int, fwd@) && !fwd@[x as int]);
                        assert(no_self_reach(vg, x as int));
                    }
                }
            }
        }
        x = x + 1;
    }
    out
}

} // verus!
