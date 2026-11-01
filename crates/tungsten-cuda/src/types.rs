/*
IR type -> CUDA type mapping (device and public mirror types), operator mapping and the helpers
for Java integer semantics (wrapping arithmetic, masked shifts).
*/

use tungsten_interface::naming::cuda_sanitize;
use tungsten_ir::model as opt;
use tungsten_ir::model::VariableType;

use crate::model as cuda;
use crate::{BASE3D_NOISE_SAMPLER_TYPE, PERLIN_NOISE_SAMPLER_TYPE};

/// Device type of a value. Panics on `DensityInput`: density inputs are read through buffers.
pub fn device_type(t: &VariableType) -> cuda::Type {
    use cuda::Type as T;
    match t {
        VariableType::DensityInput => panic!("DensityInput type should not be directly converted"),
        VariableType::Vec3 => T::named("double3"),
        VariableType::Pos3 => T::named("int3"),
        VariableType::F32 => T::Float,
        VariableType::F64 => T::Double,
        VariableType::I8 => T::SignedChar,
        VariableType::I32 => T::Int,
        VariableType::I64 => T::LongLong,
        VariableType::Bool => T::Bool,
        VariableType::PermutationTable => T::const_ptr(T::named(PERLIN_NOISE_SAMPLER_TYPE)),
        VariableType::InterpolatedNoiseSampler => T::const_ptr(T::named(BASE3D_NOISE_SAMPLER_TYPE)),
        VariableType::Extern(name) => T::Named(format!("tw::{}", cuda_sanitize(name))),
        VariableType::Array(element, n) => T::Array(Box::new(device_type(element)), *n),
    }
}

/// Type of the same value in the CUDA-free public header, if it has one. Arrays become C arrays,
/// which are only valid as declarators.
pub fn public_type(t: &VariableType) -> Option<cuda::Type> {
    use cuda::Type as T;
    Some(match t {
        VariableType::Vec3 => T::named("Vec3"),
        VariableType::Pos3 => T::named("Pos3"),
        VariableType::F32 => T::Float,
        VariableType::F64 => T::Double,
        VariableType::I8 => T::SignedChar,
        VariableType::I32 => T::Int,
        VariableType::I64 => T::LongLong,
        VariableType::Bool => T::Bool,
        VariableType::Array(element, n) => T::CArray(Box::new(public_type(element)?), *n),
        VariableType::Extern(name) => T::named(&format!("tw::{}", cuda_sanitize(name))),
        _ => return None,
    })
}

/// Owned type of a permutation table field in `AutoConsts` (parameters point to it instead).
pub fn auto_const_type(table: &opt::PermutationTableInput) -> cuda::Type {
    match table {
        opt::PermutationTableInput::PerlinNoise { .. } => {
            cuda::Type::named(PERLIN_NOISE_SAMPLER_TYPE)
        }
        opt::PermutationTableInput::Base3DNoise => cuda::Type::named(BASE3D_NOISE_SAMPLER_TYPE),
    }
}

/// Element type of a unit's buffer: the return type of its main function.
pub fn unit_return_type(
    program: &opt::OptimizedProgram,
    cu: opt::Ref<opt::ComputeUnit>,
) -> &VariableType {
    let opt::ComputeUnit::DensityFunction { function, .. } = &program.compute_units[cu];
    &program.functions[*function].return_type
}

/// The program's density input for `r`; inputs live in the units, so every unit is searched.
pub fn find_density_input(
    program: &opt::OptimizedProgram,
    r: opt::Ref<opt::DensityInput>,
) -> &opt::DensityInput {
    for (_, cu) in program.compute_units.iter() {
        let opt::ComputeUnit::DensityFunction { density_inputs, .. } = cu;
        if density_inputs.contains(r) {
            return &density_inputs[r];
        }
    }
    panic!("density input {:?} belongs to no compute unit", r)
}

/// Whether the device representation of `t` is a pointer (samplers).
pub fn is_pointer(t: &VariableType) -> bool {
    matches!(
        t,
        VariableType::PermutationTable | VariableType::InterpolatedNoiseSampler
    )
}

pub fn is_vector(t: &VariableType) -> bool {
    matches!(t, VariableType::Vec3 | VariableType::Pos3)
}

pub fn is_float(t: &VariableType) -> bool {
    matches!(t, VariableType::F32 | VariableType::F64)
}

pub fn is_integer(t: &VariableType) -> bool {
    matches!(t, VariableType::I8 | VariableType::I32 | VariableType::I64)
}

/// Types that C-style casts convert between.
pub fn is_scalar(t: &VariableType) -> bool {
    matches!(
        t,
        VariableType::F32
            | VariableType::F64
            | VariableType::I8
            | VariableType::I32
            | VariableType::I64
            | VariableType::Bool
    )
}

/// Width of an integer operation with Java semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    /// Computed in 32 bits (C++ promotes `signed char`), truncated to 8 bits.
    I8,
    I32,
    I64,
}

impl IntWidth {
    pub fn of(t: &VariableType) -> Option<IntWidth> {
        match t {
            VariableType::I8 => Some(IntWidth::I8),
            VariableType::I32 => Some(IntWidth::I32),
            VariableType::I64 => Some(IntWidth::I64),
            _ => None,
        }
    }

    pub fn signed(self) -> cuda::Type {
        match self {
            IntWidth::I8 => cuda::Type::SignedChar,
            IntWidth::I32 => cuda::Type::Int,
            IntWidth::I64 => cuda::Type::LongLong,
        }
    }

    pub fn unsigned(self) -> cuda::Type {
        match self {
            IntWidth::I8 => cuda::Type::Unsigned,
            IntWidth::I32 => cuda::Type::Unsigned,
            IntWidth::I64 => cuda::Type::UnsignedLongLong,
        }
    }

    /// Shift counts are taken modulo the width, as in Java.
    pub fn shift_mask(self) -> i64 {
        match self {
            IntWidth::I8 | IntWidth::I32 => 31,
            IntWidth::I64 => 63,
        }
    }
}

/// How an arithmetic operation on operands of these derived types is lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    /// Integer arithmetic of this width (wrapping).
    Integer(IntWidth),
    /// Floating point, vectors or anything else C++ already handles.
    Plain,
    /// Neither operand type could be derived.
    Unknown,
}

pub fn operand_kind(left: Option<&VariableType>, right: Option<&VariableType>) -> OperandKind {
    let types = [left, right];
    let known = types.iter().flatten();
    if known.clone().any(|t| is_float(t) || is_vector(t)) {
        return OperandKind::Plain;
    }
    if known.clone().any(|t| **t == VariableType::I64) {
        return OperandKind::Integer(IntWidth::I64);
    }
    if known.clone().any(|t| **t == VariableType::I32) {
        return OperandKind::Integer(IntWidth::I32);
    }
    if known.clone().any(|t| **t == VariableType::I8) {
        return OperandKind::Integer(IntWidth::I8);
    }
    if left.is_none() && right.is_none() {
        return OperandKind::Unknown;
    }
    OperandKind::Plain
}

/// The plain C++ operator for an IR operator. `LogicalShiftRight` maps to `>>`; callers make the
/// operand unsigned first.
pub fn convert_binary_op(op: opt::BinaryOperator) -> cuda::BinaryOperator {
    use cuda::BinaryOperator as D;
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
        S::ShiftRight | S::LogicalShiftRight => D::ShiftRight,
        S::BitOr => D::BitOr,
        S::BitAnd => D::BitAnd,
        S::BitXor => D::BitXor,
    }
}
