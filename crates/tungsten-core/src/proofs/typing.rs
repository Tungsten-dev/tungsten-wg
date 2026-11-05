//! Facts about typing that don't depend on any particular program.

use vstd::prelude::*;

use crate::ir::*;
use crate::spec::eval::*;
use crate::spec::typing::*;
use crate::spec::value::*;
use crate::wf;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// `c2` knows everything `c1` knows, the same way.
pub open spec fn extends(c1: Seq<Option<Type>>, c2: Seq<Option<Type>>) -> bool {
    c1.len() == c2.len() && forall|d: int| 0 <= d < c1.len() && (#[trigger] c1[d]) is Some ==> c2[d] == c1[d]
}

/// Typing with more const types known gives the same result.
pub proof fn lemma_type_mono(p: Program, locals: Seq<Type>, c1: Seq<Option<Type>>, c2: Seq<Option<Type>>, e: Expr)
    requires
        extends(c1, c2),
        spec_type_of(p, locals, c1, e) is Some,
    ensures
        spec_type_of(p, locals, c2, e) == spec_type_of(p, locals, c1, e),
    decreases e,
{
    match e {
        Expr::Density { index, .. } => {
            match index {
                Some(ix) => lemma_type_mono(p, locals, c1, c2, *ix),
                None => {},
            }
        },
        Expr::Call { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies spec_type_of(p, locals, c2, args@[k]) == spec_type_of(p, locals, c1, args@[k]) by {
                lemma_type_mono(p, locals, c1, c2, args@[k]);
            }
        },
        Expr::ExternCall { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies spec_type_of(p, locals, c2, args@[k]) == spec_type_of(p, locals, c1, args@[k]) by {
                lemma_type_mono(p, locals, c1, c2, args@[k]);
            }
        },
        Expr::Binary { left, right, .. } => {
            lemma_type_mono(p, locals, c1, c2, *left);
            lemma_type_mono(p, locals, c1, c2, *right);
        },
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => lemma_type_mono(p, locals, c1, c2, *operand),
        Expr::Field { base, .. } => lemma_type_mono(p, locals, c1, c2, *base),
        Expr::Index { array, index } => {
            lemma_type_mono(p, locals, c1, c2, *array);
            lemma_type_mono(p, locals, c1, c2, *index);
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            lemma_type_mono(p, locals, c1, c2, *x);
            lemma_type_mono(p, locals, c1, c2, *y);
            lemma_type_mono(p, locals, c1, c2, *z);
        },
        Expr::MakeRecord { fields, .. } => {
            assert forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() implies spec_type_of(p, locals, c2, fields@[k].value) == spec_type_of(p, locals, c1, fields@[k].value) by {
                lemma_type_mono(p, locals, c1, c2, fields@[k].value);
            }
        },
        Expr::ArrayLit(es) => {
            assert forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() implies spec_type_of(p, locals, c2, es@[k]) == spec_type_of(p, locals, c1, es@[k]) by {
                lemma_type_mono(p, locals, c1, c2, es@[k]);
            }
        },
        _ => {},
    }
}

/// An expression ranked below `r` only reads consts below `r`, which all have types.
pub proof fn lemma_known_from_ranked(ranks: SRanks, ctys: Seq<Option<Type>>, r: usize, e: Expr)
    requires
        spec_expr_ranked(ranks, r, e),
        ctys.len() == ranks.consts.len(),
        forall|d: int| #![trigger ranks.consts[d]] 0 <= d < ranks.consts.len() && ranks.consts[d] is Some && (ranks.consts[d]->0 as int) < r ==> ctys[d] is Some,
    ensures
        spec_reads_known(ctys, e),
    decreases e,
{
    match e {
        Expr::Const(c) => {
            assert(ranks.consts[c.0 as int] is Some);
        },
        Expr::Density { index, .. } => {
            match index {
                Some(ix) => lemma_known_from_ranked(ranks, ctys, r, *ix),
                None => {},
            }
        },
        Expr::Call { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies spec_reads_known(ctys, args@[k]) by {
                lemma_known_from_ranked(ranks, ctys, r, args@[k]);
            }
        },
        Expr::ExternCall { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies spec_reads_known(ctys, args@[k]) by {
                lemma_known_from_ranked(ranks, ctys, r, args@[k]);
            }
        },
        Expr::Binary { left, right, .. } => {
            lemma_known_from_ranked(ranks, ctys, r, *left);
            lemma_known_from_ranked(ranks, ctys, r, *right);
        },
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => lemma_known_from_ranked(ranks, ctys, r, *operand),
        Expr::Field { base, .. } => lemma_known_from_ranked(ranks, ctys, r, *base),
        Expr::Index { array, index } => {
            lemma_known_from_ranked(ranks, ctys, r, *array);
            lemma_known_from_ranked(ranks, ctys, r, *index);
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            lemma_known_from_ranked(ranks, ctys, r, *x);
            lemma_known_from_ranked(ranks, ctys, r, *y);
            lemma_known_from_ranked(ranks, ctys, r, *z);
        },
        Expr::MakeRecord { fields, .. } => {
            assert forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() implies spec_reads_known(ctys, fields@[k].value) by {
                lemma_known_from_ranked(ranks, ctys, r, fields@[k].value);
            }
        },
        Expr::ArrayLit(es) => {
            assert forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() implies spec_reads_known(ctys, es@[k]) by {
                lemma_known_from_ranked(ranks, ctys, r, es@[k]);
            }
        },
        _ => {},
    }
}

/// With every const typed, every read in a wf expression is known.
pub proof fn lemma_expr_known_all(p: &Program, n: nat, ctys: Seq<Option<Type>>, e: Expr)
    requires
        wf::wf_expr(p, n, &e),
        ctys.len() == p.consts@.len(),
        forall|c: int| 0 <= c < ctys.len() ==> #[trigger] ctys[c] is Some,
    ensures
        spec_reads_known(ctys, e),
    decreases e,
{
    match e {
        Expr::Density { index, .. } => {
            match index {
                Some(ix) => lemma_expr_known_all(p, n, ctys, *ix),
                None => {},
            }
        },
        Expr::Call { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies spec_reads_known(ctys, args@[k]) by {
                assert(wf::wf_expr(p, n, &args[k]));
                lemma_expr_known_all(p, n, ctys, args@[k]);
            }
        },
        Expr::ExternCall { args, .. } => {
            assert forall|k: int| #![trigger args@[k]] 0 <= k < args@.len() implies spec_reads_known(ctys, args@[k]) by {
                assert(wf::wf_expr(p, n, &args[k]));
                lemma_expr_known_all(p, n, ctys, args@[k]);
            }
        },
        Expr::Binary { left, right, .. } => {
            lemma_expr_known_all(p, n, ctys, *left);
            lemma_expr_known_all(p, n, ctys, *right);
        },
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => lemma_expr_known_all(p, n, ctys, *operand),
        Expr::Field { base, .. } => lemma_expr_known_all(p, n, ctys, *base),
        Expr::Index { array, index } => {
            lemma_expr_known_all(p, n, ctys, *array);
            lemma_expr_known_all(p, n, ctys, *index);
        },
        Expr::MakeVec3 { x, y, z } | Expr::MakePos3 { x, y, z } => {
            lemma_expr_known_all(p, n, ctys, *x);
            lemma_expr_known_all(p, n, ctys, *y);
            lemma_expr_known_all(p, n, ctys, *z);
        },
        Expr::MakeRecord { fields, .. } => {
            assert forall|k: int| #![trigger fields@[k]] 0 <= k < fields@.len() implies spec_reads_known(ctys, fields@[k].value) by {
                assert(wf::wf_expr(p, n, &fields[k].value));
                lemma_expr_known_all(p, n, ctys, fields@[k].value);
            }
        },
        Expr::ArrayLit(es) => {
            assert forall|k: int| #![trigger es@[k]] 0 <= k < es@.len() implies spec_reads_known(ctys, es@[k]) by {
                assert(wf::wf_expr(p, n, &es[k]));
                lemma_expr_known_all(p, n, ctys, es@[k]);
            }
        },
        _ => {},
    }
}

pub proof fn lemma_stmt_known_all(p: &Program, n: nat, ctys: Seq<Option<Type>>, s: Stmt)
    requires
        wf::wf_stmt(p, n, &s),
        ctys.len() == p.consts@.len(),
        forall|c: int| 0 <= c < ctys.len() ==> #[trigger] ctys[c] is Some,
    ensures
        spec_stmt_reads_known(ctys, s),
    decreases s,
{
    match s {
        Stmt::Assign { value, .. } => lemma_expr_known_all(p, n, ctys, value),
        Stmt::Return(e) => lemma_expr_known_all(p, n, ctys, e),
        Stmt::If { cond, then_branch, else_branch } => {
            lemma_expr_known_all(p, n, ctys, cond);
            assert forall|k: int| #![trigger then_branch@[k]] 0 <= k < then_branch@.len() implies spec_stmt_reads_known(ctys, then_branch@[k]) by {
                assert(wf::wf_stmt(p, n, &then_branch[k]));
                lemma_stmt_known_all(p, n, ctys, then_branch@[k]);
            }
            assert forall|k: int| #![trigger else_branch@[k]] 0 <= k < else_branch@.len() implies spec_stmt_reads_known(ctys, else_branch@[k]) by {
                assert(wf::wf_stmt(p, n, &else_branch[k]));
                lemma_stmt_known_all(p, n, ctys, else_branch@[k]);
            }
        },
        Stmt::While { cond, body } => {
            lemma_expr_known_all(p, n, ctys, cond);
            assert forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() implies spec_stmt_reads_known(ctys, body@[k]) by {
                assert(wf::wf_stmt(p, n, &body[k]));
                lemma_stmt_known_all(p, n, ctys, body@[k]);
            }
        },
        Stmt::Repeat { body, .. } => {
            assert forall|k: int| #![trigger body@[k]] 0 <= k < body@.len() implies spec_stmt_reads_known(ctys, body@[k]) by {
                assert(wf::wf_stmt(p, n, &body[k]));
                lemma_stmt_known_all(p, n, ctys, body@[k]);
            }
        },
        Stmt::Break => {},
    }
}

// ---------------------------------------------------------------------------------------------
// Deriving const types in rank order
// ---------------------------------------------------------------------------------------------

/// While typing consts in rank order: those before `(r, c)` have their type, the others have none.
#[verifier::opaque]
pub open spec fn const_stage(p: Program, ranks: SRanks, tys: Seq<Option<Type>>, r: int, c: int) -> bool {
    forall|d: int| 0 <= d < tys.len() ==> {
        let q = ranks.consts[d]->0 as int;
        if q < r || (q == r && d < c) {
            &&& (#[trigger] tys[d]) is Some
            &&& spec_type_of(p, Seq::empty(), tys, p.consts@[d].init) == tys[d]
        } else {
            tys[d] is None
        }
    }
}

pub proof fn lemma_stage_start(p: Program, ranks: SRanks, tys: Seq<Option<Type>>)
    requires
        forall|d: int| 0 <= d < tys.len() ==> (#[trigger] tys[d]) is None,
        forall|d: int| 0 <= d < tys.len() ==> (#[trigger] ranks.consts[d]->0) as int >= 0,
    ensures
        const_stage(p, ranks, tys, 0, 0),
{
    reveal(const_stage);
}

/// The const about to be typed reads only consts of lower rank, which all have their types.
pub proof fn lemma_stage_known(p: Program, ranks: SRanks, tys: Seq<Option<Type>>, r: int, c: int)
    requires
        const_stage(p, ranks, tys, r, c),
        0 <= c < tys.len(),
        tys.len() == ranks.consts.len(),
        ranks.consts[c] == Some(r as usize),
        0 <= r <= usize::MAX,
        spec_expr_ranked(ranks, r as usize, p.consts@[c].init),
        forall|d: int| 0 <= d < tys.len() ==> (#[trigger] ranks.consts[d]) is Some,
    ensures
        spec_reads_known(tys, p.consts@[c].init),
{
    reveal(const_stage);
    assert forall|d: int| #![trigger ranks.consts[d]] 0 <= d < ranks.consts.len() && ranks.consts[d] is Some && (ranks.consts[d]->0 as int) < r implies tys[d] is Some by {}
    lemma_known_from_ranked(ranks, tys, r as usize, p.consts@[c].init);
}

pub proof fn lemma_stage_set(p: Program, ranks: SRanks, tys: Seq<Option<Type>>, r: int, c: int, t: Type)
    requires
        const_stage(p, ranks, tys, r, c),
        0 <= c < tys.len(),
        ranks.consts[c]->0 as int == r,
        spec_type_of(p, Seq::empty(), tys, p.consts@[c].init) == Some(t),
    ensures
        const_stage(p, ranks, tys.update(c, Some(t)), r, c + 1),
{
    reveal(const_stage);
    let t2 = tys.update(c, Some(t));
    assert(extends(tys, t2)) by {
        assert forall|d: int| 0 <= d < tys.len() && (#[trigger] tys[d]) is Some implies t2[d] == tys[d] by {
            if d == c {
                assert(tys[c] is None);
            }
        }
    }
    assert forall|d: int| 0 <= d < t2.len() implies {
        let q = ranks.consts[d]->0 as int;
        if q < r || (q == r && d < c + 1) {
            &&& (#[trigger] t2[d]) is Some
            &&& spec_type_of(p, Seq::empty(), t2, p.consts@[d].init) == t2[d]
        } else {
            t2[d] is None
        }
    } by {
        let q = ranks.consts[d]->0 as int;
        if q < r || (q == r && d < c) {
            assert(tys[d] is Some);
            lemma_type_mono(p, Seq::empty(), tys, t2, p.consts@[d].init);
        } else if d == c {
            lemma_type_mono(p, Seq::empty(), tys, t2, p.consts@[d].init);
        }
    }
}

pub proof fn lemma_stage_skip(p: Program, ranks: SRanks, tys: Seq<Option<Type>>, r: int, c: int)
    requires
        const_stage(p, ranks, tys, r, c),
        0 <= c < tys.len(),
        ranks.consts[c]->0 as int != r,
    ensures
        const_stage(p, ranks, tys, r, c + 1),
{
    reveal(const_stage);
}

pub proof fn lemma_stage_next(p: Program, ranks: SRanks, tys: Seq<Option<Type>>, r: int)
    requires
        const_stage(p, ranks, tys, r, tys.len() as int),
    ensures
        const_stage(p, ranks, tys, r + 1, 0),
{
    reveal(const_stage);
}

pub proof fn lemma_stage_end(p: Program, ranks: SRanks, tys: Seq<Option<Type>>, r: int)
    requires
        const_stage(p, ranks, tys, r, 0),
        tys.len() == p.consts@.len(),
        forall|d: int| 0 <= d < tys.len() ==> ((#[trigger] ranks.consts[d])->0 as int) < r,
    ensures
        spec_consts_typed(p, tys),
{
    reveal(const_stage);
    assert forall|c: int| 0 <= c < tys.len() implies #[trigger] tys[c] is Some && spec_type_of(p, Seq::empty(), tys, p.consts@[c].init) == tys[c] by {
        assert(ranks.consts[c]->0 as int == ranks.consts[c]->0 as int);
    }
}

} // verus!
