//! Autoconst arguments for the interpreter: `resolved_auto_consts` binds auto params by name and
//! hands the frontend plain data to build its runtime objects from.

use tungsten_ir::VariableType;

use crate::map::{AutoArgSpec, AutoCoreId, LowerMap};

/// A value of an auto param, or an evaluated autoconst argument.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoArgValue {
    I64(i64),
    F64(f64),
    Bool(bool),
}

impl AutoArgValue {
    fn type_name(&self) -> &'static str {
        match self {
            AutoArgValue::I64(_) => "i64",
            AutoArgValue::F64(_) => "f64",
            AutoArgValue::Bool(_) => "bool",
        }
    }

    fn has_type(&self, t: &VariableType) -> bool {
        matches!(
            (self, t),
            (AutoArgValue::I64(_), VariableType::I64)
                | (AutoArgValue::F64(_), VariableType::F64)
                | (AutoArgValue::Bool(_), VariableType::Bool)
        )
    }
}

/// One autoconst with its arguments evaluated: params are replaced by their values.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAutoConst<'a> {
    /// Where the core program keeps it.
    pub core: AutoCoreId,
    pub name: &'a str,
    pub t: &'a VariableType,
    pub initializer: &'a str,
    pub args: Vec<AutoArgValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParamBindError {
    #[error("the program has no auto param `{0}`")]
    Unknown(String),
    #[error("no value for auto param `{0}`")]
    Missing(String),
    #[error("auto param `{0}` is given more than once")]
    Duplicate(String),
    #[error("auto param `{name}` is `{expected}`, but the value is `{found}`")]
    WrongType { name: String, expected: String, found: String },
}

impl LowerMap {
    /// Every autoconst, in arena order, read or not. Every declared param must be bound exactly
    /// once, by name, to a value of its type.
    pub fn resolved_auto_consts(
        &self,
        params: &[(String, AutoArgValue)],
    ) -> Result<Vec<ResolvedAutoConst<'_>>, ParamBindError> {
        let mut values: Vec<Option<&AutoArgValue>> = vec![None; self.auto_params.len()];
        for (name, v) in params {
            let (id, _, _) = self
                .auto_params
                .iter()
                .find(|(_, _, n)| n == name)
                .ok_or_else(|| ParamBindError::Unknown(name.clone()))?;
            if values[id.0].is_some() {
                return Err(ParamBindError::Duplicate(name.clone()));
            }
            let t = &self.auto_param_types[id.0];
            if !v.has_type(t) {
                return Err(ParamBindError::WrongType {
                    name: name.clone(),
                    expected: format!("{t:?}"),
                    found: v.type_name().to_string(),
                });
            }
            values[id.0] = Some(v);
        }
        let bound = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.cloned().ok_or_else(|| {
                    ParamBindError::Missing(
                        self.auto_params
                            .name(crate::map::AutoParamId(i))
                            .unwrap_or("?")
                            .to_string(),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self
            .auto_consts
            .iter()
            .map(|a| ResolvedAutoConst {
                core: a.core,
                name: &a.name,
                t: &a.t,
                initializer: &a.initializer,
                args: a
                    .args
                    .iter()
                    .map(|arg| match arg {
                        AutoArgSpec::I64(v) => AutoArgValue::I64(*v),
                        AutoArgSpec::Param(p) => bound[p.0].clone(),
                    })
                    .collect(),
            })
            .collect())
    }
}
