//! Type spellings and the type derivation rules of SPEC §6.3.

use tungsten_ir::{BinaryOperator, ConstExpr, Expression, VariableType};

use super::Env;
use crate::ast::{Ty, TyKind};
use crate::intern::intern;
use crate::tables::{OpClass, op_class};

pub fn var_type(t: &Ty) -> VariableType {
    match &t.kind {
        TyKind::Prim(v) => v.clone(),
        TyKind::Array(e, n) => VariableType::Array(Box::new(var_type(e)), *n),
        TyKind::Extern(name) => VariableType::Extern(intern(name)),
    }
}

/// The type of field `name` of a `base`. Only extern record fields need the `Env`, which is why
/// canonical text (which writes every field type) doesn't.
pub fn field_type(base: &VariableType, name: &str, env: &dyn Env) -> Option<VariableType> {
    match base {
        VariableType::Vec3 if matches!(name, "x" | "y" | "z") => Some(VariableType::F64),
        VariableType::Pos3 if matches!(name, "x" | "y" | "z") => Some(VariableType::I32),
        VariableType::Extern(r) => env
            .extern_type(r)?
            .into_iter()
            .find(|(f, _)| *f == name)
            .map(|(_, t)| t),
        _ => None,
    }
}

pub fn index_type(base: &VariableType) -> Option<VariableType> {
    match base {
        VariableType::Array(t, _) => Some((**t).clone()),
        VariableType::Vec3 => Some(VariableType::F64),
        VariableType::Pos3 => Some(VariableType::I32),
        _ => None,
    }
}

pub fn binary_type(
    op: BinaryOperator,
    l: Option<&VariableType>,
    r: Option<&VariableType>,
) -> Option<VariableType> {
    use VariableType::{Pos3, Vec3};
    match (op_class(op), l, r) {
        (OpClass::Cmp | OpClass::Logic, _, _) => Some(VariableType::Bool),
        (OpClass::Shift, l, _) => l.cloned(),
        (OpClass::Bit | OpClass::Arith, Some(a), Some(b)) if a == b => Some(a.clone()),
        (OpClass::Arith, Some(Vec3), Some(Pos3)) | (OpClass::Arith, Some(Pos3), Some(Vec3)) => {
            Some(Vec3)
        }
        _ => None,
    }
}

/// `Expression` → `ConstExpr`, for the subset a constant may use.
pub fn to_const(e: Expression) -> Option<ConstExpr> {
    let b = |e: Box<Expression>| to_const(*e).map(Box::new);
    Some(match e {
        Expression::Constant(c) => ConstExpr::Constant(c),
        Expression::Float(v) => ConstExpr::Float(v),
        Expression::Double(v) => ConstExpr::Double(v),
        Expression::Byte(v) => ConstExpr::Byte(v),
        Expression::Int(v) => ConstExpr::Int(v),
        Expression::Long(v) => ConstExpr::Long(v),
        Expression::Bool(v) => ConstExpr::Bool(v),
        Expression::BinaryOp { op, left, right } => ConstExpr::BinaryOp {
            op,
            left: b(left)?,
            right: b(right)?,
        },
        Expression::UnaryOp { op, operand } => ConstExpr::UnaryOp {
            op,
            operand: b(operand)?,
        },
        Expression::ExplicitCast { to, expr } => ConstExpr::ExplicitCast { to, expr: b(expr)? },
        Expression::Field {
            base,
            field,
            type_of_field,
            base_type,
        } => ConstExpr::Field {
            base: b(base)?,
            field,
            type_of_field,
            base_type,
        },
        Expression::ArrayAccess { array, index } => ConstExpr::ArrayAccess {
            array: b(array)?,
            index: b(index)?,
        },
        Expression::ArrayLiteral(items) => {
            ConstExpr::ArrayLiteral(items.into_iter().map(to_const).collect::<Option<_>>()?)
        }
        Expression::Construct { t, args } => ConstExpr::Construct {
            t,
            args: args.into_iter().map(to_const).collect::<Option<_>>()?,
        },
        _ => return None,
    })
}
