//! `VariableType` → core `Type`.

use tungsten_core::{HandleTypeId, RecordId, Type};
use tungsten_ir::VariableType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeFail {
    /// A record name the resolver doesn't know.
    UnknownRecord(&'static str),
}

/// Resolves the names a type mentions.
pub(crate) trait TypeNames {
    fn record(&mut self, name: &'static str) -> Option<RecordId>;
    /// Handle names are interned in order of first appearance.
    fn handle(&mut self, name: &'static str) -> HandleTypeId;
}

/// Lowers a type, resolving names with `names`. Stops at the first failure.
pub(crate) fn lower_type(t: &VariableType, names: &mut dyn TypeNames) -> Result<Type, TypeFail> {
    Ok(match t {
        VariableType::Handle(name) => Type::Handle(names.handle(name)),
        VariableType::Vec3 => Type::Vec3,
        VariableType::Pos3 => Type::Pos3,
        VariableType::F32 => Type::F32,
        VariableType::F64 => Type::F64,
        VariableType::I8 => Type::I8,
        VariableType::I32 => Type::I32,
        VariableType::I64 => Type::I64,
        VariableType::Bool => Type::Bool,
        VariableType::Array(elem, n) => Type::Array(Box::new(lower_type(elem, names)?), *n),
        VariableType::Extern(name) => {
            Type::Record(names.record(name).ok_or(TypeFail::UnknownRecord(name))?)
        }
    })
}

/// How types appear in error messages (`VariableType`'s `Debug`).
pub(crate) fn type_name(t: &VariableType) -> String {
    format!("{t:?}")
}
