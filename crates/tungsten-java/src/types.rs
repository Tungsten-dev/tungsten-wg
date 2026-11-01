use tungsten_interface::naming::java_sanitize;
use crate::model as java;
use tungsten_ir::model::VariableType;
use tungsten_ir::model as opt;

use super::{BASE3D_NOISE_SAMPLER_CLASS, PERLIN_NOISE_SAMPLER_CLASS};

/// Java type of a value. Panics on `DensityInput`: density inputs are read through buffers.
pub fn convert_type(t: &VariableType) -> java::Type {
    use java::Type as J;
    match t {
        VariableType::DensityInput => panic!("DensityInput type should not be directly converted"),
        VariableType::Vec3 => J::class("Vec3"),
        VariableType::Pos3 => J::class("Pos3"),
        VariableType::F32 => J::Float,
        VariableType::F64 => J::Double,
        VariableType::I8 => J::Byte,
        VariableType::I32 => J::Int,
        VariableType::I64 => J::Long,
        VariableType::Bool => J::Boolean,
        VariableType::PermutationTable => J::class(PERLIN_NOISE_SAMPLER_CLASS),
        VariableType::InterpolatedNoiseSampler => J::class(BASE3D_NOISE_SAMPLER_CLASS),
        VariableType::Extern(name) => J::Class(java_sanitize(name)),
        VariableType::Array(element, _) => J::Array(Box::new(convert_type(element))),
    }
}

/// Element type of a unit's buffer: the return type of its main function.
pub fn unit_return_type(program: &opt::OptimizedProgram, cu: opt::Ref<opt::ComputeUnit>) -> &VariableType {
    let opt::ComputeUnit::DensityFunction { function, .. } = &program.compute_units[cu];
    &program.functions[*function].return_type
}

/// The program's density input for `r`; inputs live in the units, so every unit is searched.
pub fn find_density_input(program: &opt::OptimizedProgram, r: opt::Ref<opt::DensityInput>) -> &opt::DensityInput {
    for (_, cu) in program.compute_units.iter() {
        let opt::ComputeUnit::DensityFunction { density_inputs, .. } = cu;
        if density_inputs.contains(r) {
            return &density_inputs[r];
        }
    }
    panic!("density input {:?} belongs to no compute unit", r)
}

/// Whether values of `t` are Java objects (references) rather than primitives.
pub fn is_object(t: &VariableType) -> bool {
    !convert_type(t).is_primitive()
}

/// Box class for `Ref(false)` on a primitive.
pub fn boxed_class(t: &java::Type) -> &'static str {
    match t {
        java::Type::Boolean => "Boolean",
        java::Type::Byte => "Byte",
        java::Type::Int => "Integer",
        java::Type::Long => "Long",
        java::Type::Float => "Float",
        java::Type::Double => "Double",
        other => panic!("no box class for {:?}", other),
    }
}

/// Runtime holder class for `Ref(true)` on a primitive.
pub fn holder_class(t: &java::Type) -> &'static str {
    match t {
        java::Type::Boolean => "BoolRef",
        java::Type::Byte => "ByteRef",
        java::Type::Int => "IntRef",
        java::Type::Long => "LongRef",
        java::Type::Float => "FloatRef",
        java::Type::Double => "DoubleRef",
        other => panic!("no holder class for {:?}", other),
    }
}

/// Type of a permutation table field in `AutoConsts`.
pub fn auto_const_type(table: &opt::PermutationTableInput) -> java::Type {
    match table {
        opt::PermutationTableInput::PerlinNoise { .. } => java::Type::class(PERLIN_NOISE_SAMPLER_CLASS),
        opt::PermutationTableInput::Base3DNoise => java::Type::class(BASE3D_NOISE_SAMPLER_CLASS),
    }
}

pub fn convert_binary_op(op: opt::BinaryOperator) -> java::BinaryOperator {
    use java::BinaryOperator as D;
    use opt::BinaryOperator as S;
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
        S::ShiftLeft => D::ShiftLeft,
        S::ShiftRight => D::ShiftRight,
        S::LogicalShiftRight => D::UnsignedShiftRight,
        S::BitOr => D::BitOr,
        S::BitAnd => D::BitAnd,
        S::BitXor => D::BitXor,
    }
}

/// The literal `v` in Java type `t` (used for `bool as <numeric>`).
pub fn numeric_literal(t: &java::Type, v: i32) -> java::Expression {
    match t {
        java::Type::Byte => java::Expression::cast(java::Type::Byte, java::Expression::IntLit(v)),
        java::Type::Long => java::Expression::LongLit(v as i64),
        java::Type::Float => java::Expression::FloatLit(v as f32),
        java::Type::Double => java::Expression::DoubleLit(v as f64),
        _ => java::Expression::IntLit(v),
    }
}
