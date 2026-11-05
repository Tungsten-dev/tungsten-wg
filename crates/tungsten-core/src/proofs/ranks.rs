//! From "no cycle reported" to "every id has a rank and every edge goes strictly down" (PROOF.md §7.5).
//!
//! The bounded fixpoint first: ranks only ever get added, every one is valid when added, and with
//! one more round than there are nodes the loop stops at a real fixpoint. At a fixpoint an unranked
//! node has an unranked successor, so if any node is unranked, walking successors must eventually
//! repeat a node, and that node reaches itself, which `cycles` would report.

use vstd::prelude::*;
use vstd::seq_lib::*;
use vstd::set_lib::*;

use crate::ir::*;
use crate::spec::eval::*;
use crate::spec::ranks::*;
use crate::spec::typing::*;
use crate::spec::value::*;
use crate::wf;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub open spec fn g_in_range(g: Seq<Seq<usize>>) -> bool {
    forall|x: int, i: int| #![trigger g[x][i]] 0 <= x < g.len() && 0 <= i < g[x].len() ==> g[x][i] < g.len()
}

/// A rank is only present when every successor has a lower one.
pub open spec fn valid_ranks(g: Seq<Seq<usize>>, rank: Seq<Option<usize>>) -> bool {
    forall|x: int, i: int| #![trigger g[x][i]] 0 <= x < g.len() && 0 <= i < g[x].len() && rank[x] is Some
        ==> spec_rank(rank, g[x][i]) is Some && spec_rank(rank, g[x][i])->0 < rank[x]->0
}

pub open spec fn count_some(rank: Seq<Option<usize>>, n: int) -> int
    decreases n,
{
    if n <= 0 { 0 } else { count_some(rank, n - 1) + (if rank[n - 1] is Some { 1int } else { 0int }) }
}

/// A rank is below the number of ranked nodes, so it can't overflow.
pub open spec fn rank_bounded(rank: Seq<Option<usize>>) -> bool {
    forall|x: int| 0 <= x < rank.len() && (#[trigger] rank[x]) is Some ==> (rank[x]->0 as int) < count_some(rank, rank.len() as int)
}

/// Every unranked node really can't get a rank yet.
pub open spec fn rank_fixpoint(g: Seq<Seq<usize>>, rank: Seq<Option<usize>>) -> bool {
    forall|x: int| 0 <= x < g.len() && (#[trigger] rank[x]) is None ==> spec_node_rank(g[x], rank, 0, 0, false) is None
}

proof fn lemma_count_le(rank: Seq<Option<usize>>, n: int)
    requires
        n <= rank.len(),
    ensures
        count_some(rank, n) <= if n <= 0 { 0 } else { n },
        count_some(rank, n) >= 0,
    decreases n,
{
    if n > 0 {
        lemma_count_le(rank, n - 1);
    }
}

proof fn lemma_count_update(rank: Seq<Option<usize>>, x: int, r: usize, n: int)
    requires
        0 <= x < rank.len(),
        rank[x] is None,
        n <= rank.len(),
    ensures
        count_some(rank.update(x, Some(r)), n) == count_some(rank, n) + (if x < n { 1int } else { 0int }),
    decreases n,
{
    if n > 0 {
        lemma_count_update(rank, x, r, n - 1);
    }
}

proof fn lemma_nr_sound(s: Seq<usize>, rank: Seq<Option<usize>>, i: int, max: usize, any: bool)
    requires
        0 <= i,
    ensures
        ({
            let out = spec_node_rank(s, rank, i, max, any);
            out is Some ==> {
                &&& forall|j: int| #![trigger s[j]] i <= j < s.len() ==> spec_rank(rank, s[j]) is Some && spec_rank(rank, s[j])->0 < out->0
                &&& any ==> max < out->0
            }
        }),
    decreases s.len() - i,
{
    if i < s.len() {
        match spec_rank(rank, s[i]) {
            Some(q) => lemma_nr_sound(s, rank, i + 1, if !any || q > max { q } else { max }, true),
            None => {},
        }
    }
}

proof fn lemma_nr_complete(s: Seq<usize>, rank: Seq<Option<usize>>, i: int, max: usize, any: bool, b: int)
    requires
        0 <= i,
        -1 <= b < usize::MAX,
        any ==> max <= b,
        forall|j: int| #![trigger s[j]] i <= j < s.len() ==> spec_rank(rank, s[j]) is Some && spec_rank(rank, s[j])->0 <= b,
    ensures
        spec_node_rank(s, rank, i, max, any) is Some,
        spec_node_rank(s, rank, i, max, any)->0 <= b + 1,
    decreases s.len() - i,
{
    if i < s.len() {
        assert(spec_rank(rank, s[i]) is Some && spec_rank(rank, s[i])->0 <= b);
        let q = spec_rank(rank, s[i])->0;
        lemma_nr_complete(s, rank, i + 1, if !any || q > max { q } else { max }, true, b);
    }
}

/// What one pass does, from node `x` on.
proof fn lemma_pass(g: Seq<Seq<usize>>, rank: Seq<Option<usize>>, x: int, ch: bool)
    requires
        0 <= x,
        rank.len() == g.len(),
        g.len() < usize::MAX,
        valid_ranks(g, rank),
        rank_bounded(rank),
    ensures
        ({
            let (r2, c2) = spec_pass(g, rank, x, ch);
            &&& r2.len() == rank.len()
            &&& valid_ranks(g, r2)
            &&& rank_bounded(r2)
            &&& ch ==> c2
            &&& count_some(r2, r2.len() as int) >= count_some(rank, rank.len() as int)
            &&& (!ch && c2) ==> count_some(r2, r2.len() as int) >= count_some(rank, rank.len() as int) + 1
            &&& !c2 ==> r2 == rank && forall|j: int| x <= j < g.len() && (#[trigger] rank[j]) is None ==> spec_node_rank(g[j], rank, 0, 0, false) is None
        }),
    decreases g.len() - x,
{
    if x < g.len() {
        if rank[x] is None {
            match spec_node_rank(g[x], rank, 0, 0, false) {
                Some(r) => {
                    let r1 = rank.update(x, Some(r));
                    lemma_nr_sound(g[x], rank, 0, 0, false);
                    lemma_count_update(rank, x, r, rank.len() as int);
                    let c0 = count_some(rank, rank.len() as int);
                    assert(r1.len() == g.len());
                    // The new rank is within bound.
                    assert forall|j: int| #![trigger g[x][j]] 0 <= j < g[x].len() implies spec_rank(rank, g[x][j]) is Some && spec_rank(rank, g[x][j])->0 <= c0 - 1 by {
                        assert(spec_rank(rank, g[x][j]) is Some && spec_rank(rank, g[x][j])->0 < r);
                        let q = g[x][j] as int;
                        assert(rank[q] is Some);
                    }
                    lemma_count_le(rank, rank.len() as int);
                    assert((c0 - 1) < usize::MAX);
                    assert forall|j: int| #![trigger g[x][j]] 0 <= j < g[x].len() implies (spec_rank(rank, g[x][j]) is Some && spec_rank(rank, g[x][j])->0 <= c0 - 1) by {}
                    lemma_nr_complete_top(g[x], rank, c0 - 1);
                    assert(r as int <= c0);
                    assert forall|a: int, i: int| #![trigger g[a][i]] 0 <= a < g.len() && 0 <= i < g[a].len() && r1[a] is Some
                        implies spec_rank(r1, g[a][i]) is Some && spec_rank(r1, g[a][i])->0 < r1[a]->0 by {
                        if a == x {
                            assert(spec_rank(rank, g[a][i]) is Some && spec_rank(rank, g[a][i])->0 < r);
                        } else {
                            assert(rank[a] is Some);
                            assert(spec_rank(rank, g[a][i]) is Some && spec_rank(rank, g[a][i])->0 < rank[a]->0);
                        }
                    }
                    assert forall|a: int| 0 <= a < r1.len() && (#[trigger] r1[a]) is Some implies (r1[a]->0 as int) < count_some(r1, r1.len() as int) by {
                        if a != x {
                            assert(rank[a] is Some);
                        }
                    }
                    lemma_pass(g, r1, x + 1, true);
                },
                None => lemma_pass(g, rank, x + 1, ch),
            }
        } else {
            lemma_pass(g, rank, x + 1, ch);
        }
    }
}

proof fn lemma_nr_complete_top(s: Seq<usize>, rank: Seq<Option<usize>>, b: int)
    requires
        -1 <= b < usize::MAX,
        forall|j: int| #![trigger s[j]] 0 <= j < s.len() ==> spec_rank(rank, s[j]) is Some && spec_rank(rank, s[j])->0 <= b,
    ensures
        spec_node_rank(s, rank, 0, 0, false) is Some,
        spec_node_rank(s, rank, 0, 0, false)->0 <= b + 1,
{
    lemma_nr_complete(s, rank, 0, 0, false, b);
}

proof fn lemma_rounds(g: Seq<Seq<usize>>, rank: Seq<Option<usize>>, round: int, rounds: int)
    requires
        rounds == g.len() + 1,
        0 <= round <= count_some(rank, rank.len() as int),
        rank.len() == g.len(),
        g.len() < usize::MAX,
        valid_ranks(g, rank),
        rank_bounded(rank),
    ensures
        ({
            let r = spec_rounds(g, rank, round, rounds);
            &&& r.len() == g.len()
            &&& valid_ranks(g, r)
            &&& rank_bounded(r)
            &&& rank_fixpoint(g, r)
        }),
    decreases rounds - round,
{
    lemma_count_le(rank, rank.len() as int);
    assert(round < rounds);
    lemma_pass(g, rank, 0, false);
    let (next, changed) = spec_pass(g, rank, 0, false);
    if changed {
        lemma_rounds(g, next, round + 1, rounds);
    }
}

/// **The node ranks' facts.**
pub proof fn lemma_node_ranks(g: Seq<Seq<usize>>)
    requires
        g.len() < usize::MAX,
    ensures
        ({
            let r = spec_node_ranks(g);
            &&& r.len() == g.len()
            &&& valid_ranks(g, r)
            &&& rank_bounded(r)
            &&& rank_fixpoint(g, r)
        }),
{
    let none = spec_none_ranks(g.len());
    assert forall|x: int, i: int| #![trigger g[x][i]] 0 <= x < g.len() && 0 <= i < g[x].len() && none[x] is Some
        implies spec_rank(none, g[x][i]) is Some && spec_rank(none, g[x][i])->0 < none[x]->0 by {}
    assert(valid_ranks(g, none));
    assert(rank_bounded(none));
    lemma_count_le(none, none.len() as int);
    lemma_rounds(g, none, 0, g.len() as int + 1);
}

// ---------------------------------------------------------------------------------------------
// Reachability and the pigeonhole
// ---------------------------------------------------------------------------------------------

/// `seen` contains `x`'s direct successors and is closed under successors. If `!seen[x]`, then `x`
/// can't reach itself.
pub open spec fn reach_closed(g: Seq<Seq<usize>>, x: int, seen: Seq<bool>) -> bool {
    &&& seen.len() == g.len()
    &&& forall|k: int| #![trigger g[x][k]] 0 <= k < g[x].len() ==> seen[g[x][k] as int]
    &&& forall|a: int, k: int| #![trigger g[a][k]] 0 <= a < g.len() && 0 <= k < g[a].len() && seen[a] ==> seen[g[a][k] as int]
}

pub open spec fn no_self_reach(g: Seq<Seq<usize>>, x: int) -> bool {
    exists|seen: Seq<bool>| #[trigger] reach_closed(g, x, seen) && !seen[x]
}

proof fn lemma_unranked_successor(g: Seq<Seq<usize>>, r: Seq<Option<usize>>, x: int) -> (y: int)
    requires
        r.len() == g.len(),
        g.len() < usize::MAX,
        g_in_range(g),
        rank_bounded(r),
        rank_fixpoint(g, r),
        0 <= x < g.len(),
        r[x] is None,
    ensures
        0 <= y < g.len(),
        r[y] is None,
        g[x].contains(y as usize),
{
    let c0 = count_some(r, r.len() as int);
    lemma_count_le(r, r.len() as int);
    if forall|j: int| #![trigger g[x][j]] 0 <= j < g[x].len() ==> spec_rank(r, g[x][j]) is Some {
        assert forall|j: int| #![trigger g[x][j]] 0 <= j < g[x].len() implies spec_rank(r, g[x][j]) is Some && spec_rank(r, g[x][j])->0 <= c0 - 1 by {
            assert(spec_rank(r, g[x][j]) is Some);
            let q = g[x][j] as int;
            assert(g[x][j] < g.len());
            assert(r[q] is Some);
        }
        lemma_nr_complete_top(g[x], r, c0 - 1);
        assert(false);
        0
    } else {
        let j = choose|j: int| #![trigger g[x][j]] 0 <= j < g[x].len() && !(spec_rank(r, g[x][j]) is Some);
        assert(g[x][j] < g.len());
        assert(g[x][j as int] == g[x][j]);
        g[x][j] as int
    }
}

/// Along a path of edges from a successor of `from`, a set closed from `from` contains everything.
proof fn lemma_path_seen(g: Seq<Seq<usize>>, path: Seq<int>, from: int, j: int, seen: Seq<bool>)
    requires
        g.len() < usize::MAX,
        g_in_range(g),
        reach_closed(g, path[from], seen),
        0 <= from < j < path.len(),
        0 <= path[from] < g.len(),
        forall|a: int| 0 <= a < path.len() ==> 0 <= #[trigger] path[a] < g.len(),
        forall|a: int| 0 <= a < path.len() - 1 ==> g[path[a]].contains(#[trigger] path[a + 1] as usize),
    ensures
        seen[path[j]],
    decreases j - from,
{
    let prev = j - 1;
    assert(g[path[prev]].contains(path[prev + 1] as usize));
    let k = choose|k: int| 0 <= k < g[path[prev]].len() && g[path[prev]][k] == path[prev + 1] as usize;
    assert(g[path[prev]][k] == path[prev + 1] as usize);
    assert(0 <= path[j] < g.len());
    assert(path[prev + 1] == path[j]);
    assert(0 <= path[prev] < g.len());
    assert(g[path[prev]][k] as int == path[j]) by {
        assert(path[j] <= usize::MAX);
    }
    if prev == from {
        assert(seen[g[path[from]][k] as int]);
    } else {
        lemma_path_seen(g, path, from, prev, seen);
        assert(seen[g[path[prev]][k] as int]);
    }
}

/// A path of distinct unranked nodes can't be stuck, and can't get longer forever.
proof fn lemma_path(g: Seq<Seq<usize>>, r: Seq<Option<usize>>, path: Seq<int>)
    requires
        r.len() == g.len(),
        g.len() < usize::MAX,
        g_in_range(g),
        rank_bounded(r),
        rank_fixpoint(g, r),
        forall|x: int| 0 <= x < g.len() && (#[trigger] r[x]) is None ==> no_self_reach(g, x),
        path.len() > 0,
        path.no_duplicates(),
        forall|a: int| 0 <= a < path.len() ==> 0 <= #[trigger] path[a] < g.len() && r[path[a]] is None,
        forall|a: int| 0 <= a < path.len() - 1 ==> g[path[a]].contains(#[trigger] path[a + 1] as usize),
    ensures
        false,
    decreases g.len() - path.len(),
{
    let last = path[path.len() - 1];
    assert(0 <= last < g.len() && r[last] is None);
    let y = lemma_unranked_successor(g, r, last);
    if path.contains(y) {
        let i = choose|i: int| 0 <= i < path.len() && path[i] == y;
        assert(no_self_reach(g, y));
        let seen = choose|seen: Seq<bool>| #[trigger] reach_closed(g, y, seen) && !seen[y];
        let k = choose|k: int| 0 <= k < g[last].len() && g[last][k] == y as usize;
        assert(g[last][k] == y as usize);
        if i == path.len() - 1 {
            assert(seen[g[y][k] as int]);
        } else {
            lemma_path_seen(g, path, i, path.len() - 1, seen);
            assert(seen[g[last][k] as int]);
        }
        assert(false);
    } else {
        let next = path.push(y);
        assert forall|a: int, b: int| 0 <= a < next.len() && 0 <= b < next.len() && a != b implies next[a] != next[b] by {
            if a == path.len() as int {
                assert(next[a] == y);
                if next[b] == y {
                    assert(path[b] == y);
                    assert(path.contains(y));
                }
            } else if b == path.len() as int {
                assert(next[b] == y);
                if next[a] == y {
                    assert(path[a] == y);
                    assert(path.contains(y));
                }
            } else {
                assert(next[a] == path[a] && next[b] == path[b]);
            }
        }
        assert forall|a: int| 0 <= a < next.len() - 1 implies g[next[a]].contains(#[trigger] next[a + 1] as usize) by {
            if a < path.len() - 1 {
                assert(next[a] == path[a] && next[a + 1] == path[a + 1]);
                assert(g[path[a]].contains(path[a + 1] as usize));
            } else {
                assert(next[a] == last && next[a + 1] == y);
            }
        }
        assert forall|a: int| 0 <= a < next.len() implies 0 <= #[trigger] next[a] < g.len() && r[next[a]] is None by {
            if a < path.len() {
                assert(next[a] == path[a]);
            }
        }
        // A list of distinct nodes is no longer than the node table.
        assert(next.len() <= g.len()) by {
            let s = next.to_set();
            next.unique_seq_to_set();
            let all = set_int_range(0, g.len() as int);
            lemma_int_range(0, g.len() as int);
            assert(s.subset_of(all)) by {
                assert forall|v: int| s.contains(v) implies all.contains(v) by {
                    let a = choose|a: int| 0 <= a < next.len() && next[a] == v;
                    assert(0 <= next[a] < g.len());
                }
            }
            vstd::set_lib::lemma_len_subset(s, all);
        }
        lemma_path(g, r, next);
    }
}

/// **No node without a rank reaches itself ⟹ every node has a rank.**
pub proof fn lemma_all_ranked(g: Seq<Seq<usize>>)
    requires
        g.len() < usize::MAX,
        g_in_range(g),
        forall|x: int| 0 <= x < g.len() && (#[trigger] spec_node_ranks(g)[x]) is None ==> no_self_reach(g, x),
    ensures
        forall|x: int| 0 <= x < g.len() ==> (#[trigger] spec_node_ranks(g)[x]) is Some,
{
    let r = spec_node_ranks(g);
    lemma_node_ranks(g);
    assert forall|x: int| 0 <= x < g.len() implies (#[trigger] spec_node_ranks(g)[x]) is Some by {
        if r[x] is None {
            let path = seq![x];
            assert(path.no_duplicates());
            assert(path[0] == x);
            lemma_path(g, r, path);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The graph of a program
// ---------------------------------------------------------------------------------------------

proof fn lemma_push_if_lt_bound(out: Seq<usize>, base: usize, id: usize, n: usize, b: int)
    requires
        forall|k: int| 0 <= k < out.len() ==> (#[trigger] out[k]) < b,
        base + n <= b,
        base + n <= usize::MAX,
    ensures
        forall|k: int| 0 <= k < spec_push_if_lt(out, base, id, n).len() ==> (#[trigger] spec_push_if_lt(out, base, id, n)[k]) < b,
{
    let r = spec_push_if_lt(out, base, id, n);
    assert forall|k: int| 0 <= k < r.len() implies (#[trigger] r[k]) < b by {
        if k < out.len() {
            assert(r[k] == out[k]);
        }
    }
}

proof fn lemma_push_all_bound(out: Seq<usize>, base: usize, ids: Seq<usize>, n: usize, i: int, b: int)
    requires
        forall|k: int| 0 <= k < out.len() ==> (#[trigger] out[k]) < b,
        base + n <= b,
        base + n <= usize::MAX,
    ensures
        forall|k: int| 0 <= k < spec_push_all(out, base, ids, n, i).len() ==> (#[trigger] spec_push_all(out, base, ids, n, i)[k]) < b,
    decreases ids.len() - i,
{
    let r = spec_push_all(out, base, ids, n, i);
    if 0 <= i < ids.len() {
        lemma_push_if_lt_bound(out, base, ids[i], n, b);
        lemma_push_all_bound(spec_push_if_lt(out, base, ids[i], n), base, ids, n, i + 1, b);
    }
    assert forall|k: int| 0 <= k < r.len() implies (#[trigger] r[k]) < b by {
        if 0 <= i < ids.len() {
            let o = spec_push_if_lt(out, base, ids[i], n);
            assert(r == spec_push_all(o, base, ids, n, i + 1));
            assert(spec_push_all(o, base, ids, n, i + 1)[k] < b);
        }
    }
}

proof fn lemma_push_inputs_bound(out: Seq<usize>, base: usize, ids: Seq<InputId>, n: usize, i: int, b: int)
    requires
        forall|k: int| 0 <= k < out.len() ==> (#[trigger] out[k]) < b,
        base + n <= b,
        base + n <= usize::MAX,
    ensures
        forall|k: int| 0 <= k < spec_push_inputs(out, base, ids, n, i).len() ==> (#[trigger] spec_push_inputs(out, base, ids, n, i)[k]) < b,
    decreases ids.len() - i,
{
    let r = spec_push_inputs(out, base, ids, n, i);
    if 0 <= i < ids.len() {
        lemma_push_if_lt_bound(out, base, ids[i].0, n, b);
        lemma_push_inputs_bound(spec_push_if_lt(out, base, ids[i].0, n), base, ids, n, i + 1, b);
    }
    assert forall|k: int| 0 <= k < r.len() implies (#[trigger] r[k]) < b by {
        if 0 <= i < ids.len() {
            let o = spec_push_if_lt(out, base, ids[i].0, n);
            assert(r == spec_push_inputs(o, base, ids, n, i + 1));
            assert(spec_push_inputs(o, base, ids, n, i + 1)[k] < b);
        }
    }
}

proof fn lemma_push_consts_bound(out: Seq<usize>, base: usize, n: usize, c: int, b: int)
    requires
        forall|k: int| 0 <= k < out.len() ==> (#[trigger] out[k]) < b,
        base + n <= b,
        base + n <= usize::MAX,
    ensures
        forall|k: int| 0 <= k < spec_push_consts(out, base, n, c).len() ==> (#[trigger] spec_push_consts(out, base, n, c)[k]) < b,
    decreases n - c,
{
    let r = spec_push_consts(out, base, n, c);
    if 0 <= c < n {
        lemma_push_if_lt_bound(out, base, c as usize, n, b);
        lemma_push_consts_bound(spec_push_if_lt(out, base, c as usize, n), base, n, c + 1, b);
    }
    assert forall|k: int| 0 <= k < r.len() implies (#[trigger] r[k]) < b by {
        if 0 <= c < n {
            let o = spec_push_if_lt(out, base, c as usize, n);
            assert(r == spec_push_consts(o, base, n, c + 1));
            assert(spec_push_consts(o, base, n, c + 1)[k] < b);
        }
    }
}

/// With the node table fitting, every edge is to a real node.
pub proof fn lemma_graph_in_range(p: Program)
    requires
        spec_program_fits(p),
    ensures
        g_in_range(spec_graph(p)),
        spec_graph(p).len() == spec_node_count(p),
        spec_node_count(p) < usize::MAX,
{
    let g = spec_graph(p);
    let n = spec_node_count(p) as int;
    assert forall|x: int, i: int| #![trigger g[x][i]] 0 <= x < g.len() && 0 <= i < g[x].len() implies g[x][i] < g.len() by {
        let (nf, nc, ni) = (p.functions@.len() as int, p.consts@.len() as int, p.inputs@.len() as int);
        let e: Seq<usize> = Seq::empty();
        assert(g[x] == spec_succ(p, x));
        lemma_edges_bound(p, x, n);
    }
}

proof fn lemma_edges_bound(p: Program, x: int, b: int)
    requires
        spec_program_fits(p),
        b == spec_node_count(p),
        0 <= x < b,
    ensures
        forall|k: int| 0 <= k < spec_succ(p, x).len() ==> (#[trigger] spec_succ(p, x)[k]) < b,
{
    let (nf, nc, ni) = (p.functions@.len() as int, p.consts@.len() as int, p.inputs@.len() as int);
    let e: Seq<usize> = Seq::empty();
    if x < nf + nc {
        let r = if x < nf { spec_refs_stmts(p.functions@[x].body, 0, spec_empty_refs()) } else { spec_refs_expr(p.consts@[x - nf].init, spec_empty_refs()) };
        lemma_ref_edges_bound(p, r, b);
    } else if x < nf + nc + ni {
        lemma_push_if_lt_bound(e, spec_bu(p), p.inputs@[x - nf - nc].source.0, spec_nu(p), b);
    } else {
        let u = p.units@[x - nf - nc - ni];
        let o1 = spec_push_if_lt(e, 0, u.main.0, spec_nf(p));
        lemma_push_if_lt_bound(e, 0, u.main.0, spec_nf(p), b);
        lemma_push_inputs_bound(o1, spec_bi(p), u.inputs@, spec_ni(p), 0, b);
        lemma_push_consts_bound(spec_push_inputs(o1, spec_bi(p), u.inputs@, spec_ni(p), 0), spec_nf(p), spec_nc(p), 0, b);
    }
}

proof fn lemma_ref_edges_bound(p: Program, r: SRefs, b: int)
    requires
        spec_program_fits(p),
        b == spec_node_count(p),
    ensures
        forall|k: int| 0 <= k < spec_ref_edges(p, r).len() ==> (#[trigger] spec_ref_edges(p, r)[k]) < b,
{
    let e: Seq<usize> = Seq::empty();
    lemma_push_all_bound(e, 0, r.fns, spec_nf(p), 0, b);
    let o1 = spec_push_all(e, 0, r.fns, spec_nf(p), 0);
    lemma_push_all_bound(o1, spec_nf(p), r.consts, spec_nc(p), 0, b);
    let o2 = spec_push_all(o1, spec_nf(p), r.consts, spec_nc(p), 0);
    lemma_push_all_bound(o2, spec_bi(p), r.inputs, spec_ni(p), 0, b);
}

// What pushing keeps: earlier elements stay, and every in-range id arrives.

proof fn lemma_push_if_lt_keeps(out: Seq<usize>, base: usize, id: usize, n: usize, v: usize)
    requires
        out.contains(v),
    ensures
        spec_push_if_lt(out, base, id, n).contains(v),
{
    if id < n {
        let k = choose|k: int| 0 <= k < out.len() && out[k] == v;
        assert(out.push(spec_add_or_max(base, id))[k] == v);
    }
}

proof fn lemma_push_all_keeps(out: Seq<usize>, base: usize, ids: Seq<usize>, n: usize, i: int, v: usize)
    requires
        out.contains(v),
    ensures
        spec_push_all(out, base, ids, n, i).contains(v),
    decreases ids.len() - i,
{
    if 0 <= i < ids.len() {
        lemma_push_if_lt_keeps(out, base, ids[i], n, v);
        lemma_push_all_keeps(spec_push_if_lt(out, base, ids[i], n), base, ids, n, i + 1, v);
    }
}

proof fn lemma_push_all_has(out: Seq<usize>, base: usize, ids: Seq<usize>, n: usize, i: int, j: int)
    requires
        0 <= i <= j < ids.len(),
        ids[j] < n,
    ensures
        spec_push_all(out, base, ids, n, i).contains(spec_add_or_max(base, ids[j])),
    decreases ids.len() - i,
{
    let v = spec_add_or_max(base, ids[j]);
    let o1 = spec_push_if_lt(out, base, ids[i], n);
    if i == j {
        assert(o1[o1.len() - 1] == v);
        assert(o1.contains(v));
        lemma_push_all_keeps(o1, base, ids, n, i + 1, v);
    } else {
        lemma_push_all_has(o1, base, ids, n, i + 1, j);
    }
}

proof fn lemma_push_inputs_keeps(out: Seq<usize>, base: usize, ids: Seq<InputId>, n: usize, i: int, v: usize)
    requires
        out.contains(v),
    ensures
        spec_push_inputs(out, base, ids, n, i).contains(v),
    decreases ids.len() - i,
{
    if 0 <= i < ids.len() {
        lemma_push_if_lt_keeps(out, base, ids[i].0, n, v);
        lemma_push_inputs_keeps(spec_push_if_lt(out, base, ids[i].0, n), base, ids, n, i + 1, v);
    }
}

proof fn lemma_push_inputs_has(out: Seq<usize>, base: usize, ids: Seq<InputId>, n: usize, i: int, j: int)
    requires
        0 <= i <= j < ids.len(),
        ids[j].0 < n,
    ensures
        spec_push_inputs(out, base, ids, n, i).contains(spec_add_or_max(base, ids[j].0)),
    decreases ids.len() - i,
{
    let v = spec_add_or_max(base, ids[j].0);
    let o1 = spec_push_if_lt(out, base, ids[i].0, n);
    if i == j {
        assert(o1[o1.len() - 1] == v);
        assert(o1.contains(v));
        lemma_push_inputs_keeps(o1, base, ids, n, i + 1, v);
    } else {
        lemma_push_inputs_has(o1, base, ids, n, i + 1, j);
    }
}

proof fn lemma_push_consts_keeps(out: Seq<usize>, base: usize, n: usize, c: int, v: usize)
    requires
        out.contains(v),
    ensures
        spec_push_consts(out, base, n, c).contains(v),
    decreases n - c,
{
    if 0 <= c < n {
        lemma_push_if_lt_keeps(out, base, c as usize, n, v);
        lemma_push_consts_keeps(spec_push_if_lt(out, base, c as usize, n), base, n, c + 1, v);
    }
}

// From the references in a body to "ranked below".

/// Every in-range id in `r` is ranked below `q`.
pub open spec fn refs_below(ranks: SRanks, q: usize, r: SRefs) -> bool {
    &&& forall|k: int| 0 <= k < r.fns.len() && (#[trigger] r.fns[k]) < ranks.fns.len() ==> spec_below(ranks.fns, r.fns[k], q)
    &&& forall|k: int| 0 <= k < r.consts.len() && (#[trigger] r.consts[k]) < ranks.consts.len() ==> spec_below(ranks.consts, r.consts[k], q)
    &&& forall|k: int| 0 <= k < r.inputs.len() && (#[trigger] r.inputs[k]) < ranks.inputs.len() ==> spec_below(ranks.inputs, r.inputs[k], q)
}

proof fn lemma_exprs_ranked(p: &Program, n: nat, ranks: SRanks, q: usize, es: Vec<Expr>, i: int, r: SRefs)
    requires
        0 <= i,
        forall|k: int| #![trigger es@[k]] i <= k < es@.len() ==> wf::wf_expr(p, n, &es@[k]),
        ranks.fns.len() == p.functions@.len(),
        ranks.consts.len() == p.consts@.len(),
        ranks.inputs.len() == p.inputs@.len(),
        refs_below(ranks, q, spec_refs_exprs(es, i, r)),
    ensures
        refs_below(ranks, q, r),
        forall|k: int| #![trigger es@[k]] i <= k < es@.len() ==> spec_expr_ranked(ranks, q, es@[k]),
    decreases es, es@.len() - i,
{
    if i < es@.len() {
        lemma_exprs_ranked(p, n, ranks, q, es, i + 1, spec_refs_expr(es@[i], r));
        lemma_expr_ranked(p, n, ranks, q, es@[i], r);
    }
}

proof fn lemma_fields_ranked(p: &Program, n: nat, ranks: SRanks, q: usize, fs: Vec<FieldInit>, i: int, r: SRefs)
    requires
        0 <= i,
        forall|k: int| #![trigger fs@[k]] i <= k < fs@.len() ==> wf::wf_expr(p, n, &fs@[k].value),
        ranks.fns.len() == p.functions@.len(),
        ranks.consts.len() == p.consts@.len(),
        ranks.inputs.len() == p.inputs@.len(),
        refs_below(ranks, q, spec_refs_fields(fs, i, r)),
    ensures
        refs_below(ranks, q, r),
        forall|k: int| #![trigger fs@[k]] i <= k < fs@.len() ==> spec_expr_ranked(ranks, q, fs@[k].value),
    decreases fs, fs@.len() - i,
{
    if i < fs@.len() {
        lemma_fields_ranked(p, n, ranks, q, fs, i + 1, spec_refs_expr(fs@[i].value, r));
        lemma_expr_ranked(p, n, ranks, q, fs@[i].value, r);
    }
}

proof fn lemma_refs_prefix_fns(ranks: SRanks, q: usize, r: SRefs, id: usize)
    requires
        refs_below(ranks, q, SRefs { fns: r.fns.push(id), ..r }),
    ensures
        refs_below(ranks, q, r),
        id < ranks.fns.len() ==> spec_below(ranks.fns, id, q),
{
    let r2 = SRefs { fns: r.fns.push(id), ..r };
    assert(r2.fns[r.fns.len() as int] == id);
    assert(r2.consts == r.consts);
    assert forall|k: int| 0 <= k < r.consts.len() && (#[trigger] r.consts[k]) < ranks.consts.len() implies spec_below(ranks.consts, r.consts[k], q) by {
        assert(r2.consts[k] == r.consts[k]);
    }
    assert(r2.inputs == r.inputs);
    assert forall|k: int| 0 <= k < r.inputs.len() && (#[trigger] r.inputs[k]) < ranks.inputs.len() implies spec_below(ranks.inputs, r.inputs[k], q) by {
        assert(r2.inputs[k] == r.inputs[k]);
    }
    assert forall|k: int| 0 <= k < r.fns.len() && (#[trigger] r.fns[k]) < ranks.fns.len() implies spec_below(ranks.fns, r.fns[k], q) by {
        assert(r2.fns[k] == r.fns[k]);
    }
}

proof fn lemma_refs_prefix_consts(ranks: SRanks, q: usize, r: SRefs, id: usize)
    requires
        refs_below(ranks, q, SRefs { consts: r.consts.push(id), ..r }),
    ensures
        refs_below(ranks, q, r),
        id < ranks.consts.len() ==> spec_below(ranks.consts, id, q),
{
    let r2 = SRefs { consts: r.consts.push(id), ..r };
    assert(r2.consts[r.consts.len() as int] == id);
    assert(r2.fns == r.fns);
    assert forall|k: int| 0 <= k < r.fns.len() && (#[trigger] r.fns[k]) < ranks.fns.len() implies spec_below(ranks.fns, r.fns[k], q) by {
        assert(r2.fns[k] == r.fns[k]);
    }
    assert(r2.inputs == r.inputs);
    assert forall|k: int| 0 <= k < r.inputs.len() && (#[trigger] r.inputs[k]) < ranks.inputs.len() implies spec_below(ranks.inputs, r.inputs[k], q) by {
        assert(r2.inputs[k] == r.inputs[k]);
    }
    assert forall|k: int| 0 <= k < r.consts.len() && (#[trigger] r.consts[k]) < ranks.consts.len() implies spec_below(ranks.consts, r.consts[k], q) by {
        assert(r2.consts[k] == r.consts[k]);
    }
}

proof fn lemma_refs_prefix_inputs(ranks: SRanks, q: usize, r: SRefs, id: usize)
    requires
        refs_below(ranks, q, SRefs { inputs: r.inputs.push(id), ..r }),
    ensures
        refs_below(ranks, q, r),
        id < ranks.inputs.len() ==> spec_below(ranks.inputs, id, q),
{
    let r2 = SRefs { inputs: r.inputs.push(id), ..r };
    assert(r2.inputs[r.inputs.len() as int] == id);
    assert(r2.fns == r.fns);
    assert forall|k: int| 0 <= k < r.fns.len() && (#[trigger] r.fns[k]) < ranks.fns.len() implies spec_below(ranks.fns, r.fns[k], q) by {
        assert(r2.fns[k] == r.fns[k]);
    }
    assert(r2.consts == r.consts);
    assert forall|k: int| 0 <= k < r.consts.len() && (#[trigger] r.consts[k]) < ranks.consts.len() implies spec_below(ranks.consts, r.consts[k], q) by {
        assert(r2.consts[k] == r.consts[k]);
    }
    assert forall|k: int| 0 <= k < r.inputs.len() && (#[trigger] r.inputs[k]) < ranks.inputs.len() implies spec_below(ranks.inputs, r.inputs[k], q) by {
        assert(r2.inputs[k] == r.inputs[k]);
    }
}

pub proof fn lemma_expr_ranked(p: &Program, n: nat, ranks: SRanks, q: usize, e: Expr, r: SRefs)
    requires
        wf::wf_expr(p, n, &e),
        ranks.fns.len() == p.functions@.len(),
        ranks.consts.len() == p.consts@.len(),
        ranks.inputs.len() == p.inputs@.len(),
        refs_below(ranks, q, spec_refs_expr(e, r)),
    ensures
        refs_below(ranks, q, r),
        spec_expr_ranked(ranks, q, e),
    decreases e,
{
    match e {
        Expr::Const(c) => lemma_refs_prefix_consts(ranks, q, r, c.0),
        Expr::Density { input, index } => {
            let r1 = SRefs { inputs: r.inputs.push(input.0), ..r };
            match index {
                Some(ix) => lemma_expr_ranked(p, n, ranks, q, *ix, r1),
                None => {},
            }
            lemma_refs_prefix_inputs(ranks, q, r, input.0);
        },
        Expr::Call { func, args } => {
            let r1 = SRefs { fns: r.fns.push(func.0), ..r };
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies wf::wf_expr(p, n, &args@[k]) by {
                assert(wf::wf_expr(p, n, &args[k]));
            }
            lemma_exprs_ranked(p, n, ranks, q, args, 0, r1);
            lemma_refs_prefix_fns(ranks, q, r, func.0);
        },
        Expr::ExternCall { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies wf::wf_expr(p, n, &args@[k]) by {
                assert(wf::wf_expr(p, n, &args[k]));
            }
            lemma_exprs_ranked(p, n, ranks, q, args, 0, r);
        },
        Expr::Binary { left, right, .. } => {
            lemma_expr_ranked(p, n, ranks, q, *right, spec_refs_expr(*left, r));
            lemma_expr_ranked(p, n, ranks, q, *left, r);
        },
        Expr::Unary { operand, .. } => lemma_expr_ranked(p, n, ranks, q, *operand, r),
        Expr::Cast { operand, .. } => lemma_expr_ranked(p, n, ranks, q, *operand, r),
        Expr::Field { base, .. } => lemma_expr_ranked(p, n, ranks, q, *base, r),
        Expr::Index { array, index } => {
            lemma_expr_ranked(p, n, ranks, q, *index, spec_refs_expr(*array, r));
            lemma_expr_ranked(p, n, ranks, q, *array, r);
        },
        Expr::MakeVec3 { x, y, z } => {
            lemma_expr_ranked(p, n, ranks, q, *z, spec_refs_expr(*y, spec_refs_expr(*x, r)));
            lemma_expr_ranked(p, n, ranks, q, *y, spec_refs_expr(*x, r));
            lemma_expr_ranked(p, n, ranks, q, *x, r);
        },
        Expr::MakePos3 { x, y, z } => {
            lemma_expr_ranked(p, n, ranks, q, *z, spec_refs_expr(*y, spec_refs_expr(*x, r)));
            lemma_expr_ranked(p, n, ranks, q, *y, spec_refs_expr(*x, r));
            lemma_expr_ranked(p, n, ranks, q, *x, r);
        },
        Expr::MakeRecord { fields, .. } => {
            assert forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() implies wf::wf_expr(p, n, &fields@[k].value) by {
                assert(wf::wf_expr(p, n, &fields[k].value));
            }
            lemma_fields_ranked(p, n, ranks, q, fields, 0, r);
        },
        Expr::ArrayLit(es) => {
            assert forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() implies wf::wf_expr(p, n, &es@[k]) by {
                assert(wf::wf_expr(p, n, &es[k]));
            }
            lemma_exprs_ranked(p, n, ranks, q, es, 0, r);
        },
        _ => {},
    }
}

proof fn lemma_stmts_ranked(p: &Program, n: nat, ranks: SRanks, q: usize, ss: Vec<Stmt>, i: int, r: SRefs)
    requires
        0 <= i,
        forall|k: int| #![trigger ss@[k]] i <= k < ss@.len() ==> wf::wf_stmt(p, n, &ss@[k]),
        ranks.fns.len() == p.functions@.len(),
        ranks.consts.len() == p.consts@.len(),
        ranks.inputs.len() == p.inputs@.len(),
        refs_below(ranks, q, spec_refs_stmts(ss, i, r)),
    ensures
        refs_below(ranks, q, r),
        forall|k: int| #![trigger ss@[k]] i <= k < ss@.len() ==> spec_stmt_ranked(ranks, q, ss@[k]),
    decreases ss, ss@.len() - i,
{
    if i < ss@.len() {
        lemma_stmts_ranked(p, n, ranks, q, ss, i + 1, spec_refs_stmt(ss@[i], r));
        lemma_stmt_ranked(p, n, ranks, q, ss@[i], r);
    }
}

proof fn lemma_stmt_ranked(p: &Program, n: nat, ranks: SRanks, q: usize, s: Stmt, r: SRefs)
    requires
        wf::wf_stmt(p, n, &s),
        ranks.fns.len() == p.functions@.len(),
        ranks.consts.len() == p.consts@.len(),
        ranks.inputs.len() == p.inputs@.len(),
        refs_below(ranks, q, spec_refs_stmt(s, r)),
    ensures
        refs_below(ranks, q, r),
        spec_stmt_ranked(ranks, q, s),
    decreases s,
{
    match s {
        Stmt::Assign { value, .. } => lemma_expr_ranked(p, n, ranks, q, value, r),
        Stmt::Return(e) => lemma_expr_ranked(p, n, ranks, q, e, r),
        Stmt::If { cond, then_branch, else_branch } => {
            let r1 = spec_refs_expr(cond, r);
            let r2 = spec_refs_stmts(then_branch, 0, r1);
            assert forall|k: int| #![trigger then_branch@[k]] 0 <= k < then_branch@.len() implies wf::wf_stmt(p, n, &then_branch@[k]) by {
                assert(wf::wf_stmt(p, n, &then_branch[k]));
            }
            assert forall|k: int| #![trigger else_branch@[k]] 0 <= k < else_branch@.len() implies wf::wf_stmt(p, n, &else_branch@[k]) by {
                assert(wf::wf_stmt(p, n, &else_branch[k]));
            }
            lemma_stmts_ranked(p, n, ranks, q, else_branch, 0, r2);
            lemma_stmts_ranked(p, n, ranks, q, then_branch, 0, r1);
            lemma_expr_ranked(p, n, ranks, q, cond, r);
        },
        Stmt::While { cond, body } => {
            let r1 = spec_refs_expr(cond, r);
            assert forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() implies wf::wf_stmt(p, n, &body@[k]) by {
                assert(wf::wf_stmt(p, n, &body[k]));
            }
            lemma_stmts_ranked(p, n, ranks, q, body, 0, r1);
            lemma_expr_ranked(p, n, ranks, q, cond, r);
        },
        Stmt::Repeat { body, .. } => {
            assert forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() implies wf::wf_stmt(p, n, &body@[k]) by {
                assert(wf::wf_stmt(p, n, &body[k]));
            }
            lemma_stmts_ranked(p, n, ranks, q, body, 0, r);
        },
        Stmt::Break => {},
    }
}

/// An edge lands on a rank below the source's.
proof fn lemma_edge_below(g: Seq<Seq<usize>>, rk: Seq<Option<usize>>, x: int, y: usize)
    requires
        valid_ranks(g, rk),
        rk.len() == g.len(),
        0 <= x < g.len(),
        rk[x] is Some,
        g[x].contains(y),
    ensures
        spec_rank(rk, y) is Some,
        spec_rank(rk, y)->0 < rk[x]->0,
{
    let k = choose|k: int| 0 <= k < g[x].len() && g[x][k] == y;
    assert(g[x][k] == y);
}

proof fn lemma_slice(p: Program, rk: Seq<Option<usize>>, base: usize, n: usize, k: int)
    requires
        0 <= k < n,
        base + n <= usize::MAX,
    ensures
        spec_slice_ranks(rk, base, n)[k] == spec_rank(rk, (base + k) as usize),
{
}

/// The body-level edges: every in-range reference of node `x`'s `r` is ranked below it.
proof fn lemma_refs_below_node(p: Program, g: Seq<Seq<usize>>, rk: Seq<Option<usize>>, x: int, r: SRefs)
    requires
        spec_program_fits(p),
        g == spec_graph(p),
        rk == spec_node_ranks(g),
        valid_ranks(g, rk),
        rk.len() == g.len(),
        0 <= x < g.len(),
        rk[x] is Some,
        g[x] == spec_ref_edges(p, r),
    ensures
        refs_below(spec_ranks(p), rk[x]->0, r),
{
    let ranks = spec_ranks(p);
    let q = rk[x]->0;
    let e: Seq<usize> = Seq::empty();
    let o1 = spec_push_all(e, 0, r.fns, spec_nf(p), 0);
    let o2 = spec_push_all(o1, spec_nf(p), r.consts, spec_nc(p), 0);
    assert forall|k: int| 0 <= k < r.fns.len() && (#[trigger] r.fns[k]) < ranks.fns.len() implies spec_below(ranks.fns, r.fns[k], q) by {
        let id = r.fns[k];
        lemma_push_all_has(e, 0, r.fns, spec_nf(p), 0, k);
        let v = spec_add_or_max(0, id);
        lemma_push_all_keeps(o1, spec_nf(p), r.consts, spec_nc(p), 0, v);
        lemma_push_all_keeps(o2, spec_bi(p), r.inputs, spec_ni(p), 0, v);
        lemma_edge_below(g, rk, x, v);
    }
    assert forall|k: int| 0 <= k < r.consts.len() && (#[trigger] r.consts[k]) < ranks.consts.len() implies spec_below(ranks.consts, r.consts[k], q) by {
        let id = r.consts[k];
        lemma_push_all_has(o1, spec_nf(p), r.consts, spec_nc(p), 0, k);
        let v = spec_add_or_max(spec_nf(p), id);
        lemma_push_all_keeps(o2, spec_bi(p), r.inputs, spec_ni(p), 0, v);
        lemma_edge_below(g, rk, x, v);
    }
    assert forall|k: int| 0 <= k < r.inputs.len() && (#[trigger] r.inputs[k]) < ranks.inputs.len() implies spec_below(ranks.inputs, r.inputs[k], q) by {
        let id = r.inputs[k];
        lemma_push_all_has(o2, spec_bi(p), r.inputs, spec_ni(p), 0, k);
        let v = spec_add_or_max(spec_bi(p), id);
        lemma_edge_below(g, rk, x, v);
    }
}

/// **The acyclicity theorem**: a wf program with no cycle (no unranked node reaches itself) has
/// valid ranks.
pub proof fn lemma_ranks_valid(p: Program)
    requires
        wf::wf_program(&p),
        spec_program_fits(p),
        forall|x: int| 0 <= x < spec_graph(p).len() && (#[trigger] spec_node_ranks(spec_graph(p))[x]) is None ==> no_self_reach(spec_graph(p), x),
    ensures
        spec_ranks_valid(p, spec_ranks(p)),
{
    let g = spec_graph(p);
    lemma_graph_in_range(p);
    lemma_node_ranks(g);
    lemma_all_ranked(g);
    let rk = spec_node_ranks(g);
    let ranks = spec_ranks(p);
    let (nf, nc, ni, nu) = (p.functions@.len() as int, p.consts@.len() as int, p.inputs@.len() as int, p.units@.len() as int);
    let e: Seq<usize> = Seq::empty();
    lemma_count_le(rk, rk.len() as int);
    assert forall|k: int| #![trigger ranks.fns[k]] 0 <= k < ranks.fns.len() implies ranks.fns[k] is Some && forall|j: int| #![trigger p.functions@[k].body@[j]]
        0 <= j < p.functions@[k].body@.len() ==> spec_stmt_ranked(ranks, ranks.fns[k]->0, p.functions@[k].body@[j]) by {
        assert(rk[k] is Some);
        assert(ranks.fns[k] == rk[k]);
        let f = p.functions@[k];
        assert(wf::wf_function(&p, &p.functions[k]));
        assert forall|j: int| #![trigger f.body@[j]] 0 <= j < f.body@.len() implies wf::wf_stmt(&p, f.locals@.len() as nat, &f.body@[j]) by {
            assert(wf::wf_stmt(&p, f.locals@.len() as nat, &f.body[j]));
        }
        let r = spec_refs_stmts(f.body, 0, spec_empty_refs());
        assert(g[k] == spec_succ(p, k));
        lemma_refs_below_node(p, g, rk, k, r);
        lemma_stmts_ranked(&p, f.locals@.len() as nat, ranks, rk[k]->0, f.body, 0, spec_empty_refs());
    }
    assert forall|k: int| #![trigger ranks.consts[k]] 0 <= k < ranks.consts.len() implies ranks.consts[k] is Some
        && ranks.consts[k]->0 < usize::MAX
        && spec_expr_ranked(ranks, ranks.consts[k]->0, p.consts@[k].init) by {
        let x = nf + k;
        assert(rk[x] is Some);
        assert(ranks.consts[k] == rk[x]);
        assert(rk[x]->0 < count_some(rk, rk.len() as int));
        assert(wf::wf_expr(&p, 0, &p.consts[k].init));
        let r = spec_refs_expr(p.consts@[k].init, spec_empty_refs());
        assert(g[x] == spec_succ(p, x));
        lemma_refs_below_node(p, g, rk, x, r);
        lemma_expr_ranked(&p, 0, ranks, rk[x]->0, p.consts@[k].init, spec_empty_refs());
    }
    assert forall|k: int| #![trigger ranks.inputs[k]] 0 <= k < ranks.inputs.len() implies ranks.inputs[k] is Some
        && spec_below(ranks.units, p.inputs@[k].source.0, ranks.inputs[k]->0) by {
        let x = nf + nc + k;
        assert(rk[x] is Some);
        assert(ranks.inputs[k] == rk[x]);
        assert(wf::wf_input(&p, &p.inputs[k]));
        let src = p.inputs@[k].source.0;
        assert(g[x] == spec_succ(p, x));
        let v = spec_add_or_max(spec_bu(p), src);
        assert(g[x][0] == v);
        assert(g[x].contains(v));
        lemma_edge_below(g, rk, x, v);
    }
    assert forall|k: int| #![trigger ranks.units[k]] 0 <= k < ranks.units.len() implies ranks.units[k] is Some && {
        &&& spec_below(ranks.fns, p.units@[k].main.0, ranks.units[k]->0)
        &&& forall|j: int| #![trigger p.units@[k].inputs@[j]] 0 <= j < p.units@[k].inputs@.len() ==> spec_below(ranks.inputs, p.units@[k].inputs@[j].0, ranks.units[k]->0)
    } by {
        let x = nf + nc + ni + k;
        assert(rk[x] is Some);
        assert(ranks.units[k] == rk[x]);
        let u = p.units@[k];
        assert(wf::wf_unit(&p, &p.units[k]));
        assert(g[x] == spec_succ(p, x));
        let o1 = spec_push_if_lt(e, 0, u.main.0, spec_nf(p));
        let o2 = spec_push_inputs(o1, spec_bi(p), u.inputs@, spec_ni(p), 0);
        let vm = spec_add_or_max(0, u.main.0);
        assert(o1[0] == vm);
        assert(o1.contains(vm));
        lemma_push_inputs_keeps(o1, spec_bi(p), u.inputs@, spec_ni(p), 0, vm);
        lemma_push_consts_keeps(o2, spec_nf(p), spec_nc(p), 0, vm);
        lemma_edge_below(g, rk, x, vm);
        assert forall|j: int| #![trigger p.units@[k].inputs@[j]] 0 <= j < p.units@[k].inputs@.len() implies spec_below(ranks.inputs, p.units@[k].inputs@[j].0, ranks.units[k]->0) by {
            assert(u.inputs[j].0 < p.inputs.len());
            let vi = spec_add_or_max(spec_bi(p), u.inputs@[j].0);
            lemma_push_inputs_has(o1, spec_bi(p), u.inputs@, spec_ni(p), 0, j);
            lemma_push_consts_keeps(o2, spec_nf(p), spec_nc(p), 0, vi);
            lemma_edge_below(g, rk, x, vi);
        }
    }
}

} // verus!
