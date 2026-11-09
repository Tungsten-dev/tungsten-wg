//! Bounds-checked `Vec` access and structural clones for the recursive IR types.
//!
//! Every id-based `Vec` read in `check/` and `exec/` goes through [`get`] (writes: [`set()`]). A miss
//! is `Stuck(BadIndex)` in exec and an internal wf error in the checker. Loops over a node's own
//! children index directly (`&v[i]` under `i < v.len()`), because the termination measure needs
//! the element to be a known part of the node, which [`get`] (no `ensures`) can't tell Verus.
//!
//! The `clone_*` fns are what the `impl Clone` blocks at the bottom of `ir.rs` delegate to, so
//! verified code never calls a fn Verus doesn't know.

use vstd::prelude::*;

use crate::ir::*;

verus! {

broadcast use vstd::std_specs::vec::group_vec_axioms;

pub fn get<T>(v: &Vec<T>, i: usize) -> Option<&T> {
    if i < v.len() {
        Some(&v[i])
    } else {
        None
    }
}

/// Replaces `v[i]` with `x`. Returns `false` (and drops `x`) if `i` is out of range.
pub fn set<T>(v: &mut Vec<T>, i: usize, x: T) -> bool {
    if i < v.len() {
        v.set(i, x);
        true
    } else {
        false
    }
}

/// `v[i]` for a `Copy`-like `Option<usize>` table (ranks).
pub fn get_rank(v: &Vec<Option<usize>>, i: usize) -> Option<usize> {
    if i < v.len() {
        v[i]
    } else {
        None
    }
}

pub fn get_bool(v: &Vec<bool>, i: usize) -> bool {
    if i < v.len() {
        v[i]
    } else {
        false
    }
}

/// `true` if `x` occurs in `v`.
pub fn contains_usize(v: &Vec<usize>, x: usize) -> bool {
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        if v[i] == x {
            return true;
        }
        i = i + 1;
    }
    false
}

pub fn contains_fn(v: &Vec<FnId>, x: FnId) -> bool {
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        if v[i].0 == x.0 {
            return true;
        }
        i = i + 1;
    }
    false
}

pub fn contains_host(v: &Vec<HostId>, x: HostId) -> bool {
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        if v[i].0 == x.0 {
            return true;
        }
        i = i + 1;
    }
    false
}

pub fn contains_table(v: &Vec<TableId>, x: TableId) -> bool {
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        if v[i].0 == x.0 {
            return true;
        }
        i = i + 1;
    }
    false
}

pub fn contains_const(v: &Vec<ConstId>, x: ConstId) -> bool {
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        if v[i].0 == x.0 {
            return true;
        }
        i = i + 1;
    }
    false
}

pub fn contains_input(v: &Vec<InputId>, x: InputId) -> bool {
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        if v[i].0 == x.0 {
            return true;
        }
        i = i + 1;
    }
    false
}

/// A `Vec<bool>` of length `n`, all `x`.
pub fn bools(n: usize, x: bool) -> Vec<bool> {
    let mut v: Vec<bool> = Vec::new();
    let mut i: usize = 0;
    while i < n
        invariant
            i <= n,
        decreases n - i,
    {
        v.push(x);
        i = i + 1;
    }
    v
}

/// A `Vec<Option<usize>>` of length `n`, all `None`.
pub fn nones(n: usize) -> Vec<Option<usize>> {
    let mut v: Vec<Option<usize>> = Vec::new();
    let mut i: usize = 0;
    while i < n
        invariant
            i <= n,
        decreases n - i,
    {
        v.push(None);
        i = i + 1;
    }
    v
}

pub fn copy_bools(v: &Vec<bool>) -> Vec<bool> {
    let mut out: Vec<bool> = Vec::new();
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        out.push(v[i]);
        i = i + 1;
    }
    out
}

pub fn copy_usizes(v: &Vec<usize>) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut i: usize = 0;
    while i < v.len()
        invariant
            i <= v.len(),
        decreases v.len() - i,
    {
        out.push(v[i]);
        i = i + 1;
    }
    out
}

pub fn add_or_max(a: usize, b: usize) -> usize {
    match a.checked_add(b) {
        Some(s) => s,
        None => usize::MAX,
    }
}

// ---------------------------------------------------------------------------------------------
// Small copies (the non-recursive types)
// ---------------------------------------------------------------------------------------------

pub fn copy_lit(l: &Lit) -> Lit {
    match l {
        Lit::Bool(b) => Lit::Bool(*b),
        Lit::I8(x) => Lit::I8(*x),
        Lit::I32(x) => Lit::I32(*x),
        Lit::I64(x) => Lit::I64(*x),
        Lit::F32(x) => Lit::F32(*x),
        Lit::F64(x) => Lit::F64(*x),
    }
}

pub fn copy_f64x3(v: &F64x3) -> F64x3 {
    F64x3 { x: v.x, y: v.y, z: v.z }
}

pub fn copy_dims(d: &Dims) -> Dims {
    Dims { x: d.x, y: d.y, z: d.z }
}

pub fn copy_axis(a: &Axis) -> Axis {
    match a {
        Axis::X => Axis::X,
        Axis::Y => Axis::Y,
        Axis::Z => Axis::Z,
    }
}

pub fn copy_ambient(a: &Ambient) -> Ambient {
    match a {
        Ambient::Origin => Ambient::Origin,
        Ambient::Pos3 => Ambient::Pos3,
        Ambient::OriginScale => Ambient::OriginScale,
        Ambient::PositionScale => Ambient::PositionScale,
    }
}

pub fn copy_binop(op: &BinOp) -> BinOp {
    match op {
        BinOp::Add => BinOp::Add,
        BinOp::Subtract => BinOp::Subtract,
        BinOp::Multiply => BinOp::Multiply,
        BinOp::Divide => BinOp::Divide,
        BinOp::Equal => BinOp::Equal,
        BinOp::NotEqual => BinOp::NotEqual,
        BinOp::Less => BinOp::Less,
        BinOp::LessEqual => BinOp::LessEqual,
        BinOp::Greater => BinOp::Greater,
        BinOp::GreaterEqual => BinOp::GreaterEqual,
        BinOp::And => BinOp::And,
        BinOp::Or => BinOp::Or,
        BinOp::ShiftLeft => BinOp::ShiftLeft,
        BinOp::ShiftRight => BinOp::ShiftRight,
        BinOp::LogicalShiftRight => BinOp::LogicalShiftRight,
        BinOp::BitOr => BinOp::BitOr,
        BinOp::BitAnd => BinOp::BitAnd,
        BinOp::BitXor => BinOp::BitXor,
    }
}

pub fn copy_unop(op: &UnOp) -> UnOp {
    match op {
        UnOp::Negate => UnOp::Negate,
        UnOp::Not => UnOp::Not,
    }
}

pub fn copy_sel(s: &FieldSel) -> FieldSel {
    match s {
        FieldSel::Vec3(a) => FieldSel::Vec3(copy_axis(a)),
        FieldSel::Pos3(a) => FieldSel::Pos3(copy_axis(a)),
        FieldSel::Record(r, i) => FieldSel::Record(*r, *i),
    }
}

// ---------------------------------------------------------------------------------------------
// Recursive clones and type equality
// ---------------------------------------------------------------------------------------------

pub fn clone_type(t: &Type) -> Type
    decreases t,
{
    match t {
        Type::Bool => Type::Bool,
        Type::I8 => Type::I8,
        Type::I32 => Type::I32,
        Type::I64 => Type::I64,
        Type::F32 => Type::F32,
        Type::F64 => Type::F64,
        Type::Vec3 => Type::Vec3,
        Type::Pos3 => Type::Pos3,
        Type::Array(inner, n) => Type::Array(Box::new(clone_type(inner)), *n),
        Type::Record(r) => Type::Record(*r),
        Type::Handle(h) => Type::Handle(*h),
    }
}

pub fn clone_types(ts: &Vec<Type>) -> Vec<Type> {
    let mut out: Vec<Type> = Vec::new();
    let mut i: usize = 0;
    while i < ts.len()
        invariant
            i <= ts.len(),
        decreases ts.len() - i,
    {
        out.push(clone_type(&ts[i]));
        i = i + 1;
    }
    out
}

/// Structural type equality.
pub fn type_eq(a: &Type, b: &Type) -> bool
    decreases a,
{
    match (a, b) {
        (Type::Bool, Type::Bool) => true,
        (Type::I8, Type::I8) => true,
        (Type::I32, Type::I32) => true,
        (Type::I64, Type::I64) => true,
        (Type::F32, Type::F32) => true,
        (Type::F64, Type::F64) => true,
        (Type::Vec3, Type::Vec3) => true,
        (Type::Pos3, Type::Pos3) => true,
        (Type::Array(x, n), Type::Array(y, m)) => *n == *m && type_eq(x, y),
        (Type::Record(r), Type::Record(s)) => r.0 == s.0,
        (Type::Handle(x), Type::Handle(y)) => x.0 == y.0,
        _ => false,
    }
}

pub fn types_eq(a: &Vec<Type>, b: &Vec<Type>) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i: usize = 0;
    while i < a.len()
        invariant
            i <= a.len(),
            a.len() == b.len(),
        decreases a.len() - i,
    {
        if !type_eq(&a[i], &b[i]) {
            return false;
        }
        i = i + 1;
    }
    true
}

#[verifier::loop_isolation(false)]
pub fn clone_expr(e: &Expr) -> Expr
    decreases e,
{
    match e {
        Expr::Lit(l) => Expr::Lit(copy_lit(l)),
        Expr::Local(l) => Expr::Local(*l),
        Expr::Const(c) => Expr::Const(*c),
        Expr::Host(h) => Expr::Host(*h),
        Expr::Table(t) => Expr::Table(*t),
        Expr::Ambient(a) => Expr::Ambient(copy_ambient(a)),
        Expr::Density { input, index } => {
            let index = match index {
                Some(ix) => Some(Box::new(clone_expr(ix))),
                None => None,
            };
            Expr::Density { input: *input, index }
        },
        Expr::Call { func, args } => {
            let mut out: Vec<Expr> = Vec::new();
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                out.push(clone_expr(&args[i]));
                i = i + 1;
            }
            Expr::Call { func: *func, args: out }
        },
        Expr::ExternCall { ext, params, ret, args } => {
            let mut out: Vec<Expr> = Vec::new();
            let mut i: usize = 0;
            while i < args.len()
                invariant
                    i <= args.len(),
                decreases args.len() - i,
            {
                out.push(clone_expr(&args[i]));
                i = i + 1;
            }
            Expr::ExternCall { ext: *ext, params: clone_types(params), ret: clone_type(ret), args: out }
        },
        Expr::Binary { op, left, right } => Expr::Binary {
            op: copy_binop(op),
            left: Box::new(clone_expr(left)),
            right: Box::new(clone_expr(right)),
        },
        Expr::Unary { op, operand } => Expr::Unary {
            op: copy_unop(op),
            operand: Box::new(clone_expr(operand)),
        },
        Expr::Field { base, sel, ty } => Expr::Field {
            base: Box::new(clone_expr(base)),
            sel: copy_sel(sel),
            ty: clone_type(ty),
        },
        Expr::Index { array, index } => Expr::Index {
            array: Box::new(clone_expr(array)),
            index: Box::new(clone_expr(index)),
        },
        Expr::MakeVec3 { x, y, z } => Expr::MakeVec3 {
            x: Box::new(clone_expr(x)),
            y: Box::new(clone_expr(y)),
            z: Box::new(clone_expr(z)),
        },
        Expr::MakePos3 { x, y, z } => Expr::MakePos3 {
            x: Box::new(clone_expr(x)),
            y: Box::new(clone_expr(y)),
            z: Box::new(clone_expr(z)),
        },
        Expr::MakeRecord { record, fields } => {
            let mut out: Vec<FieldInit> = Vec::new();
            let mut i: usize = 0;
            while i < fields.len()
                invariant
                    i <= fields.len(),
                decreases fields.len() - i,
            {
                out.push(FieldInit { field: fields[i].field, value: clone_expr(&fields[i].value) });
                i = i + 1;
            }
            Expr::MakeRecord { record: *record, fields: out }
        },
        Expr::ArrayLit(es) => {
            let mut out: Vec<Expr> = Vec::new();
            let mut i: usize = 0;
            while i < es.len()
                invariant
                    i <= es.len(),
                decreases es.len() - i,
            {
                out.push(clone_expr(&es[i]));
                i = i + 1;
            }
            Expr::ArrayLit(out)
        },
        Expr::Cast { to, operand } => Expr::Cast {
            to: clone_type(to),
            operand: Box::new(clone_expr(operand)),
        },
    }
}

pub fn clone_field_init(f: &FieldInit) -> FieldInit {
    FieldInit { field: f.field, value: clone_expr(&f.value) }
}

#[verifier::loop_isolation(false)]
pub fn clone_stmts(ss: &Vec<Stmt>) -> Vec<Stmt>
    decreases ss,
{
    let mut out: Vec<Stmt> = Vec::new();
    let mut i: usize = 0;
    while i < ss.len()
        invariant
            i <= ss.len(),
        decreases ss.len() - i,
    {
        out.push(clone_stmt(&ss[i]));
        i = i + 1;
    }
    out
}

pub fn clone_stmt(s: &Stmt) -> Stmt
    decreases s,
{
    match s {
        Stmt::Assign { target, value } => Stmt::Assign { target: *target, value: clone_expr(value) },
        Stmt::Return(e) => Stmt::Return(clone_expr(e)),
        Stmt::If { cond, then_branch, else_branch } => Stmt::If {
            cond: clone_expr(cond),
            then_branch: clone_stmts(then_branch),
            else_branch: clone_stmts(else_branch),
        },
        Stmt::While { cond, body } => Stmt::While { cond: clone_expr(cond), body: clone_stmts(body) },
        Stmt::Repeat { count, body } => Stmt::Repeat { count: *count, body: clone_stmts(body) },
        Stmt::Break => Stmt::Break,
    }
}

} // verus!
