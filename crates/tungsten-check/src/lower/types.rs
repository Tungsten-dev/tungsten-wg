//! `VariableType` → core `Type`.

use tungsten_core::{RecordId, Type};
use tungsten_ir::VariableType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeFail {
    /// `VariableType::DensityInput`, possibly nested in an array.
    DensityInput,
    /// A record name the resolver doesn't know.
    UnknownRecord(&'static str),
}

/// Lowers a type, resolving record names with `record`. Stops at the first failure.
pub(crate) fn lower_type(
    t: &VariableType,
    record: &mut dyn FnMut(&'static str) -> Option<RecordId>,
) -> Result<Type, TypeFail> {
    Ok(match t {
        VariableType::DensityInput => return Err(TypeFail::DensityInput),
        VariableType::PermutationTable => Type::PermTable,
        VariableType::InterpolatedNoiseSampler => Type::Sampler,
        VariableType::Vec3 => Type::Vec3,
        VariableType::Pos3 => Type::Pos3,
        VariableType::F32 => Type::F32,
        VariableType::F64 => Type::F64,
        VariableType::I8 => Type::I8,
        VariableType::I32 => Type::I32,
        VariableType::I64 => Type::I64,
        VariableType::Bool => Type::Bool,
        VariableType::Array(elem, n) => Type::Array(Box::new(lower_type(elem, record)?), *n),
        VariableType::Extern(name) => {
            Type::Record(record(name).ok_or(TypeFail::UnknownRecord(name))?)
        }
    })
}

/// How types appear in error messages (`VariableType`'s `Debug`).
pub(crate) fn type_name(t: &VariableType) -> String {
    format!("{t:?}")
}
