pub mod builder;
pub mod dag;
pub mod model;
pub mod normalize;
pub mod pretty;

pub mod conversion;

pub use model::*;

/// Keeps the historical `crate::spmt::...` and `$crate::spmt::...` paths valid.
pub extern crate self as spmt;

pub fn try_derive_type<'a>(expr: &model::Expression<'a>) -> Option<model::VariableType> {
    match expr {
        model::Expression::Variable(var) => Some(var.t.clone()),
        model::Expression::Float(_) => Some(model::VariableType::F32), // or F64 depending on precision
        model::Expression::Double(_) => Some(model::VariableType::F64),
        model::Expression::Byte(_) => Some(model::VariableType::I8),
        model::Expression::Int(_) => Some(model::VariableType::I32),
        model::Expression::Long(_) => Some(model::VariableType::I64),
        model::Expression::Bool(_) => Some(model::VariableType::Bool),
        model::Expression::BinaryOp { op, left, right } => {
            let left_type = try_derive_type(left);
            let right_type = try_derive_type(right);
            try_derive_binop_type(*op, left_type, right_type)
        }
        model::Expression::UnaryOp { operand, .. } => try_derive_type(operand),
        model::Expression::Field { type_of_field, .. } => Some(type_of_field.clone()),
        model::Expression::FunctionCall { function, .. } => Some(function.return_type.clone()),
        model::Expression::ExternCall { return_type, .. } => Some(return_type.clone()),
        model::Expression::DensityVariable(_, _) => Some(model::VariableType::DensityInput),
        model::Expression::PermutationTable(_) => Some(model::VariableType::PermutationTable),
        model::Expression::Construct { t, .. } => Some(t.clone()),
        model::Expression::ArrayAccess { array, index: _ } => {
            let array_type: VariableType = try_derive_type(array)?;
            match array_type {
                model::VariableType::Vec3 => Some(model::VariableType::F64),
                model::VariableType::Pos3 => Some(model::VariableType::F64),
                model::VariableType::DensityInput => Some(model::VariableType::F64), // Assuming density inputs are arrays of floats
                _ => None, // For other array types, we would need more information to derive the element type
            }
        }
        model::Expression::ConstructExtern { t, args: _ } => Some(t.clone()), // Assume the type is determined by the construct extern declaration
        model::Expression::ArrayLiteral(_) => None,
        model::Expression::ExplicitCast { to, expr: _ } => Some(to.clone()),
    }
}

pub fn try_derive_binop_type(
    op: model::BinaryOperator,
    left_type: Option<model::VariableType>,
    right_type: Option<model::VariableType>,
) -> Option<model::VariableType> {
    match op {
        model::BinaryOperator::Add
        | model::BinaryOperator::Subtract
        | model::BinaryOperator::Multiply
        | model::BinaryOperator::Divide => {
            match (&left_type, &right_type) {
                (Some(model::VariableType::Vec3), Some(model::VariableType::Pos3))
                | (Some(model::VariableType::Pos3), Some(model::VariableType::Vec3)) => {
                    Some(model::VariableType::Vec3)
                }

                (Some(model::VariableType::I32), Some(model::VariableType::Bool))
                | (Some(model::VariableType::Bool), Some(model::VariableType::I32)) => {
                    Some(model::VariableType::I32)
                } // Allow bool to be treated as 0/1 in arithmetic

                _ if left_type == right_type => left_type,
                _ => None, // Types don't match and we don't know how to convert
            }
        }

        model::BinaryOperator::Equal
        | model::BinaryOperator::NotEqual
        | model::BinaryOperator::Less
        | model::BinaryOperator::LessEqual
        | model::BinaryOperator::Greater
        | model::BinaryOperator::GreaterEqual
        | model::BinaryOperator::And
        | model::BinaryOperator::Or => Some(model::VariableType::Bool), // Comparisons and logical ops result in bool

        _ => None, // Other operators not handled for type conversion
    }
}
