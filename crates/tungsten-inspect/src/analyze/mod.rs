//! Builds the [`Inspection`] model from a program and its interface (SPEC.md §6).
//!
//! `Ctx` resolves every IR entity to its id and display name once (§6.1); the submodules only
//! deal in those. All interface names come from the interface built with
//! `NamingRules::inspect()` and are never recomputed here.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tungsten_interface::{Instance, Interface};
use tungsten_ir::model::{
    ComputeUnit, ConstExpr, Constant, DensityInput, Expression, Function, HostInput, Name,
    OptimizedProgram, PermutationTableInput, Ref, Statement, Var, VariableType,
};

use crate::backend::ElementSizes;
use crate::model::*;

mod buffers;
mod calls;
mod dataflow;
mod graph;
mod inline;
pub(crate) mod ir_text;
mod schedule;

/// What `build` computes. Derived from `InspectConfig`.
#[derive(Debug, Clone)]
pub(crate) struct Options {
    /// Build the per-function DAGs.
    pub dataflow: bool,
    pub element_sizes: ElementSizes,
}

pub(crate) fn build(
    program: &OptimizedProgram,
    interface: &Interface,
    options: &Options,
) -> Inspection {
    inspection(&Ctx::new(program, interface, options))
}

/// The model for an already resolved `Ctx` (`diff` keeps the `Ctx` around to map refs).
pub(crate) fn inspection(ctx: &Ctx) -> Inspection {
    let instances = graph::instances(ctx);
    let entries = graph::entries(ctx, &instances);
    let units = graph::units(ctx, &instances);
    let functions = calls::functions(ctx);
    let call_graph = calls::call_graph(ctx);
    let (constants, tables, host_inputs, resources) = calls::resources(ctx);

    let unreachable = Unreachable {
        units: ids_where(&units, |u| !u.reachable, |u| &u.id),
        functions: ids_where(&functions, |f| !f.reachable, |f| &f.id),
        constants: ids_where(&constants, |c| !c.reachable, |c| &c.id),
        tables: ids_where(&tables, |t| !t.reachable, |t| &t.id),
        host_inputs: ids_where(&host_inputs, |h| !h.reachable, |h| &h.id),
    };
    let summary = Summary {
        units: units.len(),
        reachable_units: units.iter().filter(|u| u.reachable).count(),
        helpers: functions
            .iter()
            .filter(|f| f.kind == FunctionKind::Helper)
            .count(),
        constants: constants.len(),
        tables: tables.len(),
        host_inputs: host_inputs.len(),
        entries: entries.len(),
        instances: instances.len(),
        unreachable,
    };

    Inspection {
        version: SCHEMA_VERSION,
        summary,
        entries,
        units,
        instances,
        functions,
        constants,
        tables,
        host_inputs,
        call_graph,
        resources,
        notes: ctx.notes(),
        paths: BTreeMap::new(),
    }
}

fn ids_where<T>(items: &[T], keep: impl Fn(&T) -> bool, id: impl Fn(&T) -> &String) -> Vec<String> {
    items
        .iter()
        .filter(|i| keep(i))
        .map(|i| id(i).clone())
        .collect()
}

/// Id and display name of one entity.
#[derive(Debug, Clone)]
pub(crate) struct Named {
    pub id: String,
    pub display: String,
}

/// A density input's local name, and the unit whose arena it lives in.
#[derive(Debug, Clone)]
pub(crate) struct InputName {
    pub unit: Ref<ComputeUnit>,
    pub name: String,
}

/// The program, its interface, and every id and display name, resolved once.
pub(crate) struct Ctx<'a> {
    pub program: &'a OptimizedProgram,
    pub interface: &'a Interface,
    pub options: &'a Options,
    pub units: HashMap<Ref<ComputeUnit>, Named>,
    pub instances: HashMap<Instance, Named>,
    /// By entry index.
    pub entries: Vec<Named>,
    pub functions: HashMap<Ref<Function>, Named>,
    pub constants: HashMap<Ref<Constant>, Named>,
    pub tables: HashMap<Ref<PermutationTableInput>, Named>,
    pub host_inputs: HashMap<Ref<HostInput>, Named>,
    pub inputs: HashMap<Ref<DensityInput>, InputName>,
    /// Main function → its unit.
    pub main_of: HashMap<Ref<Function>, Ref<ComputeUnit>>,
    /// Helper → every unit listing it.
    pub helper_of: HashMap<Ref<Function>, Vec<Ref<ComputeUnit>>>,
    /// Variable names per function (SPEC.md §7.5 rules).
    pub var_names: HashMap<Ref<Function>, HashMap<Ref<Var>, String>>,
    /// Density inputs that some function reads with an index.
    pub indexed_inputs: HashSet<Ref<DensityInput>>,
    pub reachable_functions: HashSet<Ref<Function>>,
}

impl<'a> Ctx<'a> {
    pub fn new(
        program: &'a OptimizedProgram,
        interface: &'a Interface,
        options: &'a Options,
    ) -> Self {
        let mut main_of = HashMap::new();
        let mut helper_of: HashMap<Ref<Function>, Vec<Ref<ComputeUnit>>> = HashMap::new();
        for (r, cu) in program.compute_units.iter() {
            let ComputeUnit::DensityFunction {
                function,
                helper_functions,
                ..
            } = cu;
            main_of.insert(*function, r);
            for h in helper_functions {
                helper_of.entry(*h).or_default().push(r);
            }
        }

        let var_names = program
            .functions
            .iter()
            .map(|(r, f)| (r, variable_names(f)))
            .collect();

        let mut ctx = Self {
            program,
            interface,
            options,
            units: HashMap::new(),
            instances: HashMap::new(),
            entries: Vec::new(),
            functions: HashMap::new(),
            constants: HashMap::new(),
            tables: HashMap::new(),
            host_inputs: HashMap::new(),
            inputs: HashMap::new(),
            main_of,
            helper_of,
            var_names,
            indexed_inputs: HashSet::new(),
            reachable_functions: HashSet::new(),
        };
        ctx.name_units();
        ctx.name_instances();
        ctx.name_entries();
        ctx.name_functions();
        ctx.name_resources();
        ctx.name_inputs();
        ctx.find_indexed_inputs();
        ctx.find_reachable_functions();
        ctx
    }

    fn name_units(&mut self) {
        let named = self.program.compute_units.iter().map(|(r, cu)| {
            let ComputeUnit::DensityFunction {
                function,
                source_hash,
                ..
            } = cu;
            let display = self.program.functions[*function]
                .canonical_name
                .clone()
                .unwrap_or_else(|| format!("density_function_{:x}", source_hash));
            (r, format!("cu:{display}"), display)
        });
        self.units = unique_ids(named.collect());
    }

    fn name_instances(&mut self) {
        // Same unit and dimensions at several scales: the scales disambiguate the display.
        let mut scales: HashMap<(Ref<ComputeUnit>, (i32, i32, i32)), usize> = HashMap::new();
        for inst in self.interface.instances() {
            *scales.entry((inst.cu, inst.dimensions)).or_default() += 1;
        }
        for inst in self.interface.instances() {
            let (x, y, z) = inst.dimensions;
            let mut display = format!("{} @ {x}x{y}x{z}", self.units[&inst.cu].display);
            if scales[&(inst.cu, inst.dimensions)] > 1 {
                display.push_str(&format!(
                    " o{} p{}",
                    fmt_triple(inst.scaled_origin()),
                    fmt_triple(inst.scaled_position())
                ));
            }
            let id = format!("inst:{}", self.interface.buffer_field(inst));
            self.instances.insert(*inst, Named { id, display });
        }
    }

    fn name_entries(&mut self) {
        let instances: Vec<Instance> = self
            .program
            .entry_points
            .iter()
            .map(Instance::from_entry)
            .collect();
        for (n, inst) in instances.iter().enumerate() {
            let named = &self.instances[inst];
            let mut display = named.display.clone();
            if instances.iter().filter(|i| *i == inst).count() > 1 {
                display.push_str(&format!(" #{n}"));
            }
            self.entries.push(Named {
                id: format!("entry:{n}:{}", named.id),
                display,
            });
        }
    }

    fn name_functions(&mut self) {
        let named = self
            .program
            .functions
            .iter()
            .map(|(r, f)| match self.main_of.get(&r) {
                Some(cu) => {
                    let display = self.units[cu].display.clone();
                    let id = match self.interface.dimensions_of(*cu).first() {
                        Some(dims) => format!("fn:{}", self.interface.density_fn(*cu, *dims)),
                        // Not scheduled, so the interface has no name for it.
                        None => format!("fn:{display}"),
                    };
                    (r, id, display)
                }
                None => {
                    let helper = self.interface.helper(r);
                    let display = f
                        .canonical_name
                        .clone()
                        .unwrap_or_else(|| helper.to_string());
                    (r, format!("fn:{helper}"), display)
                }
            });
        self.functions = unique_ids(named.collect());
    }

    fn name_resources(&mut self) {
        for (r, c) in self.program.constants.iter() {
            let id = format!("const:{}", self.interface.constant(r));
            self.constants.insert(
                r,
                Named {
                    id,
                    display: c.name.clone(),
                },
            );
        }
        for (r, t) in self.program.permutation_tables.iter() {
            let display = match t {
                PermutationTableInput::PerlinNoise {
                    ident,
                    subident,
                    subident_index,
                } => format!(
                    "perlin {ident}[{subident_index}] {}",
                    subident.as_deref().unwrap_or("")
                )
                .trim_end()
                .to_string(),
                PermutationTableInput::Base3DNoise => "base3d".to_string(),
            };
            let id = format!("table:{}", self.interface.auto_const_field(r));
            self.tables.insert(r, Named { id, display });
        }
        for (r, h) in self.program.host_inputs.iter() {
            let id = format!("host:{}", self.interface.host_field(r));
            self.host_inputs.insert(
                r,
                Named {
                    id,
                    display: h.get_name(),
                },
            );
        }
    }

    /// Local input names: the variable's name, made unique within the unit in arena order.
    fn name_inputs(&mut self) {
        for (cu_ref, cu) in self.program.compute_units.iter() {
            let ComputeUnit::DensityFunction {
                function,
                density_inputs,
                ..
            } = cu;
            let mut used = HashSet::new();
            for (r, input) in density_inputs.iter() {
                let base = self.var_name_anywhere(input.var, *function);
                let name = unique_suffix(&mut used, base);
                self.inputs.insert(r, InputName { unit: cu_ref, name });
            }
        }
    }

    /// The name of `var`, looked up in `hint` first and then in every function (an inlined
    /// unit's inputs keep pointing into the variables of the now-helper function).
    fn var_name_anywhere(&self, var: Ref<Var>, hint: Ref<Function>) -> String {
        std::iter::once(hint)
            .chain(self.program.functions.iter().map(|(r, _)| r))
            .find(|f| self.program.functions[*f].variables.contains(var))
            .map(|f| self.var_names[&f][&var].clone())
            .expect("density input variable is in no function")
    }

    fn find_indexed_inputs(&mut self) {
        for f in self.program.functions.values() {
            visit_exprs(&f.body, &mut |e| {
                if let Expression::DensityVariable(r, Some(_)) = e {
                    self.indexed_inputs.insert(*r);
                }
            });
        }
    }

    /// Main functions and helpers of scheduled units, and everything they call.
    fn find_reachable_functions(&mut self) {
        let mut stack = Vec::new();
        for (r, cu) in self.program.compute_units.iter() {
            if self.interface.dimensions_of(r).is_empty() {
                continue;
            }
            let ComputeUnit::DensityFunction {
                function,
                helper_functions,
                ..
            } = cu;
            stack.push(*function);
            stack.extend(helper_functions.iter().copied());
        }
        while let Some(f) = stack.pop() {
            if self.reachable_functions.insert(f) {
                visit_exprs(&self.program.functions[f].body, &mut |e| {
                    if let Expression::FunctionCall { function, .. } = e {
                        stack.push(*function);
                    }
                });
            }
        }
    }

    pub fn unit_reachable(&self, cu: Ref<ComputeUnit>) -> bool {
        !self.interface.dimensions_of(cu).is_empty()
    }

    /// The units a function belongs to: its own unit, or every unit listing it as a helper.
    pub fn owners(&self, f: Ref<Function>) -> Vec<Ref<ComputeUnit>> {
        match self.main_of.get(&f) {
            Some(cu) => vec![*cu],
            None => self.helper_of.get(&f).cloned().unwrap_or_default(),
        }
    }

    pub fn unit_id(&self, cu: Ref<ComputeUnit>) -> String {
        self.units[&cu].id.clone()
    }

    pub fn function_id(&self, f: Ref<Function>) -> String {
        self.functions[&f].id.clone()
    }

    pub fn instance_id(&self, i: &Instance) -> String {
        self.instances[i].id.clone()
    }

    /// Bytes per element of a unit's buffer, `None` for types that can't be buffer elements.
    pub fn element_size(&self, t: &VariableType) -> Option<usize> {
        let s = &self.options.element_sizes;
        Some(match t {
            VariableType::F32 => s.f32,
            VariableType::F64 => s.f64,
            VariableType::I8 => s.i8,
            VariableType::I32 => s.i32,
            VariableType::I64 => s.i64,
            VariableType::Bool => s.bool,
            VariableType::Vec3 => s.vec3,
            VariableType::Pos3 => s.pos3,
            VariableType::Array(t, n) => self.element_size(t)? * n,
            VariableType::DensityInput
            | VariableType::PermutationTable
            | VariableType::InterpolatedNoiseSampler
            | VariableType::Extern(_) => return None,
        })
    }

    pub fn return_type(&self, cu: Ref<ComputeUnit>) -> &VariableType {
        let ComputeUnit::DensityFunction { function, .. } = &self.program.compute_units[cu];
        &self.program.functions[*function].return_type
    }

    /// Notes for the model, sorted and de-duplicated.
    fn notes(&self) -> Vec<String> {
        let mut notes = BTreeSet::new();
        for (r, _) in self.program.compute_units.iter() {
            let t = self.return_type(r);
            if self.element_size(t).is_none() {
                notes.insert(format!(
                    "{}: return type {:?} is not a valid buffer element; its buffers count as 0 bytes",
                    self.units[&r].display, t
                ));
            }
        }
        notes.into_iter().collect()
    }
}

/// Assigns ids from `(key, base id, display)`, appending `#2`, `#3` … to repeated base ids in
/// (base id, input) order. Inputs come in arena order, so the result is deterministic.
fn unique_ids<K: std::hash::Hash + Eq>(mut items: Vec<(K, String, String)>) -> HashMap<K, Named> {
    items.sort_by(|a, b| a.1.cmp(&b.1));
    let mut seen: HashMap<String, usize> = HashMap::new();
    items
        .into_iter()
        .map(|(k, id, display)| {
            let n = seen.entry(id.clone()).or_default();
            *n += 1;
            let id = if *n == 1 { id } else { format!("{id}#{n}") };
            (k, Named { id, display })
        })
        .collect()
}

/// `base`, or `base_2`, `base_3` … for the first one not in `used`.
fn unique_suffix(used: &mut HashSet<String>, base: String) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|c| used.insert(c.clone()))
        .unwrap()
}

/// Variable names of one function: `Named(s)` → `s`; `Prefixed(s)` → `s`, or `s_2` … on a
/// collision, in arena order; `Anonymous` → `_t1`, `_t2` … in arena order.
fn variable_names(f: &Function) -> HashMap<Ref<Var>, String> {
    let mut used: HashSet<String> = f
        .variables
        .values()
        .filter_map(|v| match &v.name {
            Name::Named(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let mut anonymous = 0;
    f.variables
        .iter()
        .map(|(r, v)| {
            let name = match &v.name {
                Name::Named(s) => s.clone(),
                Name::Prefixed(s) => unique_suffix(&mut used, s.clone()),
                Name::Anonymous => {
                    anonymous += 1;
                    unique_suffix(&mut used, format!("_t{anonymous}"))
                }
            };
            (r, name)
        })
        .collect()
}

pub(crate) fn fmt_triple(v: (f64, f64, f64)) -> String {
    format!("({}, {}, {})", v.0, v.1, v.2)
}

pub(crate) fn fmt_dims(d: (i32, i32, i32)) -> String {
    format!("{}x{}x{}", d.0, d.1, d.2)
}

pub(crate) fn grid_of_input(i: &DensityInput) -> Grid {
    Grid {
        dimensions: i.dimensions,
        scaled_origin: i.scaled_origin,
        scaled_position: i.scaled_position,
    }
}

pub(crate) fn grid_of_instance(i: &Instance) -> Grid {
    Grid {
        dimensions: i.dimensions,
        scaled_origin: i.scaled_origin(),
        scaled_position: i.scaled_position(),
    }
}

/// Sorts by display name, then id.
pub(crate) fn sort_named<T>(items: &mut [T], key: impl Fn(&T) -> (&str, &str)) {
    items.sort_by(|a, b| key(a).cmp(&key(b)));
}

/// Sorts ids by the display name of what they refer to (via `display`), then by id.
pub(crate) fn sort_ids(ids: &mut Vec<String>, display: &HashMap<String, String>) {
    ids.sort_by(|a, b| (display.get(a), a).cmp(&(display.get(b), b)));
    ids.dedup();
}

/// Pre-order walk over every expression of a body.
pub(crate) fn visit_exprs(stmts: &[Statement], f: &mut dyn FnMut(&Expression)) {
    for s in stmts {
        match s {
            Statement::Assign { value, .. } => visit_expr(value, f),
            Statement::Return(e) => visit_expr(e, f),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                visit_expr(condition, f);
                visit_exprs(then_branch, f);
                visit_exprs(else_branch, f);
            }
            Statement::While { condition, body } => {
                visit_expr(condition, f);
                visit_exprs(body, f);
            }
            Statement::Repeat { body, .. } => visit_exprs(body, f),
            Statement::Break => {}
        }
    }
}

fn visit_expr(e: &Expression, f: &mut dyn FnMut(&Expression)) {
    f(e);
    for child in expr_children(e) {
        visit_expr(child, f);
    }
}

/// Direct sub-expressions, in evaluation order.
pub(crate) fn expr_children(e: &Expression) -> Vec<&Expression> {
    match e {
        Expression::FunctionCall { parameters, .. } | Expression::ExternCall { parameters, .. } => {
            parameters.iter().collect()
        }
        Expression::DensityVariable(_, idx) => idx.iter().map(|i| &**i).collect(),
        Expression::BinaryOp { left, right, .. } => vec![left, right],
        Expression::UnaryOp { operand, .. } => vec![operand],
        Expression::Field { base, .. } => vec![base],
        Expression::ArrayAccess { array, index } => vec![array, index],
        Expression::Construct { args, .. } => args.iter().collect(),
        Expression::ConstructExtern { args, .. } => args.iter().map(|(_, a)| a).collect(),
        Expression::ArrayLiteral(items) => items.iter().collect(),
        Expression::ExplicitCast { expr, .. } => vec![expr],
        Expression::Constant(_)
        | Expression::Variable(_)
        | Expression::Float(_)
        | Expression::Double(_)
        | Expression::Byte(_)
        | Expression::Int(_)
        | Expression::Long(_)
        | Expression::Bool(_)
        | Expression::PermutationTable(_)
        | Expression::Ambient(_)
        | Expression::HostInput(_) => vec![],
    }
}

/// Constants a constant expression refers to directly.
pub(crate) fn const_refs(e: &ConstExpr, out: &mut Vec<Ref<Constant>>) {
    match e {
        ConstExpr::Constant(r) => out.push(*r),
        ConstExpr::BinaryOp { left, right, .. } => {
            const_refs(left, out);
            const_refs(right, out);
        }
        ConstExpr::UnaryOp { operand, .. } => const_refs(operand, out),
        ConstExpr::ExplicitCast { expr, .. } => const_refs(expr, out),
        ConstExpr::Field { base, .. } => const_refs(base, out),
        ConstExpr::ArrayAccess { array, index } => {
            const_refs(array, out);
            const_refs(index, out);
        }
        ConstExpr::ArrayLiteral(items) => items.iter().for_each(|i| const_refs(i, out)),
        ConstExpr::Construct { args, .. } => args.iter().for_each(|a| const_refs(a, out)),
        ConstExpr::Float(_)
        | ConstExpr::Double(_)
        | ConstExpr::Byte(_)
        | ConstExpr::Int(_)
        | ConstExpr::Long(_)
        | ConstExpr::Bool(_) => {}
    }
}
