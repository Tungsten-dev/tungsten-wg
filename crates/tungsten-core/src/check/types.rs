//! Typing (§6.3): expression types, statement rules, const types and annotations.
//!
//! `type_expr` computes `Option<Type>`. `None` means the expression is already in error, and
//! parents report nothing about a `None` operand, so type errors don't cascade.

use vstd::prelude::*;

use crate::check::acyclic::Ranks;
use crate::check::error::*;
use crate::ir::*;
use crate::util::*;

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

pub fn is_numeric(t: &Type) -> bool {
    match t {
        Type::I8 | Type::I32 | Type::I64 | Type::F32 | Type::F64 => true,
        _ => false,
    }
}

pub fn is_integer(t: &Type) -> bool {
    match t {
        Type::I8 | Type::I32 | Type::I64 => true,
        _ => false,
    }
}

pub fn lit_type(l: &Lit) -> Type {
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
pub fn binary_type(op: &BinOp, l: &Type, r: &Type) -> Option<Type> {
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

pub fn unary_type(op: &UnOp, t: &Type) -> Option<Type> {
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
pub fn cast_ok(from: &Type, to: &Type) -> bool {
    if is_numeric(from) && is_numeric(to) {
        return true;
    }
    match (from, to) {
        (Type::Bool, _) => is_integer(to),
        (Type::Pos3, Type::Vec3) => true,
        _ => false,
    }
}

fn mismatch(cx: &TCx, expected: &Type, found: &Type, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>) {
    report(errs, CheckErrorKind::TypeMismatch { expected: clone_type(expected), found: clone_type(found) }, cx.item, path);
}

/// Reports a mismatch if `found` is known and differs from `expected`.
fn expect(cx: &TCx, expected: &Type, found: &Option<Type>, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>) {
    match found {
        Some(t) => {
            if !type_eq(expected, t) {
                mismatch(cx, expected, t, path, errs);
            }
        },
        None => {},
    }
}

fn forbidden(cx: &TCx, what: Forbidden, path: &Vec<CoreStep>, errs: &mut Vec<CheckError>) -> bool {
    if cx.in_const {
        report(errs, CheckErrorKind::ConstForbidden { what }, cx.item, path);
        true
    } else {
        false
    }
}

/// The return type of the unit that input `i` reads.
fn density_type(p: &Program, i: InputId) -> Option<Type> {
    let d = get(&p.inputs, i.0)?;
    let u = get(&p.units, d.source.0)?;
    let f = get(&p.functions, u.main.0)?;
    Some(clone_type(&f.ret))
}

fn field_type(p: &Program, sel: &FieldSel, base: &Type) -> Option<Type> {
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

#[verifier::loop_isolation(false)]
pub fn type_expr(cx: &TCx, e: &Expr, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>) -> Option<Type>
    decreases e,
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
                Some(d) => Some(Type::Handle(HandleTypeId(d.ty.0))),
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
        Expr::Call { func, args } => {
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
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
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
        },
        Expr::ExternCall { ext, params, ret, args } => {
            if forbidden(cx, Forbidden::ExternCall, path, errs) {
                return None;
            }
            let d = match get(&p.externs, ext.0) {
                Some(d) => d,
                None => { return None; },
            };
            if !types_eq(params, &d.params) || !type_eq(ret, &d.ret) {
                report(errs, CheckErrorKind::ExternSignature { ext: *ext }, cx.item, path);
            }
            if args.len() != d.params.len() {
                report(errs, CheckErrorKind::Arity { expected: d.params.len(), found: args.len() }, cx.item, path);
            }
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
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
        },
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
        Expr::MakeRecord { record, fields } => {
            let d = match get(&p.records, record.0) {
                Some(d) => d,
                None => { return None; },
            };
            let mut seen = bools(d.fields.len(), false);
            let mut i: usize = 0;
            while i < fields.len()
                invariant
                    i <= fields.len(),
                decreases fields.len() - i,
            {
                let k = fields[i].field;
                path.push(CoreStep::Field(*record, fields[i].field));
                if get_bool(&seen, k) {
                    report(errs, CheckErrorKind::RecordFieldRepeated { record: *record, field: k }, cx.item, path);
                }
                set(&mut seen, k, true);
                let t = type_expr(cx, &fields[i].value, path, errs);
                match get(&d.fields, k) {
                    Some(want) => expect(cx, want, &t, path, errs),
                    None => {},
                }
                let _ = path.pop();
                i = i + 1;
            }
            let mut k: usize = 0;
            while k < d.fields.len()
                invariant
                    k <= d.fields.len(),
                decreases d.fields.len() - k,
            {
                if !get_bool(&seen, k) {
                    report(errs, CheckErrorKind::RecordFieldMissing { record: *record, field: k }, cx.item, path);
                }
                k = k + 1;
            }
            Some(Type::Record(*record))
        },
        Expr::ArrayLit(es) => {
            if es.len() == 0 {
                report(errs, CheckErrorKind::EmptyArray, cx.item, path);
                return None;
            }
            path.push(CoreStep::Element(0));
            let first = type_expr(cx, &es[0], path, errs);
            let _ = path.pop();
            let mut i: usize = 1;
            while i < es.len()
                invariant
                    1 <= i <= es.len(),
                decreases es.len() - i,
            {
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
        },
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
    decreases ss,
{
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        path.push(mk_step(kind, i));
        type_stmt(cx, ret, &ss[i], path, errs);
        let _ = path.pop();
        i = i + 1;
    }
}

pub fn type_stmt(cx: &TCx, ret: &Type, s: &Stmt, path: &mut Vec<CoreStep>, errs: &mut Vec<CheckError>)
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

fn is_rank(x: Option<usize>, r: usize) -> bool {
    match x {
        Some(y) => y == r,
        None => false,
    }
}

/// Derives every const's type, in rank order so a const's dependencies are typed first. A const
/// with no rank (on or reaching a cycle) has type `None`; its init is still checked.
pub fn const_types(p: &Program, ranks: &Ranks, errs: &mut Vec<CheckError>) -> Vec<Option<Type>> {
    let n = p.consts.len();
    let mut tys: Vec<Option<Type>> = Vec::new();
    let mut k: usize = 0;
    while k < n
        invariant
            k <= n,
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
    let rounds = add_or_max(max, 1);
    let mut r: usize = 0;
    while r < rounds
        invariant
            r <= rounds,
            n == p.consts.len(),
        decreases rounds - r,
    {
        let mut c: usize = 0;
        while c < n
            invariant
                c <= n,
                n == p.consts.len(),
            decreases n - c,
        {
            if is_rank(get_rank(&ranks.consts, c), r) {
                let t = {
                    let cx = TCx { p, item: CoreItem::Const(ConstId(c)), locals: &no_locals, in_const: true, const_types: &tys };
                    let mut path: Vec<CoreStep> = Vec::new();
                    path.push(CoreStep::Value);
                    type_expr(&cx, &p.consts[c].init, &mut path, errs)
                };
                set(&mut tys, c, t);
            }
            c = c + 1;
        }
        r = r + 1;
    }
    let mut c: usize = 0;
    while c < n
        invariant
            c <= n,
            n == p.consts.len(),
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
pub fn check_types(p: &Program, const_types: &Vec<Option<Type>>, errs: &mut Vec<CheckError>) {
    let mut k: usize = 0;
    while k < p.functions.len()
        invariant
            k <= p.functions.len(),
        decreases p.functions.len() - k,
    {
        let f = &p.functions[k];
        let cx = TCx { p, item: CoreItem::Function(FnId(k)), locals: &f.locals, in_const: false, const_types };
        let mut path: Vec<CoreStep> = Vec::new();
        type_stmts(&cx, &f.ret, &f.body, StepKind::Stmt, &mut path, errs);
        k = k + 1;
    }
    let mut u: usize = 0;
    while u < p.units.len()
        invariant
            u <= p.units.len(),
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
        u = u + 1;
    }
}

} // verus!
