//! Converts the interned, immutable SPMT model (`crate::spmt`) into the arena-based
//! optimizer model (`crate::model`).
//!
//!
//! # Conversion rules
//!
//! * Every SPMT `DensityFunction` becomes one `ComputeUnit` plus one `Function` (its body and variables).
//! * Every SPMT `Function` becomes one `Function`.
//! * Variables are owned by the `Function` they are declared in (parameters, `variables`,
//!   and the vars of density inputs). Host-input vars and constant vars are NOT variables
//!   in the new model: uses of them become `HostInput(..)` / `Constant(..)` expressions.
//! * Permutation tables and host inputs are deduplicated program-wide. Density inputs are
//!   deduplicated per compute unit by their target (matching `add_density_input`).
//! * Inputs, permutation tables and helper functions that are used but not declared
//!   are registered on first use, so the produced lists always match the bodies.
//! * Variable lookup is strict: a variable that is used but never declared is an error, except
//!   for the ambient names below.
//! * Variables named `origin`, `pos3`, `origin_scale` or `position_scale` are ambients: reads
//!   become `Ambient(..)` and they are not variables in the new model. A declared local of another
//!   type, a helper parameter, a constant or a host input with such a name shadows the ambient. An
//!   undeclared variable matches on its name alone, whatever its type.
//! * Ambients are read-only in the new model, but legacy bodies may assign to these names
//!   (`water_levels` in `tungsten-mc-compile` moves `pos3`). A function that does gets a real local
//!   of that name, initialized from the ambient at the top of its body.

use std::{collections::HashMap, fmt};

use crate::spmt::{self as old, Name::Named};
use tungsten_ir::model::{self as new, Arena, Ref};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ConvertError {
    /// A variable was used but is not declared in the enclosing function.
    UnknownVariable { context: String, var: String },
    /// An `Assign` targets a constant or a host input.
    AssignToReadOnly { context: String, var: String },
    /// `HostInput::Dynamic` carries no name/type in the new model.
    DynamicHostInputUnsupported,
    /// Host inputs must be `Name::Named` (`HostInput::get_name` relies on it).
    HostInputNotNamed { name: String },
    /// A helper function (not a density function) reads a density input.
    DensityInputInHelperFunction { context: String },
    /// A constant's initializer uses something that is not a constant expression.
    NotConstant {
        constant: String,
        found: &'static str,
    },
    /// The type of a field access base can't be determined (e.g. an empty array literal or an
    /// index into a non-array).
    UnknownFieldBaseType { context: String, field: String },
    /// A variable that resolved to an ambient by name was created with another type (warning only).
    AmbientTypeMismatch {
        context: String,
        var: String,
        found: new::VariableType,
    },
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::UnknownVariable { context, var } => {
                write!(f, "in `{context}`: variable {var} is used but not declared")
            }
            ConvertError::AssignToReadOnly { context, var } => {
                write!(f, "in `{context}`: assignment to constant/host input {var}")
            }
            ConvertError::DynamicHostInputUnsupported => {
                write!(
                    f,
                    "dynamic host inputs are not supported by the optimizer model"
                )
            }
            ConvertError::HostInputNotNamed { name } => {
                write!(
                    f,
                    "host input must have a unique `Named` name, found {name}"
                )
            }
            ConvertError::DensityInputInHelperFunction { context } => {
                write!(f, "helper function `{context}` reads a density input")
            }
            ConvertError::NotConstant { constant, found } => {
                write!(
                    f,
                    "constant `{constant}` is not a constant expression ({found})"
                )
            }
            ConvertError::UnknownFieldBaseType { context, field } => {
                write!(
                    f,
                    "in `{context}`: can't determine the type of the base of field `{field}`"
                )
            }
            ConvertError::AmbientTypeMismatch { context, var, found } => {
                write!(f, "in `{context}`: ambient {var} is used with the wrong type {found:?}")
            }
        }
    }
}

impl std::error::Error for ConvertError {}

type Result<T> = std::result::Result<T, ConvertError>;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Converts a whole SPMT program into the optimizer model.
pub fn convert<'m>(spmt: &old::SPMT<'m>) -> Result<new::OptimizedProgram> {
    let mut c = Converter::new();

    for df in &spmt.density_functions {
        c.density_function(*df)?;
    }
    for f in &spmt.functions {
        c.function(*f)?;
    }
    for main in &spmt.main_density_functions {
        let unit = c.density_function(main.density_function)?;
        c.program.entry_points.push(new::EntryPoint {
            density_function: unit,
            dimensions: main.dimensions,
            scaled_origin: main.scaled_origin,
            scaled_position: main.scaled_position,
        });
    }

    Ok(c.program)
}

// ---------------------------------------------------------------------------
// Converter state
// ---------------------------------------------------------------------------

/// Everything that is local to one function body while it is being converted.
struct Scope<'m> {
    /// Name used in error messages.
    label: String,
    /// Density functions may read density inputs; helper functions may not.
    is_density_function: bool,

    /// The variable arena of the function being built.
    vars: Arena<new::Var>,
    var_map: HashMap<old::Var<'m>, Ref<new::Var>>,

    host_map: HashMap<old::Var<'m>, Ref<new::HostInput>>,
    const_map: HashMap<old::Var<'m>, Ref<new::Constant>>,

    density_map: HashMap<old::Var<'m>, Ref<new::DensityInput>>,
    density_direct_map: Vec<(old::DensityInput<'m>, Ref<new::DensityInput>)>,
    // Lists that end up on the `ComputeUnit`.
    density_inputs: Arena<new::DensityInput>,
    perm_tables: Vec<Ref<new::PermutationTableInput>>,
    helpers: Vec<Ref<new::Function>>,
}

impl<'m> Scope<'m> {
    fn new(label: String, is_density_function: bool) -> Self {
        Self {
            label,
            is_density_function,
            vars: Arena::new(),
            var_map: HashMap::new(),
            host_map: HashMap::new(),
            const_map: HashMap::new(),
            density_map: HashMap::new(),
            density_inputs: Arena::new(),
            density_direct_map: Vec::new(),
            perm_tables: Vec::new(),
            helpers: Vec::new(),
        }
    }

    fn is_read_only(&self, v: &old::Var<'m>) -> bool {
        self.const_map.contains_key(v) || self.host_map.contains_key(v)
    }

    fn add_perm(&mut self, r: Ref<new::PermutationTableInput>) {
        if !self.perm_tables.contains(&r) {
            self.perm_tables.push(r);
        }
    }

    fn add_helper(&mut self, r: Ref<new::Function>) {
        if !self.helpers.contains(&r) {
            self.helpers.push(r);
        }
    }

    // fn unknown_var(&self, v: &old::Var<'m>) -> ConvertError {
    //     ConvertError::UnknownVariable {
    //         context: self.label.clone(),
    //         var: format!("{:?}", v.name),
    //     }
    // }

    fn find_named_var(&self, target: &old::Var<'m>) -> Option<Ref<new::Var>> {
        for (ref_var, new_var) in self.vars.iter() {
            let (Named(name), Named(target_name)) = (&new_var.name, &target.name) else {
                continue;
            };
            if name == target_name {
                return Some(ref_var);
            }
        }
        None
    }
}

/// Declares `v` in the scope's variable arena (idempotent).
fn declare_var<'m>(scope: &mut Scope<'m>, v: old::Var<'m>) -> Ref<new::Var> {
    if let Some(&r) = scope.var_map.get(&v) {
        return r;
    }
    let r = scope.vars.insert(new::Var {
        name: v.name.clone(),
        t: v.t.clone(),
    });
    scope.var_map.insert(v, r);
    r
}

/// The ambient a variable name stands for.
fn ambient_kind(name: &old::Name) -> Option<new::AmbientKind> {
    let Named(name) = name else { return None };
    match name.as_str() {
        "origin" => Some(new::AmbientKind::Origin),
        "pos3" => Some(new::AmbientKind::Pos3),
        "origin_scale" => Some(new::AmbientKind::OriginScale),
        "position_scale" => Some(new::AmbientKind::PositionScale),
        _ => None,
    }
}

fn ambient_type(kind: new::AmbientKind) -> old::VariableType {
    match kind {
        new::AmbientKind::Pos3 => old::VariableType::Pos3,
        _ => old::VariableType::Vec3,
    }
}

/// `v` has both the name and the type of an ambient, so declaring it declares nothing.
fn is_ambient_var(v: &old::Var<'_>) -> bool {
    ambient_kind(&v.name).is_some_and(|k| ambient_type(k) == v.t)
}

/// Declares a local for every ambient name `stmts` assign to (unless something else in the scope
/// already owns the name) and returns the assignments that initialize them from the ambient.
fn bind_assigned_ambients<'m>(
    scope: &mut Scope<'m>,
    stmts: &[old::Statement<'m>],
    out: &mut Vec<new::Statement>,
) {
    for s in stmts {
        match s {
            old::Statement::Assign { target, .. } => {
                let Some(kind) = ambient_kind(&target.name) else { continue };
                if scope.var_map.contains_key(target)
                    || scope.is_read_only(target)
                    || scope.find_named_var(target).is_some()
                {
                    continue;
                }
                let local = scope.vars.insert(new::Var {
                    name: target.name.clone(),
                    t: ambient_type(kind),
                });
                out.push(new::Statement::Assign {
                    target: local,
                    value: new::Expression::Ambient(kind),
                });
            }
            old::Statement::If { then_branch, else_branch, .. } => {
                bind_assigned_ambients(scope, then_branch, out);
                bind_assigned_ambients(scope, else_branch, out);
            }
            old::Statement::While { body, .. } | old::Statement::Repeat { body, .. } => {
                bind_assigned_ambients(scope, body, out);
            }
            old::Statement::Return(_) | old::Statement::Break => {}
        }
    }
}

struct Converter<'m> {
    program: new::OptimizedProgram,

    // Memo tables. Entries are inserted *before* a body is converted, so
    // (mutually) recursive references resolve to the placeholder slot.
    compute_units: HashMap<old::DensityFunctionRef<'m>, Ref<new::ComputeUnit>>,
    functions: HashMap<old::FunctionRef<'m>, Ref<new::Function>>,

    // Program-wide deduplication.
    perm_tables: HashMap<new::PermutationTableInput, Ref<new::PermutationTableInput>>,
    host_inputs: Vec<(new::HostInput, Ref<new::HostInput>)>,

    warnings: Vec<ConvertError>,
}

impl<'m> Converter<'m> {
    fn new() -> Self {
        Self {
            program: new::OptimizedProgram {
                compute_units: Arena::new(),
                functions: Arena::new(),
                entry_points: Vec::new(),
                constants: Arena::new(),
                host_inputs: Arena::new(),
                permutation_tables: Arena::new(),
            },
            compute_units: HashMap::new(),
            functions: HashMap::new(),
            perm_tables: HashMap::new(),
            host_inputs: Vec::new(),
            warnings: Vec::new(),
        }
    }

    // ----- density functions -------------------------------------------------

    fn density_function(
        &mut self,
        df: old::DensityFunctionRef<'m>,
    ) -> Result<Ref<new::ComputeUnit>> {
        if let Some(&r) = self.compute_units.get(&df) {
            return Ok(r);
        }

        // Reserve both slots first so cyclic references can't recurse forever.
        let function_ref = self
            .program
            .functions
            .insert(empty_function(old::VariableType::F64));
        let unit_ref = self
            .program
            .compute_units
            .insert(new::ComputeUnit::DensityFunction {
                function: function_ref,
                density_inputs: Arena::new(),
                permutation_table_inputs: vec![],
                host_inputs: vec![],
                helper_functions: vec![],
                constants: vec![],
                source_hash: df.source_hash,
            });
        self.compute_units.insert(df, unit_ref);

        let label = df
            .canonical_name
            .clone()
            .unwrap_or_else(|| "<anonymous density function>".to_string());
        let mut scope = Scope::new(label, true);

        // 1. Host inputs (their vars become HostInput expressions, not variables).
        let mut host_refs = Vec::with_capacity(df.host_inputs.len());
        for h in &df.host_inputs {
            host_refs.push(self.host_input(&mut scope, h)?);
        }

        // 2. Constants: reserve slots first so constants may reference each other in any order.
        let mut const_refs = Vec::with_capacity(df.constants.len());
        for (i, (var, _)) in df.constants.iter().enumerate() {
            let r = self.program.constants.insert(new::Constant {
                value: new::ConstExpr::Int(0), // placeholder, filled in step 6
                name: constant_name(&var.name, i),
            });
            scope.const_map.insert(*var, r);
            const_refs.push(r);
        }

        // 3. Declared variables. Density-input vars only name their input (step 5).
        for v in &df.variables {
            if !scope.is_read_only(v)
                && !is_ambient_var(v)
                && v.t != old::VariableType::DensityInput
            {
                declare_var(&mut scope, *v);
            }
        }

        // 4. Declared permutation tables.
        for p in &df.permutation_table_inputs {
            let r = self.perm_table(p);
            scope.add_perm(r);
        }

        // 5. Declared density inputs (converts the dependencies recursively).
        for di in &df.density_inputs {
            self.density_input(&mut scope, di)?;
        }

        // 6. Constant initializers.
        for (r, (var, expr)) in const_refs.iter().zip(&df.constants) {
            let name = constant_name(&var.name, 0);
            let value = const_expr(&scope, &name, expr)?;
            self.program.constants[*r].value = value;
        }

        // 7. Declared helper functions.
        for f in &df.helper_functions {
            let r = self.function(*f)?;
            scope.add_helper(r);
        }

        // 8. Body.
        let mut body = Vec::new();
        bind_assigned_ambients(&mut scope, &df.body, &mut body);
        body.extend(self.statements(&mut scope, &df.body)?);

        // 9. Fill the reserved slots.
        let Scope {
            vars,
            density_inputs,
            perm_tables,
            helpers,
            ..
        } = scope;

        self.program.functions[function_ref] = new::Function {
            canonical_name: df.canonical_name.clone(),
            host_inputs: host_refs.clone(),
            body,
            variables: vars,
            parameters: vec![], // the position is implicit for density functions
            return_type: df.return_type.clone(),
            source_hash: df.source_hash,
        };
        self.program.compute_units[unit_ref] = new::ComputeUnit::DensityFunction {
            function: function_ref,
            density_inputs,
            permutation_table_inputs: perm_tables,
            host_inputs: host_refs,
            helper_functions: helpers,
            constants: const_refs,
            source_hash: df.source_hash,
        };

        Ok(unit_ref)
    }

    // ----- helper functions --------------------------------------------------

    fn function(&mut self, f: old::FunctionRef<'m>) -> Result<Ref<new::Function>> {
        if let Some(&r) = self.functions.get(&f) {
            return Ok(r);
        }

        let function_ref = self
            .program
            .functions
            .insert(empty_function(f.return_type.clone()));
        self.functions.insert(f, function_ref);

        let label = f
            .canonical_name
            .clone()
            .unwrap_or_else(|| "<anonymous function>".to_string());
        let mut scope = Scope::new(label, false);

        let mut parameters = Vec::with_capacity(f.parameters.len());
        for p in &f.parameters {
            parameters.push(declare_var(&mut scope, *p));
        }
        for v in &f.variables {
            if !is_ambient_var(v) && v.t != old::VariableType::DensityInput {
                declare_var(&mut scope, *v);
            }
        }

        let mut body = Vec::new();
        bind_assigned_ambients(&mut scope, &f.body, &mut body);
        body.extend(self.statements(&mut scope, &f.body)?);

        self.program.functions[function_ref] = new::Function {
            canonical_name: f.canonical_name.clone(),
            host_inputs: vec![],
            body,
            variables: scope.vars,
            parameters,
            return_type: f.return_type.clone(),
            // SPMT functions carry no source hash; the hashing/dedup pass recomputes it.
            source_hash: 0,
        };

        Ok(function_ref)
    }

    // ----- inputs ------------------------------------------------------------

    fn host_input(
        &mut self,
        scope: &mut Scope<'m>,
        h: &old::HostInput<'m>,
    ) -> Result<Ref<new::HostInput>> {
        let var = match h {
            old::HostInput::Static(v) => *v,
            old::HostInput::Dynamic(_) => return Err(ConvertError::DynamicHostInputUnsupported),
        };
        if !matches!(var.name, old::Name::Named(_)) {
            return Err(ConvertError::HostInputNotNamed {
                name: format!("{:?}", var.name),
            });
        }

        let candidate = new::HostInput::Static {
            name: var.name.clone(),
            t: var.t.clone(),
        };
        let existing = self
            .host_inputs
            .iter()
            .find(|(h, _)| *h == candidate)
            .map(|(_, r)| *r);
        let r = match existing {
            Some(r) => r,
            None => {
                let r = self.program.host_inputs.insert(candidate.clone());
                self.host_inputs.push((candidate, r));
                r
            }
        };

        scope.host_map.insert(var, r);
        Ok(r)
    }

    fn perm_table(&mut self, p: &old::PermutationTableInput) -> Ref<new::PermutationTableInput> {
        let converted = convert_perm(p);
        if let Some(&r) = self.perm_tables.get(&converted) {
            return r;
        }
        let r = self.program.permutation_tables.insert(converted.clone());
        self.perm_tables.insert(converted, r);
        r
    }

    fn density_input(
        &mut self,
        scope: &mut Scope<'m>,
        di: &old::DensityInput<'m>,
    ) -> Result<Ref<new::DensityInput>> {
        if let Some(&r) = scope.density_map.get(&di.var) {
            return Ok(r);
        }

        // // Same target as an existing input: SPMT's `add_density_input` keeps only the first.
        // if let Some(&r) = scope.density_by_target.get(&di.density_function) {
        //     let existing = &self.program.density_inputs[r];
        //     if existing.scaled_origin != di.scaled_origin
        //         || existing.scaled_position != di.scaled_position
        //         || existing.dimensions != di.dimensions
        //     {
        //         return Err(ConvertError::ConflictingDensityInput {
        //             context: scope.label.clone(),
        //             var: format!("{:?}", di.var.name),
        //         });
        //     }
        //     scope.density_map.insert(di.var, r);
        //     return Ok(r);
        // }
        for (existing_di, r) in &scope.density_direct_map {
            if existing_di.density_function == di.density_function
                && existing_di.scaled_origin == di.scaled_origin
                && existing_di.scaled_position == di.scaled_position
                && existing_di.dimensions == di.dimensions
            {
                return Ok(*r);
            }
        }
        if !scope.is_density_function {
            return Err(ConvertError::DensityInputInHelperFunction {
                context: scope.label.clone(),
            });
        }

        let target = self.density_function(di.density_function)?;
        let r = scope.density_inputs.insert(new::DensityInput {
            name: di.var.name.clone(),
            density_function: target,
            scaled_origin: di.scaled_origin,
            scaled_position: di.scaled_position,
            dimensions: di.dimensions,
        });

        scope.density_map.insert(di.var, r);
        // scope.density_by_target.insert(di.density_function, r);
        scope.density_direct_map.push((di.clone(), r));
        Ok(r)
    }

    // ----- statements --------------------------------------------------------

    fn statements(
        &mut self,
        scope: &mut Scope<'m>,
        stmts: &[old::Statement<'m>],
    ) -> Result<Vec<new::Statement>> {
        stmts.iter().map(|s| self.statement(scope, s)).collect()
    }

    fn statement(
        &mut self,
        scope: &mut Scope<'m>,
        s: &old::Statement<'m>,
    ) -> Result<new::Statement> {
        use new::Statement as N;
        use old::Statement as S;

        Ok(match s {
            S::Assign { target, value } => {
                let target = match scope.var_map.get(target) {
                    Some(&r) => r,
                    None if scope.is_read_only(target) => {
                        return Err(ConvertError::AssignToReadOnly {
                            context: scope.label.clone(),
                            var: format!("{:?}", target.name),
                        });
                    }
                    // None if target.name == Named("origin".into()) => {
                    None if scope.find_named_var(target).is_some() => {
                        let r = scope.find_named_var(target).unwrap();
                        self.warn(ConvertError::UnknownVariable {
                            context: scope.label.clone(),
                            var: format!("{:?}", target.name),
                        });
                        r
                    }
                    // None => return Err(scope.unknown_var(target)),
                    None => {
                        panic!("Unknown variable: {:?}", target.name);
                        // return Err(scope.unknown_var(target));
                    }
                };
                N::Assign {
                    target,
                    value: self.expr(scope, value)?,
                }
            }
            S::Return(e) => N::Return(self.expr(scope, e)?),
            S::If {
                condition,
                then_branch,
                else_branch,
            } => N::If {
                condition: self.expr(scope, condition)?,
                then_branch: self.statements(scope, then_branch)?,
                else_branch: self.statements(scope, else_branch)?,
            },
            S::While { condition, body } => N::While {
                condition: self.expr(scope, condition)?,
                body: self.statements(scope, body)?,
            },
            S::Repeat { count, body } => N::Repeat {
                count: *count,
                body: self.statements(scope, body)?,
            },
            S::Break => N::Break,
        })
    }

    // ----- expressions -------------------------------------------------------

    fn exprs(
        &mut self,
        scope: &mut Scope<'m>,
        es: &[old::Expression<'m>],
    ) -> Result<Vec<new::Expression>> {
        es.iter().map(|e| self.expr(scope, e)).collect()
    }

    fn boxed(
        &mut self,
        scope: &mut Scope<'m>,
        e: &old::Expression<'m>,
    ) -> Result<Box<new::Expression>> {
        Ok(Box::new(self.expr(scope, e)?))
    }

    fn expr(&mut self, scope: &mut Scope<'m>, e: &old::Expression<'m>) -> Result<new::Expression> {
        use new::Expression as N;
        use old::Expression as E;

        Ok(match e {
            E::Variable(v) => {
                if let Some(&r) = scope.const_map.get(v) {
                    N::Constant(r)
                } else if let Some(&r) = scope.host_map.get(v) {
                    N::HostInput(r)
                } else if let Some(&r) = scope.var_map.get(v) {
                    N::Variable(r)
                } else if let Some(r) = scope.find_named_var(v) {
                    self.warn(ConvertError::UnknownVariable {
                        context: scope.label.clone(),
                        var: format!("{:?}", v),
                    });
                    N::Variable(r)
                } else if let Some(kind) = ambient_kind(&v.name) {
                    if ambient_type(kind) != v.t {
                        self.warn(ConvertError::AmbientTypeMismatch {
                            context: scope.label.clone(),
                            var: format!("{:?}", v.name),
                            found: v.t.clone(),
                        });
                    }
                    N::Ambient(kind)
                } else {
                    // return Err(scope.unknown_var(v));
                    panic!("Unknown variable: {:?}", **v);
                }
            }
            E::Float(x) => N::Float(*x),
            E::Double(x) => N::Double(*x),
            E::Byte(x) => N::Byte(*x),
            E::Int(x) => N::Int(*x),
            E::Long(x) => N::Long(*x),
            E::Bool(x) => N::Bool(*x),

            E::FunctionCall {
                function,
                parameters,
            } => {
                let callee = self.function(*function)?;
                scope.add_helper(callee);
                N::FunctionCall {
                    function: callee,
                    parameters: self.exprs(scope, parameters)?,
                }
            }
            E::ExternCall {
                function_name,
                parameters,
                parameter_types,
                return_type,
            } => N::ExternCall {
                function_name: function_name.clone(),
                parameters: self.exprs(scope, parameters)?,
                parameter_types: parameter_types.clone(),
                return_type: return_type.clone(),
            },

            E::DensityVariable(di, index) => {
                let input = self.density_input(scope, di)?;
                let index = match index {
                    Some(i) => Some(self.boxed(scope, i)?),
                    None => None,
                };
                N::DensityVariable(input, index)
            }
            E::PermutationTable(p) => {
                let r = self.perm_table(p);
                scope.add_perm(r);
                N::PermutationTable(r)
            }

            E::BinaryOp { op, left, right } => N::BinaryOp {
                op: binary_op(*op),
                left: self.boxed(scope, left)?,
                right: self.boxed(scope, right)?,
            },
            E::UnaryOp { op, operand } => N::UnaryOp {
                op: unary_op(*op),
                operand: self.boxed(scope, operand)?,
            },
            E::Field {
                base,
                field,
                type_of_field,
                known_idnex: _,
            } => {
                let converted = self.boxed(scope, base)?;
                // An ambient has its own type, whatever the legacy variable was created with.
                let base_type = match *converted {
                    N::Ambient(kind) => Some(ambient_type(kind)),
                    _ => base_type(base),
                }
                .ok_or_else(|| ConvertError::UnknownFieldBaseType {
                    context: scope.label.clone(),
                    field: field.clone(),
                })?;
                N::Field {
                    base_type,
                    base: converted,
                    field: field.clone(),
                    type_of_field: type_of_field.clone(),
                }
            }
            E::ArrayAccess { array, index } => N::ArrayAccess {
                array: self.boxed(scope, array)?,
                index: self.boxed(scope, index)?,
            },
            E::Construct { t, args } => N::Construct {
                t: t.clone(),
                args: self.exprs(scope, args)?,
            },
            E::ConstructExtern { t, args } => N::ConstructExtern {
                t: t.clone(),
                args: args
                    .iter()
                    .map(|(name, e)| Ok((*name, self.expr(scope, e)?)))
                    .collect::<Result<Vec<_>>>()?,
            },
            E::ArrayLiteral(items) => N::ArrayLiteral(self.exprs(scope, items)?),
            E::ExplicitCast { to, expr } => N::ExplicitCast {
                to: to.clone(),
                expr: self.boxed(scope, expr)?,
            },
        })
    }

    fn warn(&mut self, warning: ConvertError) {
        self.warnings.push(warning);
    }
}

// ---------------------------------------------------------------------------
// Constant expressions
// ---------------------------------------------------------------------------

/// Converts a constant's initializer. Only literals, operators, casts, fields, arrays,
/// constructors and references to other constants are allowed.
fn const_expr<'m>(
    scope: &Scope<'m>,
    constant: &str,
    e: &old::Expression<'m>,
) -> Result<new::ConstExpr> {
    use new::ConstExpr as C;
    use old::Expression as E;

    let rec = |x: &old::Expression<'m>| const_expr(scope, constant, x);
    let boxed = |x: &old::Expression<'m>| rec(x).map(Box::new);
    let not_const = |found: &'static str| ConvertError::NotConstant {
        constant: constant.to_string(),
        found,
    };

    Ok(match e {
        E::Variable(v) => match scope.const_map.get(v) {
            Some(&r) => C::Constant(r),
            None => return Err(not_const("reference to a non-constant variable")),
        },
        E::Float(x) => C::Float(*x),
        E::Double(x) => C::Double(*x),
        E::Byte(x) => C::Byte(*x),
        E::Int(x) => C::Int(*x),
        E::Long(x) => C::Long(*x),
        E::Bool(x) => C::Bool(*x),

        E::BinaryOp { op, left, right } => C::BinaryOp {
            op: binary_op(*op),
            left: boxed(left)?,
            right: boxed(right)?,
        },
        E::UnaryOp { op, operand } => C::UnaryOp {
            op: unary_op(*op),
            operand: boxed(operand)?,
        },
        E::ExplicitCast { to, expr } => C::ExplicitCast {
            to: to.clone(),
            expr: boxed(expr)?,
        },
        E::Field {
            base,
            field,
            type_of_field,
            known_idnex: _,
        } => C::Field {
            base_type: base_type(base).ok_or_else(|| ConvertError::UnknownFieldBaseType {
                context: constant.to_string(),
                field: field.clone(),
            })?,
            base: boxed(base)?,
            field: field.clone(),
            type_of_field: type_of_field.clone(),
        },
        E::ArrayAccess { array, index } => C::ArrayAccess {
            array: boxed(array)?,
            index: boxed(index)?,
        },
        E::ArrayLiteral(items) => {
            C::ArrayLiteral(items.iter().map(|x| rec(x)).collect::<Result<Vec<_>>>()?)
        }
        E::Construct { t, args } => C::Construct {
            t: t.clone(),
            args: args.iter().map(|x| rec(x)).collect::<Result<Vec<_>>>()?,
        },

        E::FunctionCall { .. } => return Err(not_const("function call")),
        E::ExternCall { .. } => return Err(not_const("extern call")),
        E::DensityVariable(..) => return Err(not_const("density input")),
        E::PermutationTable(_) => return Err(not_const("permutation table")),
        E::ConstructExtern { .. } => return Err(not_const("extern construction")),
    })
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// The type of a field access base, stored in the new IR's `Field::base_type`. `None` if no rule
/// gives one (empty array literal, index into a non-array). Unlike `try_derive_type` it handles
/// mixed `vec3`/`pos3`/scalar arithmetic and indexing into arrays.
fn base_type(e: &old::Expression<'_>) -> Option<old::VariableType> {
    use old::BinaryOperator as B;
    use old::Expression as E;
    use old::VariableType as T;
    Some(match e {
        E::Variable(v) => v.t.clone(),
        E::Float(_) => T::F32,
        E::Double(_) => T::F64,
        E::Byte(_) => T::I8,
        E::Int(_) => T::I32,
        E::Long(_) => T::I64,
        E::Bool(_) => T::Bool,
        E::FunctionCall { function, .. } => function.return_type.clone(),
        E::ExternCall { return_type, .. } => return_type.clone(),
        E::DensityVariable(input, _) => input.density_function.return_type.clone(),
        E::PermutationTable(old::PermutationTableInput::PerlinNoise { .. }) => T::PermutationTable,
        E::PermutationTable(old::PermutationTableInput::Base3DNoise) => {
            T::InterpolatedNoiseSampler
        }
        E::BinaryOp { op, left, right } => match op {
            B::Equal
            | B::NotEqual
            | B::Less
            | B::LessEqual
            | B::Greater
            | B::GreaterEqual
            | B::And
            | B::Or => T::Bool,
            B::Add | B::Subtract | B::Multiply | B::Divide => {
                let l = base_type(left)?;
                match (&l, base_type(right)) {
                    (T::Vec3, Some(T::Pos3)) | (T::Pos3, Some(T::Vec3)) => T::Vec3,
                    _ => l,
                }
            }
            _ => base_type(left)?,
        },
        E::UnaryOp { operand, .. } => base_type(operand)?,
        E::Field { type_of_field, .. } => type_of_field.clone(),
        E::ArrayAccess { array, .. } => match base_type(array)? {
            T::Array(elem, _) => *elem,
            T::Vec3 => T::F64,
            T::Pos3 => T::I32,
            _ => return None,
        },
        E::Construct { t, .. } | E::ConstructExtern { t, .. } => t.clone(),
        E::ExplicitCast { to, .. } => to.clone(),
        E::ArrayLiteral(items) => T::Array(Box::new(base_type(items.first()?)?), items.len()),
    })
}

fn empty_function(return_type: old::VariableType) -> new::Function {
    new::Function {
        canonical_name: None,
        host_inputs: vec![],
        body: vec![],
        variables: Arena::new(),
        parameters: vec![],
        return_type,
        source_hash: 0,
    }
}

fn constant_name(name: &old::Name, index: usize) -> String {
    match name {
        old::Name::Named(s) | old::Name::Prefixed(s) => s.clone(),
        old::Name::Anonymous => format!("constant_{index}"),
    }
}

fn convert_perm(p: &old::PermutationTableInput) -> new::PermutationTableInput {
    match p {
        old::PermutationTableInput::PerlinNoise {
            ident,
            subident,
            subident_index,
        } => new::PermutationTableInput::PerlinNoise {
            ident: ident.clone(),
            subident: subident.clone(),
            subident_index: *subident_index,
        },
        old::PermutationTableInput::Base3DNoise => new::PermutationTableInput::Base3DNoise,
    }
}

fn binary_op(op: old::BinaryOperator) -> new::BinaryOperator {
    use new::BinaryOperator as N;
    use old::BinaryOperator as O;
    match op {
        O::Add => N::Add,
        O::Subtract => N::Subtract,
        O::Multiply => N::Multiply,
        O::Divide => N::Divide,
        O::Equal => N::Equal,
        O::NotEqual => N::NotEqual,
        O::Less => N::Less,
        O::LessEqual => N::LessEqual,
        O::Greater => N::Greater,
        O::GreaterEqual => N::GreaterEqual,
        O::And => N::And,
        O::Or => N::Or,
        O::ShiftLeft => N::ShiftLeft,
        O::ShiftRight => N::ShiftRight,
        O::LogicalShiftRight => N::LogicalShiftRight,
        O::BitOr => N::BitOr,
        O::BitAnd => N::BitAnd,
        O::BitXor => N::BitXor,
    }
}

fn unary_op(op: old::UnaryOperator) -> new::UnaryOperator {
    match op {
        old::UnaryOperator::Negate => new::UnaryOperator::Negate,
        old::UnaryOperator::Not => new::UnaryOperator::Not,
        old::UnaryOperator::Ref => new::UnaryOperator::Ref(false),
        old::UnaryOperator::MutRef => new::UnaryOperator::Ref(true),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// #[cfg(test)]
// mod tests {
//     use super::*;

//     fn leak<T>(v: T) -> &'static T {
//         Box::leak(Box::new(v))
//     }

//     fn var(name: &str, t: old::VariableType) -> old::Var<'static> {
//         old::Interned::new(leak(old::Variable {
//             name: old::Name::Named(name.to_string()),
//             t,
//         }))
//     }

//     fn density_function<'m>(
//         name: &str,
//         body: Vec<old::Statement<'m>>,
//         variables: Vec<old::Var<'m>>,
//         density_inputs: Vec<old::DensityInput<'m>>,
//         host_inputs: Vec<old::HostInput<'m>>,
//     ) -> old::DensityFunctionRef<'m> {
//         old::Interned::new(leak(old::DensityFunction {
//             canonical_name: Some(name.to_string()),
//             density_inputs,
//             permutation_table_inputs: vec![],
//             host_inputs,
//             body,
//             variables,
//             helper_functions: vec![],
//             constants: vec![],
//             source_hash: 7,
//         }))
//     }

//     #[test]
//     fn converts_dependencies_host_inputs_and_entry_points() {
//         use old::{Expression as E, Statement as S};

//         // leaf: returns the host input `seed` cast to f32
//         let seed = var("seed", old::VariableType::I64);
//         let leaf = density_function(
//             "leaf",
//             vec![S::Return(E::ExplicitCast {
//                 to: old::VariableType::F32,
//                 expr: Box::new(E::Variable(seed)),
//             })],
//             vec![],
//             vec![],
//             vec![old::HostInput::Static(seed)],
//         );

//         // root: reads `leaf` through a density input and adds one
//         let input_var = var("leaf_in", old::VariableType::DensityInput);
//         let tmp = var("tmp", old::VariableType::F32);
//         let input = old::DensityInput {
//             var: input_var,
//             density_function: leaf,
//             scaled_origin: (0.0, 0.0, 0.0),
//             scaled_position: (1.0, 1.0, 1.0),
//             dimensions: (4, 4, 4),
//         };
//         let root = density_function(
//             "root",
//             vec![
//                 S::Assign {
//                     target: tmp,
//                     value: E::BinaryOp {
//                         op: old::BinaryOperator::Add,
//                         left: Box::new(E::DensityVariable(input.clone(), None)),
//                         right: Box::new(E::Float(1.0)),
//                     },
//                 },
//                 S::Return(E::Variable(tmp)),
//             ],
//             vec![tmp],
//             vec![input],
//             vec![],
//         );

//         let spmt = old::SPMT {
//             density_functions: vec![root],
//             functions: vec![],
//             main_density_functions: vec![old::MainDensityFunction {
//                 density_function: root,
//                 dimensions: (4, 4, 4),
//                 scaled_origin: (0.0, 0.0, 0.0),
//                 scaled_position: (1.0, 1.0, 1.0),
//             }],
//         };

//         let program = convert(&spmt).expect("conversion should succeed");

//         assert_eq!(program.entry_points.len(), 1);
//         assert_eq!(program.compute_units.iter().count(), 2);
//         assert_eq!(program.functions.iter().count(), 2);
//         assert_eq!(program.density_inputs.iter().count(), 1);
//         assert_eq!(program.host_inputs.iter().count(), 1);
//     }

//     #[test]
//     fn undeclared_variable_is_an_error() {
//         use old::{Expression as E, Statement as S};

//         let ghost = var("ghost", old::VariableType::F32);
//         let df = density_function("bad", vec![S::Return(E::Variable(ghost))], vec![], vec![], vec![]);
//         let spmt = old::SPMT {
//             density_functions: vec![df],
//             functions: vec![],
//             main_density_functions: vec![],
//         };

//         assert!(matches!(
//             convert(&spmt),
//             Err(ConvertError::UnknownVariable { .. })
//         ));
//     }
// }

#[cfg(test)]
mod base_type_tests {
    use super::base_type;
    use crate::spmt::{self as old, BinaryOperator as B, Expression as E, VariableType as T};

    fn var<'m>(v: &'m old::Variable) -> E<'m> {
        E::Variable(old::Interned::new(v))
    }

    fn bin<'m>(op: B, l: E<'m>, r: E<'m>) -> E<'m> {
        E::BinaryOp {
            op,
            left: Box::new(l),
            right: Box::new(r),
        }
    }

    #[test]
    fn rules() {
        let v3 = old::Variable {
            name: old::Name::Named("v".into()),
            t: T::Vec3,
        };
        let p3 = old::Variable {
            name: old::Name::Named("p".into()),
            t: T::Pos3,
        };
        let arr = old::Variable {
            name: old::Name::Named("a".into()),
            t: T::Array(Box::new(T::Pos3), 2),
        };
        // Mixed binops, which `try_derive_type` can't type.
        assert_eq!(base_type(&bin(B::Multiply, var(&v3), E::Double(2.0))), Some(T::Vec3));
        assert_eq!(base_type(&bin(B::Add, var(&p3), var(&v3))), Some(T::Vec3));
        assert_eq!(base_type(&bin(B::Add, var(&v3), var(&p3))), Some(T::Vec3));
        assert_eq!(base_type(&bin(B::Less, var(&v3), var(&p3))), Some(T::Bool));
        // Indexing.
        fn index(a: E<'_>) -> E<'_> {
            E::ArrayAccess {
                array: Box::new(a),
                index: Box::new(E::Int(0)),
            }
        }
        assert_eq!(base_type(&index(var(&arr))), Some(T::Pos3));
        assert_eq!(base_type(&index(var(&v3))), Some(T::F64));
        assert_eq!(base_type(&index(E::Double(1.0))), None);
        // Array literals.
        assert_eq!(
            base_type(&E::ArrayLiteral(vec![E::Float(1.0), E::Float(2.0)])),
            Some(T::Array(Box::new(T::F32), 2))
        );
        assert_eq!(base_type(&E::ArrayLiteral(vec![])), None);
        // Nested fields use the field's type.
        let field = E::Field {
            base: Box::new(var(&v3)),
            field: "x".into(),
            type_of_field: T::F64,
            known_idnex: None,
        };
        assert_eq!(base_type(&field), Some(T::F64));
    }
}

#[cfg(test)]
mod ambient_tests {
    use bumpalo::Bump;

    use super::{ConvertError, Converter, Result};
    use crate::builder::{Builder, SPMTBuilder};
    use crate::model::{DensityFunctionRef, Expression as E, FunctionRef, Name, Statement, Var, VariableType as T};
    use tungsten_ir::model::{self as new, AmbientKind as K, Expression as N};

    fn field<'m>(v: Var<'m>, name: &str, t: T) -> E<'m> {
        E::Field {
            base: Box::new(E::Variable(v)),
            field: name.into(),
            type_of_field: t,
            known_idnex: None,
        }
    }

    fn named(name: &str) -> Name {
        Name::Named(name.into())
    }

    /// Converts a unit and returns its function plus the warnings.
    fn unit<'m>(df: DensityFunctionRef<'m>) -> Result<(new::Function, Vec<ConvertError>)> {
        let mut c = Converter::new();
        let r = c.density_function(df)?;
        let new::ComputeUnit::DensityFunction { function, .. } = &c.program.compute_units[r];
        let f = std::mem::replace(&mut c.program.functions[*function], super::empty_function(T::F64));
        Ok((f, c.warnings))
    }

    fn helper<'m>(f: FunctionRef<'m>) -> new::Function {
        let mut c = Converter::new();
        let r = c.function(f).expect("conversion");
        std::mem::replace(&mut c.program.functions[r], super::empty_function(T::F64))
    }

    fn returned(f: &new::Function) -> &N {
        match f.body.last() {
            Some(new::Statement::Return(e)) => e,
            other => panic!("expected a return, found {other:?}"),
        }
    }

    fn var_names(f: &new::Function) -> Vec<String> {
        f.variables.iter().map(|(_, v)| format!("{:?}", v.name)).collect()
    }

    #[test]
    fn all_four_names_become_ambients() {
        for (name, t, kind) in [
            ("origin", T::Vec3, K::Origin),
            ("pos3", T::Pos3, K::Pos3),
            ("origin_scale", T::Vec3, K::OriginScale),
            ("position_scale", T::Vec3, K::PositionScale),
        ] {
            // Undeclared, which is how `tungsten-mc-compile` mostly does it.
            let arena = Bump::new();
            let mut b = Builder::new(&arena).named("u").returns(t.clone());
            let v = b.alloc_var(named(name), t.clone());
            b.set_body(vec![Statement::Return(E::Variable(v))]);
            let (f, warnings) = unit(b.finish_compute_unit()).unwrap();
            assert_eq!(returned(&f), &N::Ambient(kind));
            assert_eq!(f.variables.iter().count(), 0, "{name}");
            assert!(warnings.is_empty(), "{warnings:?}");

            // Declared.
            let mut b = Builder::new(&arena).named("u").returns(t.clone());
            let v = b.local(name, t.clone());
            b.set_body(vec![Statement::Return(E::Variable(v))]);
            let (f, warnings) = unit(b.finish_compute_unit()).unwrap();
            assert_eq!(returned(&f), &N::Ambient(kind));
            assert_eq!(f.variables.iter().count(), 0, "{name}");
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }

    #[test]
    fn helpers_read_ambients_too() {
        let arena = Bump::new();
        let mut h = Builder::new(&arena).named("h").returns(T::Pos3);
        let v = h.alloc_var(named("pos3"), T::Pos3);
        h.set_body(vec![Statement::Return(E::Variable(v))]);
        let f = helper(h.finish_function());
        assert_eq!(returned(&f), &N::Ambient(K::Pos3));
    }

    #[test]
    fn helper_parameter_shadows_the_ambient() {
        let arena = Bump::new();
        let mut h = Builder::new(&arena).named("h").returns(T::Vec3);
        let v = h.param("origin", T::Vec3);
        h.set_body(vec![Statement::Return(E::Variable(v))]);
        let f = helper(h.finish_function());
        assert!(matches!(returned(&f), N::Variable(_)));
        assert_eq!(f.parameters.len(), 1);
    }

    #[test]
    fn declared_local_of_another_type_stays_a_local() {
        let arena = Bump::new();
        let mut b = Builder::new(&arena).named("u").returns(T::F64);
        let v = b.local("origin", T::F64);
        b.set_body(vec![
            Statement::Assign { target: v, value: 1.0f64.into() },
            Statement::Return(E::Variable(v)),
        ]);
        let (f, _) = unit(b.finish_compute_unit()).unwrap();
        assert!(matches!(returned(&f), N::Variable(_)));
        assert_eq!(var_names(&f).len(), 1);
    }

    /// `aquifer_random` in `tungsten-mc-compile` creates `origin` as a `Pos3` and reads `.x` as `F64`.
    #[test]
    fn undeclared_mistyped_variable_is_the_ambient_with_its_real_type() {
        let arena = Bump::new();
        let mut b = Builder::new(&arena).named("u").returns(T::F64);
        let v = b.alloc_var(named("origin"), T::Pos3);
        b.set_body(vec![Statement::Return(field(v, "x", T::F64))]);
        let (f, warnings) = unit(b.finish_compute_unit()).unwrap();
        let N::Field { base, base_type, .. } = returned(&f) else {
            panic!("expected a field read");
        };
        assert_eq!(**base, N::Ambient(K::Origin));
        assert_eq!(*base_type, T::Vec3);
        assert!(
            matches!(warnings[..], [ConvertError::AmbientTypeMismatch { found: T::Pos3, .. }]),
            "{warnings:?}"
        );
    }

    /// `water_levels` in `tungsten-mc-compile`: it assigns one `pos3` variable and reads another.
    #[test]
    fn assigned_ambient_becomes_a_local_initialized_from_the_ambient() {
        let arena = Bump::new();
        let mut b = Builder::new(&arena).named("u").returns(T::Pos3);
        let written = b.local("pos3", T::Pos3);
        let read = b.alloc_var(named("pos3"), T::Pos3);
        b.set_body(vec![
            Statement::If {
                condition: true.into(),
                then_branch: vec![Statement::Assign { target: written, value: E::Variable(read) }],
                else_branch: vec![],
            },
            Statement::Return(E::Variable(read)),
        ]);
        let (f, _) = unit(b.finish_compute_unit()).unwrap();
        assert_eq!(f.variables.iter().count(), 1);
        let local = f.variables.iter().next().unwrap().0;
        match &f.body[0] {
            new::Statement::Assign { target, value } => {
                assert_eq!(*target, local);
                assert_eq!(*value, N::Ambient(K::Pos3));
            }
            other => panic!("expected the initializer, found {other:?}"),
        }
        assert_eq!(returned(&f), &N::Variable(local));
    }

    #[test]
    fn assigned_parameter_gets_no_initializer() {
        let arena = Bump::new();
        let mut h = Builder::new(&arena).named("h").returns(T::Vec3);
        let v = h.param("origin", T::Vec3);
        h.set_body(vec![
            Statement::Assign { target: v, value: E::Variable(v) },
            Statement::Return(E::Variable(v)),
        ]);
        let f = helper(h.finish_function());
        assert_eq!(f.body.len(), 2);
        assert_eq!(f.variables.iter().count(), 1);
    }
}
