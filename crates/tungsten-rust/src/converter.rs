/*
Per-function conversion state: variable naming, type derivation and lookups into the program.
Shared by expression.rs, statement.rs and function.rs.
*/

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use tungsten_interface::Interface;
use tungsten_interface::naming::sanitize_name;
use tungsten_ir::model::{
    AmbientKind, Arena, ComputeUnit, Constant, DensityInput, Expression, Function, Name, OptimizedProgram,
    PermutationTableInput, Ref, Var, VariableType,
};
use super::types::{convert_type, derive_binop_type, derive_const_type, element_type};
use super::uniform_parameters;
use crate::model as rcl;

/// The five parameters every density and helper function starts with.
pub struct UniformVars {
    // pub pos3: Rc<rcl::Variable>,
    pub it: Rc<rcl::Variable>,
    pub config: Rc<rcl::Variable>,
    pub buffers: Rc<rcl::Variable>,
    pub auto_consts: Rc<rcl::Variable>,
    pub host_consts: Rc<rcl::Variable>,
}

impl UniformVars {
    fn new() -> Self {
        let mut v = uniform_parameters().into_iter().map(|p| {
            Rc::new(rcl::Variable { name: Some(p.name), t: p.t, mutable: false })
        });
        Self {
            // pos3: v.next().unwrap(),
            it: v.next().unwrap(),
            config: v.next().unwrap(),
            buffers: v.next().unwrap(),
            auto_consts: v.next().unwrap(),
            host_consts: v.next().unwrap(),
        }
    }

    pub fn as_args<'m>(&self) -> Vec<rcl::Expression<'m>> {
        [&self.it, &self.config, &self.buffers, &self.auto_consts, &self.host_consts]
            .into_iter()
            .map(|v| rcl::Expression::Variable(v.clone()))
            .collect()
    }
}

/// Converts one function body. Collects the helpers and constants it references so the
/// caller can generate them afterwards.
pub struct FunctionConverter<'a> {
    pub program: &'a OptimizedProgram,
    pub naming: &'a Interface,
    pub function: &'a Function,
    /// Density inputs of the unit being converted; `None` for helper functions.
    pub density_inputs: Option<&'a Arena<DensityInput>>,
    pub uniform: UniformVars,
    /// Helper functions referenced by the converted code, still to be generated.
    pub called: Vec<Ref<Function>>,
    /// Constants referenced by the converted code, still to be generated.
    pub used_constants: Vec<Ref<Constant>>,
    vars: HashMap<Ref<Var>, Rc<rcl::Variable>>,
    used_names: HashSet<String>,
}

impl<'a> FunctionConverter<'a> {
    pub fn new(
        program: &'a OptimizedProgram,
        naming: &'a Interface,
        function: &'a Function,
        density_inputs: Option<&'a Arena<DensityInput>>,
    ) -> Self {
        let uniform = UniformVars::new();
        let used_names = [&uniform.it, &uniform.config, &uniform.buffers, &uniform.auto_consts, &uniform.host_consts]
            .iter()
            .map(|v| v.name.clone().unwrap())
            .collect();
        Self {
            program,
            naming,
            function,
            density_inputs,
            uniform,
            called: Vec::new(),
            used_constants: Vec::new(),
            vars: HashMap::new(),
            used_names,
        }
    }

    /// `base`, or `base_N` for the first free N; keeps names unique within the function.
    pub fn fresh_name(&mut self, base: &str) -> String {
        if self.used_names.insert(base.to_string()) {
            return base.to_string();
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

    /// The RCL variable for `r`, created on first use with a unique name.
    pub fn declare_var(&mut self, r: Ref<Var>) -> Rc<rcl::Variable> {
        if let Some(v) = self.vars.get(&r) {
            return v.clone();
        }
        let function: &'a Function = self.function;
        let var = &function.variables[r];
        let base = match &var.name {
            Name::Named(n) | Name::Prefixed(n) => sanitize_name(n),
            Name::Anonymous => "var".to_string(),
        };
        let name = self.fresh_name(&base);
        let rcl_var = Rc::new(rcl::Variable { name: Some(name), t: convert_type(&var.t), mutable: true });
        self.vars.insert(r, rcl_var.clone());
        rcl_var
    }

    /// The program's density input for `r`; helpers are shared between units, so every unit is searched.
    pub fn find_density_input(&self, r: Ref<DensityInput>) -> &'a DensityInput {
        if let Some(inputs) = self.density_inputs {
            if inputs.contains(r) {
                return &inputs[r];
            }
        }
        let program: &'a OptimizedProgram = self.program;
        for (_, cu) in program.compute_units.iter() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = cu;
            if density_inputs.contains(r) {
                return &density_inputs[r];
            }
        }
        panic!("density input does not belong to any compute unit");
    }

    /// Return type of the unit's main function, i.e. the element type of its buffer.
    pub fn unit_return_type(&self, cu: Ref<ComputeUnit>) -> &'a VariableType {
        let program: &'a OptimizedProgram = self.program;
        let ComputeUnit::DensityFunction { function, .. } = &program.compute_units[cu];
        &program.functions[*function].return_type
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
                self.unit_return_type(self.find_density_input(*input).density_function).clone()
            }
            E::PermutationTable(r) => match &self.program.permutation_tables[*r] {
                PermutationTableInput::PerlinNoise { .. } => VariableType::PermutationTable,
                PermutationTableInput::Base3DNoise => VariableType::InterpolatedNoiseSampler,
            },
            E::HostInput(r) => self.program.host_inputs[*r].get_type(),
            E::Ambient(k) => match k {
                AmbientKind::Pos3 => VariableType::Pos3,
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
}
