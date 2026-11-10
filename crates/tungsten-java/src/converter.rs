/*
Per-function conversion state: variable naming, type derivation, and statements hoisted out of
expressions (holders for `Ref(true)` on primitives). Shared by expression.rs, statement.rs and function.rs.
*/

use std::collections::{HashMap, HashSet};

use super::types::{convert_type, find_density_input, unit_return_type};
use tungsten_rust::types::{derive_binop_type, derive_const_type, element_type};
use tungsten_interface::Interface;
use tungsten_interface::naming::{NamingRules, java_sanitize};
use crate::model as java;
use tungsten_ir::model::{Name, VariableType};
use tungsten_ir::model::{
    Constant, Expression, Function, OptimizedProgram, PermutationTableInput, Ref, Var,
};

/// Converts one function body. Collects the helpers and constants it references so the
/// caller can generate them afterwards.
pub struct FunctionConverter<'a> {
    pub program: &'a OptimizedProgram,
    pub naming: &'a Interface,
    /// The generated top-level class; constants are always qualified with it.
    pub class_name: &'a str,
    pub function: &'a Function,
    /// Helper functions referenced by the converted code, still to be generated.
    pub called: Vec<Ref<Function>>,
    /// Constants referenced by the converted code, still to be generated.
    pub used_constants: Vec<Ref<Constant>>,
    /// Statements that must run before the statement being converted.
    pub pre: Vec<java::Statement>,
    /// Statements that must run after it (write-backs of `Ref(true)` holders).
    pub post: Vec<java::Statement>,
    /// Variables written back by `post`.
    pub written_back: Vec<Ref<Var>>,
    vars: HashMap<Ref<Var>, String>,
    used_names: HashSet<String>,
}

impl<'a> FunctionConverter<'a> {
    pub fn new(
        program: &'a OptimizedProgram,
        naming: &'a Interface,
        rules: &NamingRules,
        class_name: &'a str,
        function: &'a Function,
    ) -> Self {
        Self {
            program,
            naming,
            class_name,
            function,
            called: Vec::new(),
            used_constants: Vec::new(),
            pre: Vec::new(),
            post: Vec::new(),
            written_back: Vec::new(),
            vars: HashMap::new(),
            used_names: rules.used_set(),
        }
    }

    /// `base`, or `base_N` for the first free N; keeps names unique within the function.
    pub fn fresh_name(&mut self, base: &str) -> String {
        let base = java_sanitize(base);
        if self.used_names.insert(base.clone()) {
            return base;
        }
        let mut n = 1;
        loop {
            let candidate = format!("{}_{}", base, n);
            if self.used_names.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }

    /// The Java local for `r`, named on first use.
    pub fn declare_var(&mut self, r: Ref<Var>) -> String {
        if let Some(v) = self.vars.get(&r) {
            return v.clone();
        }
        let base = match &self.function.variables[r].name {
            Name::Named(n) | Name::Prefixed(n) => n.clone(),
            Name::Anonymous => "var".to_string(),
        };
        let name = self.fresh_name(&base);
        self.vars.insert(r, name.clone());
        name
    }

    pub fn var_type(&self, r: Ref<Var>) -> &'a VariableType {
        let function: &'a Function = self.function;
        &function.variables[r].t
    }

    pub fn java_var_type(&self, r: Ref<Var>) -> java::Type {
        convert_type(self.var_type(r))
    }

    /// Type of `expr` if it can be derived; used to decide where casts are needed.
    pub fn derive_type(&self, expr: &Expression) -> Option<VariableType> {
        use Expression as E;
        Some(match expr {
            E::Constant(r) => derive_const_type(self.program, &self.program.constants[*r].value)?,
            E::Variable(r) => self.function.variables[*r].t.clone(),
            E::Float(_) => VariableType::F32,
            E::Double(_) => VariableType::F64,
            E::Byte(_) => VariableType::I8,
            E::Int(_) => VariableType::I32,
            E::Long(_) => VariableType::I64,
            E::Bool(_) => VariableType::Bool,
            E::FunctionCall { function, .. } => self.program.functions[*function].return_type.clone(),
            E::ExternCall { return_type, .. } => return_type.clone(),
            E::DensityVariable(input, _) => {
                unit_return_type(self.program, find_density_input(self.program, *input).density_function).clone()
            }
            E::PermutationTable(r) => match &self.program.permutation_tables[*r] {
                PermutationTableInput::PerlinNoise { .. } => VariableType::PermutationTable,
                PermutationTableInput::Base3DNoise => VariableType::InterpolatedNoiseSampler,
            },
            E::HostInput(r) => self.program.host_inputs[*r].get_type(),
            E::Ambient(k) => match k {
                tungsten_ir::model::AmbientKind::Pos3 => VariableType::Pos3,
                _ => VariableType::Vec3,
            },
            E::BinaryOp { op, left, right } => {
                derive_binop_type(*op, self.derive_type(left), self.derive_type(right))?
            }
            E::UnaryOp { operand, .. } => self.derive_type(operand)?,
            E::Field { type_of_field, .. } => type_of_field.clone(),
            E::ArrayAccess { array, .. } => element_type(&self.derive_type(array)?)?,
            E::Construct { t, .. } | E::ConstructExtern { t, .. } => t.clone(),
            E::ArrayLiteral(items) => {
                VariableType::Array(Box::new(self.derive_type(items.first()?)?), items.len())
            }
            E::ExplicitCast { to, .. } => to.clone(),
        })
    }

    /// Takes the hoisted statements collected since the last call.
    pub fn take_hoisted(&mut self) -> (Vec<java::Statement>, Vec<java::Statement>, Vec<Ref<Var>>) {
        (
            std::mem::take(&mut self.pre),
            std::mem::take(&mut self.post),
            std::mem::take(&mut self.written_back),
        )
    }
}
