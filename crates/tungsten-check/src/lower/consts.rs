//! `ConstExpr` → `Expression`, so constant initializers go through the expression walker.

use tungsten_ir::{ConstExpr, Expression};

/// `ConstExpr` is a subset of `Expression`; constants lower and synthesize through this.
pub(crate) fn const_to_expr(c: &ConstExpr) -> Expression {
    let b = |e: &ConstExpr| Box::new(const_to_expr(e));
    match c {
        ConstExpr::Constant(r) => Expression::Constant(*r),
        ConstExpr::Float(v) => Expression::Float(*v),
        ConstExpr::Double(v) => Expression::Double(*v),
        ConstExpr::Byte(v) => Expression::Byte(*v),
        ConstExpr::Int(v) => Expression::Int(*v),
        ConstExpr::Long(v) => Expression::Long(*v),
        ConstExpr::Bool(v) => Expression::Bool(*v),
        ConstExpr::BinaryOp { op, left, right } => Expression::BinaryOp {
            op: *op,
            left: b(left),
            right: b(right),
        },
        ConstExpr::UnaryOp { op, operand } => Expression::UnaryOp {
            op: *op,
            operand: b(operand),
        },
        ConstExpr::ExplicitCast { to, expr } => Expression::ExplicitCast {
            to: to.clone(),
            expr: b(expr),
        },
        ConstExpr::Field {
            base,
            field,
            type_of_field,
            base_type,
        } => Expression::Field {
            base: b(base),
            field: field.clone(),
            type_of_field: type_of_field.clone(),
            base_type: base_type.clone(),
        },
        ConstExpr::ArrayAccess { array, index } => Expression::ArrayAccess {
            array: b(array),
            index: b(index),
        },
        ConstExpr::ArrayLiteral(elems) => {
            Expression::ArrayLiteral(elems.iter().map(const_to_expr).collect())
        }
        ConstExpr::Construct { t, args } => Expression::Construct {
            t: t.clone(),
            args: args.iter().map(const_to_expr).collect(),
        },
    }
}
