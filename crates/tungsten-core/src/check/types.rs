//! Typing (§6.3): expression types, statement rules, const types and annotations.
//!
//! `type_expr` computes `Option<Type>`. `None` means the expression is already in error, and
//! parents report nothing about a `None` operand, so type errors don't cascade.

use vstd::prelude::*;

use crate::check::acyclic::Ranks;
use crate::check::error::*;
use crate::ir::*;
use crate::proofs::typing::*;
use crate::spec::eval::*;
use crate::spec::ops::*;
use crate::spec::typing::*;
use crate::spec::value::*;
use crate::util::*;
use crate::wf as swf;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

/// Where an expression sits: a function (with its locals) or a constant initializer.
pub struct TCx<'a> {
    pub p: &'a Program,
    pub item: CoreItem,
    pub locals: &'a Vec<Type>,
    pub in_const: bool,
    pub const_types: &'a Vec<Option<Type>>,
}

pub open spec fn cx_type_of(cx: TCx, e: Expr) -> Option<Type> {
    spec_type_of(*cx.p, cx.locals@, cx.const_types@, e)
}

/// What the checker needs for "no error" to mean "well typed": ids are in range, and every const
/// the expression reads has a known type.
pub open spec fn cx_hyp(cx: TCx, e: Expr) -> bool {
    &&& swf::wf_program(cx.p)
    &&& swf::wf_expr(cx.p, cx.locals@.len() as nat, &e)
    &&& spec_reads_known(cx.const_types@, e)
}

pub open spec fn cx_stmt_hyp(cx: TCx, s: Stmt) -> bool {
    &&& swf::wf_program(cx.p)
    &&& swf::wf_stmt(cx.p, cx.locals@.len() as nat, &s)
    &&& spec_stmt_reads_known(cx.const_types@, s)
}

pub fn is_numeric(t: &Type) -> (r: bool)
    ensures
        r == spec_is_numeric(*t),
{
    match t {
        Type::I8 | Type::I32 | Type::I64 | Type::F32 | Type::F64 => true,
        _ => false,
    }
}

pub fn is_integer(t: &Type) -> (r: bool)
    ensures
        r == spec_is_integer(*t),
{
    match t {
        Type::I8 | Type::I32 | Type::I64 => true,
        _ => false,
    }
}

pub fn lit_type(l: &Lit) -> (r: Type)
    ensures
        r == spec_lit_type(*l),
{
    match l {
        Lit::Bool(_) => Type::Bool,
        Lit::I8(_) => Type::I8,
        Lit::I32(_) => Type::I32,
        Lit::I64(_) => Type::I64,
        Lit::F32(_) => Type::F32,
        Lit::F64(_) => Type::F64,
    }
}

/// The result type of `l op r`, or `None` if the operands don't fit the operator.
pub fn binary_type(op: &BinOp, l: &Type, r: &Type) -> (out: Option<Type>)
    ensures
        out == spec_binary_type(*op, *l, *r),
{
    match op {
        BinOp::Add | BinOp::Subtract | BinOp::Multiply | BinOp::Divide => {
            if is_numeric(l) && type_eq(l, r) {
                Some(clone_type(l))
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
        BinOp::Equal | BinOp::NotEqual => {
            let scalar = match l {
                Type::Bool => true,
                _ => is_numeric(l),
            };
            if scalar && type_eq(l, r) {
                Some(Type::Bool)
            } else {
                None
            }
        },
        BinOp::Less | BinOp::LessEqual | BinOp::Greater | BinOp::GreaterEqual => {
            if is_numeric(l) && type_eq(l, r) {
                Some(Type::Bool)
            } else {
                None
            }
        },
        BinOp::And | BinOp::Or => match (l, r) {
            (Type::Bool, Type::Bool) => Some(Type::Bool),
            _ => None,
        },
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
            let ok = match l {
                Type::Bool => true,
                _ => is_integer(l),
            };
            if ok && type_eq(l, r) {
                Some(clone_type(l))
            } else {
                None
            }
        },
        BinOp::ShiftLeft | BinOp::ShiftRight | BinOp::LogicalShiftRight => {
            let right_ok = match r {
                Type::I32 => true,
                _ => false,
            };
            if is_integer(l) && right_ok {
                Some(clone_type(l))
            } else {
                None
            }
        },
    }
}

pub fn unary_type(op: &UnOp, t: &Type) -> (r: Option<Type>)
    ensures
        r == spec_unary_type(*op, *t),
{
    match op {
        UnOp::Not => match t {
            Type::Bool => Some(Type::Bool),
            _ => if is_integer(t) { Some(clone_type(t)) } else { None },
        },
        UnOp::Negate => match t {
            Type::Vec3 => Some(Type::Vec3),
            Type::Pos3 => Some(Type::Pos3),
            _ => if is_numeric(t) { Some(clone_type(t)) } else { None },
        },
    }
}

/// Numeric → numeric (identity included), `Bool` → integer, `Pos3` → `Vec3`.
pub fn cast_ok(from: &Type, to: &Type) -> (r: bool)
    ensures
        r == spec_cast_ok(*from, *to),
{
    if is_numeric(from) && is_numeric(to) {
        return true;
    }
    match (from, to) {
        (Type::Bool, _) => is_integer(to),
        (Type::Pos3, Type::Vec3) => true,
        _ => false,
    }
}

fn mismatch(cx: &TCx, expected: &Type, found: &Type, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>)
    ensures
        final(errs)@.len() == old(errs)@.len() + 1,
{
    report(errs, CheckErrorKind::TypeMismatch { expected: clone_type(expected), found: clone_type(found) }, cx.item, path);
}

/// Reports a mismatch if `found` is known and differs from `expected`.
fn expect(cx: &TCx, expected: &Type, found: &Option<Type>, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && *found is Some ==> *found == Some(*expected),
{
    match found {
        Some(t) => {
            if !type_eq(expected, t) {
                mismatch(cx, expected, t, path, errs);
            }
        },
        None => {},
    }
}

fn forbidden(cx: &TCx, what: Forbidden, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: bool)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() ==> !r,
{
    if cx.in_const {
        report(errs, CheckErrorKind::ConstForbidden { what }, cx.item, path);
        true
    } else {
        false
    }
}

/// The return type of the unit that input `i` reads.
fn density_type(p: &Program, i: InputId) -> (r: Option<Type>)
    ensures
        r == spec_density_type(*p, i),
{
    let d = get(&p.inputs, i.0)?;
    let u = get(&p.units, d.source.0)?;
    let f = get(&p.functions, u.main.0)?;
    Some(clone_type(&f.ret))
}

fn field_type(p: &Program, sel: &FieldSel, base: &Type) -> (out: Option<Type>)
    ensures
        out == spec_field_type(*p, *sel, *base),
{
    match (sel, base) {
        (FieldSel::Vec3(_), Type::Vec3) => Some(Type::F64),
        (FieldSel::Pos3(_), Type::Pos3) => Some(Type::I32),
        (FieldSel::Record(r, i), Type::Record(s)) => {
            if r.0 != s.0 {
                return None;
            }
            let d = get(&p.records, r.0)?;
            let t = get(&d.fields, *i)?;
            Some(clone_type(t))
        },
        _ => None,
    }
}

/// One arm of `type_expr`, in a function of its own so that its proof is a query of its own.
#[verifier::loop_isolation(false)]
fn type_call(cx: &TCx, func: FnId, args: &Vec<Expr>, e: Ghost<Expr>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: Option<Type>)
    requires
        e@ == (Expr::Call { func, args: *args }),
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_hyp(*cx, e@) ==> r is Some && r == cx_type_of(*cx, e@),
    decreases e@, 0int,
{
    let p = cx.p;
    if forbidden(cx, Forbidden::Call, path, errs) {
        return None;
    }
    let f = match get(&p.functions, func.0) {
        Some(f) => f,
        None => { return None; },
    };
    if args.len() != f.params {
        report(errs, CheckErrorKind::Arity { expected: f.params, found: args.len() }, cx.item, path);
    }
    let ghost e1 = old(errs)@.len();
    let mut i: usize = 0;
    while i < args.len()
        invariant
            i <= args.len(),
            *f == cx.p.functions@[func.0 as int],
            func.0 < cx.p.functions@.len(),
            args.len() != f.params ==> errs@.len() > e1,
            errs@.len() >= e1,
            errs@.len() == e1 && cx_hyp(*cx, e@) ==> forall|k: int| #![trigger args@[k]] 0 <= k < i
                ==> cx_type_of(*cx, args@[k]) == Some(f.locals@[k]),
        decreases args.len() - i,
    {
        proof {
            if cx_hyp(*cx, e@) {
                assert(swf::wf_expr(cx.p, cx.locals@.len() as nat, &args[i as int]));
                assert(spec_reads_known(cx.const_types@, args@[i as int]));
                assert(swf::wf_function(cx.p, &cx.p.functions[func.0 as int]));
            }
        }
        path.push(CoreStep::Arg(i));
        let t = type_expr(cx, &args[i], path, errs);
        if args.len() == f.params {
            match get(&f.locals, i) {
                Some(want) => expect(cx, want, &t, path, errs),
                None => {},
            }
        }
        let _ = path.pop();
        i = i + 1;
    }
    Some(clone_type(&f.ret))
}

/// One arm of `type_expr`, in a function of its own so that its proof is a query of its own.
#[verifier::loop_isolation(false)]
fn type_extern_call(cx: &TCx, ext: ExternId, params: &Vec<Type>, ret: &Type, args: &Vec<Expr>, e: Ghost<Expr>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: Option<Type>)
    requires
        e@ == (Expr::ExternCall { ext, params: *params, ret: *ret, args: *args }),
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_hyp(*cx, e@) ==> r is Some && r == cx_type_of(*cx, e@),
    decreases e@, 0int,
{
    let p = cx.p;
    if forbidden(cx, Forbidden::ExternCall, path, errs) {
        return None;
    }
    let d = match get(&p.externs, ext.0) {
        Some(d) => d,
        None => { return None; },
    };
    if !types_eq(params, &d.params) || !type_eq(ret, &d.ret) {
        report(errs, CheckErrorKind::ExternSignature { ext: ext }, cx.item, path);
    }
    if args.len() != d.params.len() {
        report(errs, CheckErrorKind::Arity { expected: d.params.len(), found: args.len() }, cx.item, path);
    }
    let ghost e1 = old(errs)@.len();
    let mut i: usize = 0;
    while i < args.len()
        invariant
            i <= args.len(),
            *d == cx.p.externs@[ext.0 as int],
            args.len() != d.params.len() ==> errs@.len() > e1,
            errs@.len() >= e1,
            errs@.len() == e1 && cx_hyp(*cx, e@) ==> forall|k: int| #![trigger args@[k]] 0 <= k < i
                ==> cx_type_of(*cx, args@[k]) == Some(d.params@[k]),
        decreases args.len() - i,
    {
        proof {
            if cx_hyp(*cx, e@) {
                assert(swf::wf_expr(cx.p, cx.locals@.len() as nat, &args[i as int]));
                assert(spec_reads_known(cx.const_types@, args@[i as int]));
            }
        }
        path.push(CoreStep::Arg(i));
        let t = type_expr(cx, &args[i], path, errs);
        if args.len() == d.params.len() {
            match get(&d.params, i) {
                Some(want) => expect(cx, want, &t, path, errs),
                None => {},
            }
        }
        let _ = path.pop();
        i = i + 1;
    }
    Some(clone_type(&d.ret))
}

/// One arm of `type_expr`, in a function of its own so that its proof is a query of its own.
#[verifier::loop_isolation(false)]
fn type_make_record(cx: &TCx, record: RecordId, fields: &Vec<FieldInit>, e: Ghost<Expr>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: Option<Type>)
    requires
        e@ == (Expr::MakeRecord { record, fields: *fields }),
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_hyp(*cx, e@) ==> r is Some && r == cx_type_of(*cx, e@),
    decreases e@, 0int,
{
    let p = cx.p;
    let d = match get(&p.records, record.0) {
        Some(d) => d,
        None => { return None; },
    };
    let mut seen = bools(d.fields.len(), false);
    let ghost e1 = old(errs)@.len();
    let ghost n = d.fields@.len();
    let mut i: usize = 0;
    while i < fields.len()
        invariant
            i <= fields.len(),
            n == d.fields@.len(),
            *d == cx.p.records@[record.0 as int],
            seen@.len() == n,
            errs@.len() >= e1,
            errs@.len() == e1 && cx_hyp(*cx, e@) ==> {
                &&& forall|k: int| #![trigger fields@[k]] 0 <= k < i ==> {
                    &&& fields@[k].field < n
                    &&& cx_type_of(*cx, fields@[k].value) == Some(d.fields@[fields@[k].field as int])
                }
                &&& forall|a: int, b: int| #![trigger fields@[a], fields@[b]] 0 <= a < b < i ==> fields@[a].field != fields@[b].field
                &&& forall|a: int| 0 <= a < i ==> seen@[#[trigger] fields@[a].field as int]
                &&& forall|j: int| 0 <= j < n && seen@[j] ==> spec_field_given(fields@.subrange(0, i as int), j)
            },
        decreases fields.len() - i,
    {
        let k = fields[i].field;
        let ghost seen0 = seen@;
        path.push(CoreStep::Field(record, fields[i].field));
        if get_bool(&seen, k) {
            report(errs, CheckErrorKind::RecordFieldRepeated { record: record, field: k }, cx.item, path);
        }
        set(&mut seen, k, true);
        proof {
            if cx_hyp(*cx, e@) {
                assert(swf::wf_expr(cx.p, cx.locals@.len() as nat, &fields[i as int].value));
                assert(spec_reads_known(cx.const_types@, fields@[i as int].value));
            }
        }
        let t = type_expr(cx, &fields[i].value, path, errs);
        match get(&d.fields, k) {
            Some(want) => expect(cx, want, &t, path, errs),
            None => {},
        }
        let _ = path.pop();
        proof {
            if errs@.len() == e1 && cx_hyp(*cx, e@) {
                let sub = fields@.subrange(0, i as int + 1);
                assert forall|j: int| 0 <= j < n && seen@[j] implies spec_field_given(sub, j) by {
                    if j == k as int {
                        assert(sub[i as int].field == j);
                    } else {
                        assert(seen0[j]);
                        let prev = fields@.subrange(0, i as int);
                        assert(spec_field_given(prev, j));
                        let a = choose|a: int| 0 <= a < prev.len() && #[trigger] prev[a].field == j;
                        assert(sub[a] == prev[a]);
                    }
                }
            }
        }
        i = i + 1;
    }
    let ghost e2 = errs@.len();
    let mut k: usize = 0;
    while k < d.fields.len()
        invariant
            k <= d.fields.len(),
            seen@.len() == d.fields@.len(),
            errs@.len() >= e2,
            errs@.len() == e2 ==> forall|j: int| 0 <= j < k ==> seen@[j],
        decreases d.fields.len() - k,
    {
        if !get_bool(&seen, k) {
            report(errs, CheckErrorKind::RecordFieldMissing { record: record, field: k }, cx.item, path);
        }
        k = k + 1;
    }
    proof {
        if errs@.len() == old(errs)@.len() && cx_hyp(*cx, e@) {
            assert(fields@.subrange(0, fields@.len() as int) =~= fields@);
            assert forall|j: int| #![trigger spec_field_given(fields@, j)] 0 <= j < d.fields@.len() implies spec_field_given(fields@, j) by {
                assert(seen@[j]);
            }
        }
    }
    Some(Type::Record(record))
}

/// One arm of `type_expr`, in a function of its own so that its proof is a query of its own.
#[verifier::loop_isolation(false)]
fn type_array_lit(cx: &TCx, es: &Vec<Expr>, e: Ghost<Expr>, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: Option<Type>)
    requires
        e@ == Expr::ArrayLit(*es),
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_hyp(*cx, e@) ==> r is Some && r == cx_type_of(*cx, e@),
    decreases e@, 0int,
{
    if es.len() == 0 {
        report(errs, CheckErrorKind::EmptyArray, cx.item, path);
        return None;
    }
    proof {
        if cx_hyp(*cx, e@) {
            assert(swf::wf_expr(cx.p, cx.locals@.len() as nat, &es[0]));
            assert(spec_reads_known(cx.const_types@, es@[0]));
        }
    }
    path.push(CoreStep::Element(0));
    let first = type_expr(cx, &es[0], path, errs);
    let _ = path.pop();
    let ghost e1 = old(errs)@.len();
    let mut i: usize = 1;
    while i < es.len()
        invariant
            1 <= i <= es.len(),
            errs@.len() >= e1,
            errs@.len() == e1 && cx_hyp(*cx, e@) ==> first is Some && first == cx_type_of(*cx, es@[0])
                && forall|k: int| #![trigger es@[k]] 0 <= k < i ==> cx_type_of(*cx, es@[k]) == first,
        decreases es.len() - i,
    {
        proof {
            if cx_hyp(*cx, e@) {
                assert(swf::wf_expr(cx.p, cx.locals@.len() as nat, &es[i as int]));
                assert(spec_reads_known(cx.const_types@, es@[i as int]));
            }
        }
        path.push(CoreStep::Element(i));
        let t = type_expr(cx, &es[i], path, errs);
        match &first {
            Some(f) => expect(cx, f, &t, path, errs),
            None => {},
        }
        let _ = path.pop();
        i = i + 1;
    }
    match first {
        Some(f) => Some(Type::Array(Box::new(f), es.len())),
        None => None,
    }
}

#[verifier::loop_isolation(false)]
pub fn type_expr(cx: &TCx, e: &Expr, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> (r: Option<Type>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_hyp(*cx, *e) ==> r is Some && r == cx_type_of(*cx, *e),
    decreases e, 1int,
{
    let p = cx.p;
    match e {
        Expr::Lit(l) => Some(lit_type(l)),
        Expr::Local(l) => match get(cx.locals, l.0) {
            Some(t) => Some(clone_type(t)),
            None => None,
        },
        Expr::Const(c) => match get(cx.const_types, c.0) {
            Some(Some(t)) => Some(clone_type(t)),
            _ => None,
        },
        Expr::Host(h) => {
            if forbidden(cx, Forbidden::Host, path, errs) {
                return None;
            }
            match get(&p.hosts, h.0) {
                Some(d) => Some(clone_type(&d.ty)),
                None => None,
            }
        },
        Expr::Table(t) => {
            if forbidden(cx, Forbidden::Table, path, errs) {
                return None;
            }
            match get(&p.tables, t.0) {
                Some(d) => match d.kind {
                    TableKind::Perm => Some(Type::PermTable),
                    TableKind::Sampler => Some(Type::Sampler),
                },
                None => None,
            }
        },
        Expr::Ambient(a) => {
            if forbidden(cx, Forbidden::Ambient, path, errs) {
                return None;
            }
            match a {
                Ambient::Pos3 => Some(Type::Pos3),
                _ => Some(Type::Vec3),
            }
        },
        Expr::Density { input, index } => {
            if forbidden(cx, Forbidden::Density, path, errs) {
                return None;
            }
            match index {
                Some(ix) => {
                    path.push(CoreStep::Index);
                    let t = type_expr(cx, ix, path, errs);
                    expect(cx, &Type::I32, &t, path, errs);
                    let _ = path.pop();
                },
                None => {},
            }
            density_type(p, *input)
        },
        Expr::Call { func, args } => type_call(cx, *func, args, Ghost(*e), path, errs),
        Expr::ExternCall { ext, params, ret, args } => type_extern_call(cx, *ext, params, ret, args, Ghost(*e), path, errs),
        Expr::Binary { op, left, right } => {
            path.push(CoreStep::Left);
            let l = type_expr(cx, left, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Right);
            let r = type_expr(cx, right, path, errs);
            let _ = path.pop();
            match (l, r) {
                (Some(l), Some(r)) => match binary_type(op, &l, &r) {
                    Some(t) => Some(t),
                    None => {
                        report(errs, CheckErrorKind::BadBinary { op: copy_binop(op), left: l, right: r }, cx.item, path);
                        None
                    },
                },
                _ => None,
            }
        },
        Expr::Unary { op, operand } => {
            path.push(CoreStep::Operand);
            let t = type_expr(cx, operand, path, errs);
            let _ = path.pop();
            match t {
                Some(t) => match unary_type(op, &t) {
                    Some(r) => Some(r),
                    None => {
                        report(errs, CheckErrorKind::BadUnary { op: copy_unop(op), operand: t }, cx.item, path);
                        None
                    },
                },
                None => None,
            }
        },
        Expr::Field { base, sel, ty } => {
            path.push(CoreStep::Base);
            let b = type_expr(cx, base, path, errs);
            let _ = path.pop();
            let b = match b {
                Some(b) => b,
                None => { return None; },
            };
            match field_type(p, sel, &b) {
                Some(t) => {
                    if !type_eq(ty, &t) {
                        report(errs, CheckErrorKind::FieldAnnotation { annotated: clone_type(ty), actual: clone_type(&t) }, cx.item, path);
                    }
                    Some(t)
                },
                None => {
                    report(errs, CheckErrorKind::BadFieldBase { sel: copy_sel(sel), found: b }, cx.item, path);
                    None
                },
            }
        },
        Expr::Index { array, index } => {
            path.push(CoreStep::Base);
            let a = type_expr(cx, array, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Index);
            let i = type_expr(cx, index, path, errs);
            expect(cx, &Type::I32, &i, path, errs);
            let _ = path.pop();
            match a {
                Some(Type::Array(inner, _)) => Some(clone_type(&inner)),
                Some(t) => {
                    report(errs, CheckErrorKind::NotArray { found: t }, cx.item, path);
                    None
                },
                None => None,
            }
        },
        Expr::MakeVec3 { x, y, z } => {
            path.push(CoreStep::Arg(0));
            let t = type_expr(cx, x, path, errs);
            expect(cx, &Type::F64, &t, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(1));
            let t = type_expr(cx, y, path, errs);
            expect(cx, &Type::F64, &t, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(2));
            let t = type_expr(cx, z, path, errs);
            expect(cx, &Type::F64, &t, path, errs);
            let _ = path.pop();
            Some(Type::Vec3)
        },
        Expr::MakePos3 { x, y, z } => {
            path.push(CoreStep::Arg(0));
            let t = type_expr(cx, x, path, errs);
            expect(cx, &Type::I32, &t, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(1));
            let t = type_expr(cx, y, path, errs);
            expect(cx, &Type::I32, &t, path, errs);
            let _ = path.pop();
            path.push(CoreStep::Arg(2));
            let t = type_expr(cx, z, path, errs);
            expect(cx, &Type::I32, &t, path, errs);
            let _ = path.pop();
            Some(Type::Pos3)
        },
        Expr::MakeRecord { record, fields } => type_make_record(cx, *record, fields, Ghost(*e), path, errs),
        Expr::ArrayLit(es) => type_array_lit(cx, es, Ghost(*e), path, errs),
        Expr::Cast { to, operand } => {
            path.push(CoreStep::Operand);
            let t = type_expr(cx, operand, path, errs);
            let _ = path.pop();
            match t {
                Some(t) => {
                    if cast_ok(&t, to) {
                        Some(clone_type(to))
                    } else {
                        report(errs, CheckErrorKind::BadCast { from: t, to: clone_type(to) }, cx.item, path);
                        None
                    }
                },
                None => None,
            }
        },
    }
}

#[verifier::loop_isolation(false)]
pub fn type_stmts(cx: &TCx, ret: &Type, ss: &Vec<Stmt>, kind: StepKind, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() ==> forall|k: int| #![trigger ss@[k]] 0 <= k < ss@.len() && cx_stmt_hyp(*cx, ss@[k])
            ==> spec_stmt_typed(*cx.p, cx.locals@, cx.const_types@, *ret, ss@[k]),
    decreases ss,
{
    let ghost e0 = errs@.len();
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
            errs@.len() >= e0,
            errs@.len() == e0 ==> forall|k: int| #![trigger ss@[k]] 0 <= k < i && cx_stmt_hyp(*cx, ss@[k])
                ==> spec_stmt_typed(*cx.p, cx.locals@, cx.const_types@, *ret, ss@[k]),
        decreases ss.len() - i,
    {
        path.push(mk_step(kind, i));
        type_stmt(cx, ret, &ss[i], path, errs);
        let _ = path.pop();
        i = i + 1;
    }
}

pub fn type_stmt(cx: &TCx, ret: &Type, s: &Stmt, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && cx_stmt_hyp(*cx, *s)
            ==> spec_stmt_typed(*cx.p, cx.locals@, cx.const_types@, *ret, *s),
    decreases s,
{
    match s {
        Stmt::Assign { target, value } => {
            path.push(CoreStep::Value);
            let t = type_expr(cx, value, path, errs);
            match get(cx.locals, target.0) {
                Some(want) => expect(cx, want, &t, path, errs),
                None => {},
            }
            let _ = path.pop();
        },
        Stmt::Return(e) => {
            path.push(CoreStep::Value);
            let t = type_expr(cx, e, path, errs);
            expect(cx, ret, &t, path, errs);
            let _ = path.pop();
        },
        Stmt::If { cond, then_branch, else_branch } => {
            path.push(CoreStep::Cond);
            let t = type_expr(cx, cond, path, errs);
            expect(cx, &Type::Bool, &t, path, errs);
            let _ = path.pop();
            type_stmts(cx, ret, then_branch, StepKind::Then, path, errs);
            type_stmts(cx, ret, else_branch, StepKind::Else, path, errs);
        },
        Stmt::While { cond, body } => {
            path.push(CoreStep::Cond);
            let t = type_expr(cx, cond, path, errs);
            expect(cx, &Type::Bool, &t, path, errs);
            let _ = path.pop();
            type_stmts(cx, ret, body, StepKind::Body, path, errs);
        },
        Stmt::Repeat { body, .. } => {
            type_stmts(cx, ret, body, StepKind::Body, path, errs);
        },
        Stmt::Break => {},
    }
}

fn is_rank(x: Option<usize>, r: usize) -> (out: bool)
    ensures
        out == (x == Some(r)),
{
    match x {
        Some(y) => y == r,
        None => false,
    }
}

/// Derives every const's type, in rank order so a const's dependencies are typed first. A const
/// with no rank (on or reaching a cycle) has type `None`; its init is still checked.
#[verifier::loop_isolation(false)]
pub fn const_types(p: &Program, ranks: &Ranks, errs: &mut Vec<CheckError>) -> (tys: Vec<Option<Type>>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && swf::wf_program(p) && spec_ranks_valid(*p, view_ranks(*ranks))
            ==> spec_consts_typed(*p, tys@),
{
    let ghost e0 = errs@.len();
    let ghost vr = view_ranks(*ranks);
    let ghost hyp = swf::wf_program(p) && spec_ranks_valid(*p, vr);
    let n = p.consts.len();
    let mut tys: Vec<Option<Type>> = Vec::new();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
            n == p.consts@.len(),
            tys@.len() == k,
            forall|j: int| 0 <= j < k ==> (#[trigger] tys@[j]) is None,
        decreases n - k,
    {
        tys.push(None);
        k = k + 1;
    }
    let mut max: usize = 0;
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
            n == p.consts@.len(),
            hyp ==> forall|j: int| 0 <= j < k ==> (#[trigger] ranks.consts@[j])->0 <= max,
        decreases n - k,
    {
        match get_rank(&ranks.consts, k) {
            Some(r) => {
                if r > max {
                    max = r;
                }
            },
            None => {},
        }
        k = k + 1;
    }
    let no_locals: Vec<Type> = Vec::new();
    proof {
        assert(no_locals@ =~= Seq::<Type>::empty());
    }
    let rounds = add_or_max(max, 1);
    proof {
        if errs@.len() == e0 && hyp {
            lemma_stage_start(*p, vr, tys@);
        }
    }
    let mut r: usize = 0;
    while r < rounds
        invariant
            r <= rounds,
            n == p.consts.len(),
            tys@.len() == n,
            errs@.len() >= e0,
            errs@.len() == e0 && hyp ==> const_stage(*p, vr, tys@, r as int, 0),
        decreases rounds - r,
    {
        let mut c: usize = 0;
        while c < n
            invariant
                c <= n,
                r < rounds,
                n == p.consts.len(),
                tys@.len() == n,
                errs@.len() >= e0,
                errs@.len() == e0 && hyp ==> const_stage(*p, vr, tys@, r as int, c as int),
            decreases n - c,
        {
            if is_rank(get_rank(&ranks.consts, c), r) {
                let ghost tys0 = tys@;
                let t = {
                    let cx = TCx { p, item: CoreItem::Const(ConstId(c)), locals: &no_locals, in_const: true, const_types: &tys };
                    let mut path: Vec<CoreStep> = Vec::new();
                    path.push(CoreStep::Value);
                    proof {
                        if errs@.len() == e0 && hyp {
                            assert(vr.consts[c as int] == Some(r));
                            lemma_stage_known(*p, vr, tys0, r as int, c as int);
                            assert(swf::wf_expr(p, 0, &p.consts[c as int].init));
                        }
                    }
                    type_expr(&cx, &p.consts[c].init, &mut path, errs)
                };
                set(&mut tys, c, t);
                proof {
                    if errs@.len() == e0 && hyp {
                        lemma_stage_set(*p, vr, tys0, r as int, c as int, t->0);
                    }
                }
            } else {
                proof {
                    if errs@.len() == e0 && hyp {
                        assert(vr.consts[c as int]->0 as int != r);
                        lemma_stage_skip(*p, vr, tys@, r as int, c as int);
                    }
                }
            }
            c = c + 1;
        }
        proof {
            if errs@.len() == e0 && hyp {
                lemma_stage_next(*p, vr, tys@, r as int);
            }
        }
        r = r + 1;
    }
    let ghost final_tys = tys@;
    let ghost e_mid = errs@.len();
    proof {
        if errs@.len() == e0 && hyp {
            assert forall|d: int| 0 <= d < tys@.len() implies ((#[trigger] vr.consts[d])->0 as int) < rounds by {
                assert(ranks.consts@[d]->0 <= max);
                assert(vr.consts[d]->0 < usize::MAX);
            }
            lemma_stage_end(*p, vr, tys@, rounds as int);
        }
    }
    let mut c: usize = 0;
    while c < n
        invariant
            c <= n,
            n == p.consts.len(),
            tys@ == final_tys,
            e_mid >= e0,
            errs@.len() >= e_mid,
            e_mid == e0 && hyp ==> spec_consts_typed(*p, final_tys),
        decreases n - c,
    {
        if get_rank(&ranks.consts, c).is_none() {
            let cx = TCx { p, item: CoreItem::Const(ConstId(c)), locals: &no_locals, in_const: true, const_types: &tys };
            let mut path: Vec<CoreStep> = Vec::new();
            path.push(CoreStep::Value);
            let _ = type_expr(&cx, &p.consts[c].init, &mut path, errs);
        }
        c = c + 1;
    }
    tys
}

/// Types every function body and checks unit mains have no params.
#[verifier::loop_isolation(false)]
pub fn check_types(p: &Program, const_types: &Vec<Option<Type>>, errs: &mut Vec<CheckError>)
    ensures
        final(errs)@.len() >= old(errs)@.len(),
        final(errs)@.len() == old(errs)@.len() && swf::wf_program(p) && (forall|c: int| 0 <= c < const_types@.len() ==> #[trigger] const_types@[c] is Some) && const_types@.len() == p.consts@.len() ==> {
            &&& forall|k: int| 0 <= k < p.functions@.len() ==> spec_function_typed(*p, const_types@, #[trigger] p.functions@[k])
            &&& forall|u: int| 0 <= u < p.units@.len() ==> p.functions@[(#[trigger] p.units@[u]).main.0 as int].params == 0
        },
{
    let ghost e0 = errs@.len();
    let ghost hyp = swf::wf_program(p) && (forall|c: int| 0 <= c < const_types@.len() ==> #[trigger] const_types@[c] is Some) && const_types@.len() == p.consts@.len();
    let mut k: usize = 0;
    while k < p.functions.len()
        invariant
            k <= p.functions.len(),
            errs@.len() >= e0,
            errs@.len() == e0 && hyp ==> forall|j: int| 0 <= j < k ==> spec_function_typed(*p, const_types@, #[trigger] p.functions@[j]),
        decreases p.functions.len() - k,
    {
        let f = &p.functions[k];
        let cx = TCx { p, item: CoreItem::Function(FnId(k)), locals: &f.locals, in_const: false, const_types };
        let mut path: Vec<CoreStep> = Vec::new();
        type_stmts(&cx, &f.ret, &f.body, StepKind::Stmt, &mut path, errs);
        proof {
            if errs@.len() == e0 && hyp {
                assert(swf::wf_function(p, &p.functions[k as int]));
                assert forall|j: int| #![trigger f.body@[j]] 0 <= j < f.body@.len() implies spec_stmt_typed(*p, f.locals@, const_types@, f.ret, f.body@[j]) by {
                    assert(swf::wf_stmt(p, f.locals@.len() as nat, &f.body[j]));
                    lemma_stmt_known_all(p, f.locals@.len() as nat, const_types@, f.body@[j]);
                }
            }
        }
        k = k + 1;
    }
    let ghost e1 = errs@.len();
    let mut u: usize = 0;
    while u < p.units.len()
        invariant
            u <= p.units.len(),
            errs@.len() >= e1,
            errs@.len() == e1 && hyp ==> forall|j: int| 0 <= j < u ==> p.functions@[(#[trigger] p.units@[j]).main.0 as int].params == 0,
        decreases p.units.len() - u,
    {
        match get(&p.functions, p.units[u].main.0) {
            Some(f) => {
                if f.params > 0 {
                    let mut path: Vec<CoreStep> = Vec::new();
                    path.push(CoreStep::Main);
                    report(errs, CheckErrorKind::MainParams { params: f.params }, CoreItem::Unit(UnitId(u)), &path);
                }
            },
            None => {},
        }
        proof {
            if errs@.len() == e1 && hyp {
                assert(swf::wf_unit(p, &p.units[u as int]));
            }
        }
        u = u + 1;
    }
}

} // verus!
