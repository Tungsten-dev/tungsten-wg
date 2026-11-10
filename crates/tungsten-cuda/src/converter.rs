/*
Per-function conversion state: variable naming, type derivation, collected references and
warnings. Shared by expression.rs, statement.rs and function.rs.
*/

use std::collections::{HashMap, HashSet};

use tungsten_interface::Interface;
use tungsten_interface::naming::cuda_sanitize;
use tungsten_ir::model::{
    BinaryOperator, Constant, Expression, Function, Name, OptimizedProgram, PermutationTableInput, Ref, Var,
    VariableType,
};
use tungsten_rust::types::{derive_binop_type, derive_const_type, element_type};

use super::types::{device_type, find_density_input, unit_return_type};
use crate::backend::CudaConfig;
use crate::model as cuda;

/// Converts one function body. Collects the helpers and constants it references so the caller can
/// generate them afterwards.
pub struct FunctionConverter<'a> {
    pub program: &'a OptimizedProgram,
    pub naming: &'a Interface,
    pub config: &'a CudaConfig,
    pub function: &'a Function,
    /// Generated name of the function, used in warnings and debug-check locations.
    pub function_name: String,
    /// Helper functions referenced by the converted code, still to be generated.
    pub called: Vec<Ref<Function>>,
    /// Constants referenced by the converted code, still to be generated.
    pub used_constants: Vec<Ref<Constant>>,
    pub warnings: Vec<String>,
    vars: HashMap<Ref<Var>, String>,
    used_names: HashSet<String>,
}

impl<'a> FunctionConverter<'a> {
    /// `used_names` must hold every reserved and namespace-level name: C++ locals shadow functions
    /// and constants, so a local may not reuse one.
    pub fn new(
        program: &'a OptimizedProgram,
        naming: &'a Interface,
        config: &'a CudaConfig,
        function: &'a Function,
        function_name: &str,
        used_names: HashSet<String>,
    ) -> Self {
        Self {
            program,
            naming,
            config,
            function,
            function_name: function_name.to_string(),
            called: Vec::new(),
            used_constants: Vec::new(),
            warnings: Vec::new(),
            vars: HashMap::new(),
            used_names,
        }
    }

    /// `base`, or `base_N` for the first free N; keeps names unique within the function.
    pub fn fresh_name(&mut self, base: &str) -> String {
        let base = cuda_sanitize(base);
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

    /// The C++ local for `r`, named on first use.
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

    pub fn cuda_var_type(&self, r: Ref<Var>) -> cuda::Type {
        device_type(self.var_type(r))
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
                let (l, r) = (self.derive_type(left), self.derive_type(right));
                match op {
                    // `derive_binop_type` leaves these out; Java gives shifts the left operand's
                    // type and bitwise operators the wider integer type.
                    BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight | BinaryOperator::LogicalShiftRight => l?,
                    BinaryOperator::BitAnd | BinaryOperator::BitOr | BinaryOperator::BitXor => {
                        match (l, r) {
                            (Some(VariableType::I64), _) | (_, Some(VariableType::I64)) => VariableType::I64,
                            (Some(VariableType::I32), _) | (_, Some(VariableType::I32)) => VariableType::I32,
                            (l, r) if l == r => l?,
                            _ => return None,
                        }
                    }
                    _ => derive_binop_type(*op, l, r)?,
                }
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

    pub fn warn(&mut self, message: String) {
        self.warnings.push(format!("cuda: {}: {}", self.function_name, message));
    }

    /// String literal naming this function, for debug-check messages.
    pub fn location(&self) -> cuda::Expression {
        cuda::Expression::Raw(format!("\"{}\"", self.function_name))
    }
}
