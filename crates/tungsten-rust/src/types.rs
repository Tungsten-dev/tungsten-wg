use crate::model as rcl;
use tungsten_interface::naming::sanitize_name;
use tungsten_ir::model as opt;
use tungsten_ir::model::VariableType;

use super::{BASE3D_NOISE_SAMPLER_STRUCT_NAME, PERLIN_NOISE_SAMPLER_STRUCT_NAME};

/// Value type of a variable. Panics on `DensityInput`: density inputs are read through buffers.
pub fn convert_type(t: &VariableType) -> rcl::Type {
    match t {
        VariableType::DensityInput => panic!("DensityInput type should not be directly converted"),
        VariableType::Vec3 => rcl::Type::Struct("Vec3".to_string()),
        VariableType::Pos3 => rcl::Type::Struct("Pos3".to_string()),
        VariableType::F32 => rcl::Type::F32,
        VariableType::F64 => rcl::Type::F64,
        VariableType::I8 => rcl::Type::I8,
        VariableType::I32 => rcl::Type::I32,
        VariableType::I64 => rcl::Type::I64,
        VariableType::Bool => rcl::Type::Bool,
        VariableType::PermutationTable => rcl::Type::Ref(Box::new(rcl::Type::Struct(
            PERLIN_NOISE_SAMPLER_STRUCT_NAME.to_string(),
        ))),
        VariableType::InterpolatedNoiseSampler => rcl::Type::Ref(Box::new(rcl::Type::Struct(
            BASE3D_NOISE_SAMPLER_STRUCT_NAME.to_string(),
        ))),
        VariableType::Extern(name) => rcl::Type::Struct(sanitize_name(name)),
        VariableType::Array(element, size) => {
            rcl::Type::Array(Box::new(convert_type(element)), *size)
        }
    }
}

/// Element type of an `InternalBuffers` field for a density function with this return type.
// pub fn buffer_element_type(return_type: &VariableType) -> rcl::Type {
//     convert_type(return_type)
// }

/// Owned type of a permutation table field in `AutoConsts` (parameters borrow it instead).
pub fn auto_const_type(table: &opt::PermutationTableInput) -> rcl::Type {
    match table {
        opt::PermutationTableInput::PerlinNoise { .. } => {
            rcl::Type::Struct(PERLIN_NOISE_SAMPLER_STRUCT_NAME.to_string())
        }
        opt::PermutationTableInput::Base3DNoise => {
            rcl::Type::Struct(BASE3D_NOISE_SAMPLER_STRUCT_NAME.to_string())
        }
    }
}

pub fn convert_binary_op(op: opt::BinaryOperator) -> rcl::BinaryOperator {
    use opt::BinaryOperator as S;
    use rcl::BinaryOperator as D;
    match op {
        S::Add => D::Add,
        S::Subtract => D::Subtract,
        S::Multiply => D::Multiply,
        S::Divide => D::Divide,
        S::Equal => D::Equal,
        S::NotEqual => D::NotEqual,
        S::Less => D::Less,
        S::LessEqual => D::LessEqual,
        S::Greater => D::Greater,
        S::GreaterEqual => D::GreaterEqual,
        S::And => D::And,
        S::Or => D::Or,
        S::ShiftLeft => D::LeftShift,
        S::ShiftRight => D::RightShift,
        S::LogicalShiftRight => D::RightShift,
        S::BitOr => D::BitwiseOr,
        S::BitAnd => D::BitwiseAnd,
        S::BitXor => D::BitwiseXor,
    }
}

pub fn convert_unary_op(op: opt::UnaryOperator) -> rcl::UnaryOperator {
    match op {
        opt::UnaryOperator::Negate => rcl::UnaryOperator::Negate,
        opt::UnaryOperator::Not => rcl::UnaryOperator::Not,
        opt::UnaryOperator::Ref(m) => rcl::UnaryOperator::Ref(m),
    }
}

/// Element type when indexing a value of type `array`; `None` if unknown.
pub fn element_type(array: &VariableType) -> Option<VariableType> {
    match array {
        VariableType::Vec3 | VariableType::Pos3 | VariableType::DensityInput => {
            Some(VariableType::F64)
        }
        VariableType::Array(t, _) => Some((**t).clone()),
        _ => None,
    }
}

/// Result type of a binary operation; mirrors `spmt::try_derive_binop_type`.
pub fn derive_binop_type(
    op: opt::BinaryOperator,
    left: Option<VariableType>,
    right: Option<VariableType>,
) -> Option<VariableType> {
    use opt::BinaryOperator as B;
    match op {
        B::Add | B::Subtract | B::Multiply | B::Divide => match (&left, &right) {
            (Some(VariableType::Vec3), Some(VariableType::Pos3))
            | (Some(VariableType::Pos3), Some(VariableType::Vec3)) => Some(VariableType::Vec3),
            (Some(VariableType::I32), Some(VariableType::Bool))
            | (Some(VariableType::Bool), Some(VariableType::I32)) => Some(VariableType::I32),
            _ if left == right => left,
            _ => None,
        },
        B::Equal
        | B::NotEqual
        | B::Less
        | B::LessEqual
        | B::Greater
        | B::GreaterEqual
        | B::And
        | B::Or => Some(VariableType::Bool),
        _ => None,
    }
}

/// Type of a constant initializer; `None` if it cannot be derived.
pub fn derive_const_type(
    program: &opt::OptimizedProgram,
    e: &opt::ConstExpr,
) -> Option<VariableType> {
    use opt::ConstExpr as C;
    match e {
        C::Constant(r) => derive_const_type(program, &program.constants[*r].value),
        C::Float(_) => Some(VariableType::F32),
        C::Double(_) => Some(VariableType::F64),
        C::Byte(_) => Some(VariableType::I8),
        C::Int(_) => Some(VariableType::I32),
        C::Long(_) => Some(VariableType::I64),
        C::Bool(_) => Some(VariableType::Bool),
        C::BinaryOp { op, left, right } => derive_binop_type(
            *op,
            derive_const_type(program, left),
            derive_const_type(program, right),
        ),
        C::UnaryOp { operand, .. } => derive_const_type(program, operand),
        C::ExplicitCast { to, .. } => Some(to.clone()),
        C::Field { type_of_field, .. } => Some(type_of_field.clone()),
        C::ArrayAccess { array, .. } => element_type(&derive_const_type(program, array)?),
        C::ArrayLiteral(items) => {
            let first = derive_const_type(program, items.first()?)?;
            Some(VariableType::Array(Box::new(first), items.len()))
        }
        C::Construct { t, .. } => Some(t.clone()),
    }
}
