use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use tungsten_ir::Name;
use tungsten_ir::model::{
    ComputeUnit, DensityInput, Expression, Function, OptimizedProgram, Ref, Statement, Var,
};

/// The grid a compute unit is evaluated over, as observed by whoever consumes it
/// (an entry point or a `DensityInput` of another compute unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridInfo {
    pub dimensions: (i32, i32, i32),
    pub scaled_origin: (f64, f64, f64),
    pub scaled_position: (f64, f64, f64),
}

impl GridInfo {
    fn of_input(i: &DensityInput) -> Self {
        Self {
            dimensions: i.dimensions,
            scaled_origin: i.scaled_origin,
            scaled_position: i.scaled_position,
        }
    }
}

/// Whether a compute unit can be inlined into its consumer, for the program as it is.
#[derive(Debug, Clone, PartialEq)]
pub enum InlineVerdict {
    /// It would be inlined into `into`, at `site`, which reads it `reads` times.
    Inlinable {
        into: Ref<ComputeUnit>,
        site: Ref<Function>,
        reads: usize,
    },
    Rejected(InlineRejection),
}

/// The first inlining criterion a compute unit fails, in the order the inliner checks them.
#[derive(Debug, Clone, PartialEq)]
pub enum InlineRejection {
    /// Entry points must stay addressable.
    EntryPoint,
    /// No compute unit reads it (and it isn't an entry point).
    Unused,
    /// Total number of `DensityInput`s pointing at it, across all compute units.
    MultipleUses(usize),
    /// Its only reader is itself.
    SelfUse,
    /// The caller is already too complex. Never produced today (the criterion is a placeholder).
    CallerTooComplex,
    /// The caller is consumed on several different grids (or not at all).
    CallerGridAmbiguous,
    /// The grid the caller reads it on differs from the grid the caller is consumed on.
    GridMismatch { caller: GridInfo, input: GridInfo },
    /// `function` reads it with an explicit index (an identity mapping is required).
    IndexedRead { function: Ref<Function> },
    /// More than one function of the caller reads it.
    ReadInSeveralFunctions,
    /// The caller declares the input but never reads it.
    NotRead,
}

/// One merge done by the inliner. Refs are in the tags of the program that was mutated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRecord {
    /// Iteration of the inliner's outer loop, from 0.
    pub round: usize,
    /// Still valid after the pass.
    pub caller: Ref<ComputeUnit>,
    /// Stale after the pass (the unit was removed).
    pub callee: Ref<ComputeUnit>,
    /// Canonical name of the callee's function, for display.
    pub callee_name: Option<String>,
    /// Now a helper of `caller`.
    pub callee_function: Ref<Function>,
    /// The caller function that read the callee.
    pub site: Ref<Function>,
    pub reads: usize,
}

/// Everything `inline` needs to know, produced by `plan` after all criteria passed.
struct InlinePlan {
    /// The one compute unit that reads the callee.
    caller: Ref<ComputeUnit>,
    /// The caller's `DensityInput` that points at the callee.
    input: Ref<DensityInput>,
    /// The (single) function of the caller that reads `input`.
    site: Ref<Function>,
    /// How many times `site` reads `input`.
    reads: usize,
}

/// One verdict per compute unit, for the program as it is (no iteration: inlining one unit can
/// change the verdicts of others).
pub fn explain_inline(program: &OptimizedProgram) -> Vec<(Ref<ComputeUnit>, InlineVerdict)> {
    let planner = Planner { program };
    program
        .compute_units
        .iter()
        .map(|(r, _)| {
            let verdict = match planner.plan(r) {
                Ok(plan) => InlineVerdict::Inlinable {
                    into: plan.caller,
                    site: plan.site,
                    reads: plan.reads,
                },
                Err(rejection) => InlineVerdict::Rejected(rejection),
            };
            (r, verdict)
        })
        .collect()
}

pub struct Inliner<'m> {
    program: &'m mut OptimizedProgram,
}

impl<'m> Inliner<'m> {
    pub fn new(program: &'m mut OptimizedProgram) -> Self {
        Self { program }
    }

    fn planner(&self) -> Planner<'_> {
        Planner {
            program: self.program,
        }
    }

    /// Runs the inlining pass until no more compute units can be inlined.
    /// Returns one record per merge, in the order they happened.
    pub fn traverse(&mut self) -> Vec<InlineRecord> {
        let mut records = Vec::new();
        for round in 0.. {
            let mut progressed = false;
            // The candidate list can go stale while we mutate (a candidate may have been
            // removed, or its caller may have changed), so everything is re-checked per item.
            for callee in self.planner().find_inlinable() {
                if self.program.compute_units.try_get(callee).is_none() {
                    continue;
                }
                if let Ok(plan) = self.planner().plan(callee) {
                    records.push(self.inline(callee, plan, round));
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        records
    }
}

/// The read-only half of the inliner: deciding what can be inlined.
struct Planner<'m> {
    program: &'m OptimizedProgram,
}

impl<'m> Planner<'m> {
    /// Finds all compute units that could be potentially inlined.
    fn find_inlinable(&self) -> Vec<Ref<ComputeUnit>> {
        // To be inlinable, the compute unit must meet these criteria:
        // 1. It may only be used once by another compute unit.
        //    (Entry points count as a use: they must stay addressable.)
        self.program
            .compute_units
            .iter()
            .map(|(r, _)| r)
            .filter(|r| self.sole_user(*r).is_ok())
            .collect()
    }

    /// Returns the one compute unit that uses `callee`, if there is exactly one use in total
    /// and the callee is not an entry point.
    fn sole_user(&self, callee: Ref<ComputeUnit>) -> Result<Ref<ComputeUnit>, InlineRejection> {
        if self
            .program
            .entry_points
            .iter()
            .any(|e| e.density_function == callee)
        {
            return Err(InlineRejection::EntryPoint);
        }
        let mut users = Vec::new();
        for (unit_ref, unit) in self.program.compute_units.iter() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = unit;
            for (_, input) in density_inputs.iter() {
                if input.density_function == callee {
                    users.push(unit_ref);
                }
            }
        }
        match users[..] {
            [] => Err(InlineRejection::Unused),
            [user] if user == callee => Err(InlineRejection::SelfUse),
            [user] => Ok(user),
            _ => Err(InlineRejection::MultipleUses(users.len())),
        }
    }

    /// The grid `unit` is consumed on. `None` if it has no consumers or they disagree.
    fn consumer_grid(&self, unit: Ref<ComputeUnit>) -> Option<GridInfo> {
        let mut grids: Vec<GridInfo> = Vec::new();
        for e in &self.program.entry_points {
            if e.density_function == unit {
                grids.push(GridInfo {
                    dimensions: e.dimensions,
                    scaled_origin: e.scaled_origin,
                    scaled_position: e.scaled_position,
                });
            }
        }
        for (_, u) in self.program.compute_units.iter() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = u;
            for (_, i) in density_inputs.iter() {
                if i.density_function == unit {
                    grids.push(GridInfo::of_input(i));
                }
            }
        }
        let first = *grids.first()?;
        grids.iter().all(|g| *g == first).then_some(first)
    }

    /// Placeholder for criterion 1 of `plan` ("caller must not already be too complex").
    /// Skipped for now.
    fn caller_too_complex(&self, _caller: &ComputeUnit) -> bool {
        false
    }

    /// Determines if the callee can be inlined into its (sole) caller.
    ///
    /// The callee must be inlinable at all (see `find_inlinable`); that is re-verified here since
    /// it is cheap and protects `inline`. Then the caller is checked:
    /// 1. The caller must not already be too complex. (skipped for now)
    /// 2. The caller and callee must have an identity index mapping (None).
    /// 3. The caller and callee must have the same output shape.
    /// Additionally (needed for correctness of the generated call):
    /// 4. The callee is read from a single function of the caller. If it is read more than
    ///    once there, `inline` computes it once into a variable at the top of that function.
    /// 5. The caller function that reads it has a signature the callee can be called with.
    ///
    /// Returns the first failing criterion, in the order checked here.
    fn plan(&self, callee: Ref<ComputeUnit>) -> Result<InlinePlan, InlineRejection> {
        let caller = self.sole_user(callee)?;
        let caller_unit = &self.program.compute_units[caller];
        let ComputeUnit::DensityFunction {
            function: caller_fn,
            density_inputs,
            helper_functions,
            ..
        } = caller_unit;

        // 1. complexity
        if self.caller_too_complex(caller_unit) {
            return Err(InlineRejection::CallerTooComplex);
        }

        // The caller's single input that points at the callee (`sole_user` counted exactly one).
        let (input_ref, input) = density_inputs
            .iter()
            .find(|(_, i)| i.density_function == callee)
            .expect("sole user has no input pointing at the callee");

        // 3. same output shape: what the caller reads must be the grid the caller itself is
        //    consumed on.
        let caller_grid = self
            .consumer_grid(caller)
            .ok_or(InlineRejection::CallerGridAmbiguous)?;
        let input_grid = GridInfo::of_input(input);
        if caller_grid != input_grid {
            return Err(InlineRejection::GridMismatch {
                caller: caller_grid,
                input: input_grid,
            });
        }

        // 2 + 4. every read is un-indexed, and all reads are in one function (the call, or the
        //    variable holding its result, is local to that function).
        let mut reads = 0;
        let mut site = None;
        for f in std::iter::once(*caller_fn).chain(helper_functions.iter().copied()) {
            let func = &self.program.functions[f];
            let (identity, indexed) = count_density_uses(&func.body, input_ref);
            if indexed > 0 {
                return Err(InlineRejection::IndexedRead { function: f });
            }
            if identity > 0 {
                if site.is_some() {
                    return Err(InlineRejection::ReadInSeveralFunctions);
                }
                site = Some(f);
            }
            reads += identity;
        }
        let site = site.ok_or(InlineRejection::NotRead)?;

        // 5. the call is made with the site function's own parameters, forwarded unchanged
        //    (identity mapping means the callee sees the same index/position), so the
        //    signatures must line up.
        // let site_fn = self.program.functions.try_get(site)?;
        // let callee_f = self.program.functions.try_get(*callee_fn)?;
        // if site_fn.parameters.len() != callee_f.parameters.len()
        //     || site_fn.return_type != callee_f.return_type
        // {
        //     return None;
        // }
        // for (a, b) in site_fn.parameters.iter().zip(&callee_f.parameters) {
        //     if site_fn.variables.try_get(*a)?.t != callee_f.variables.try_get(*b)?.t {
        //         return None;
        //     }
        // }

        Ok(InlinePlan {
            caller,
            input: input_ref,
            site,
            reads,
        })
    }
}

impl<'m> Inliner<'m> {
    /// Inlines the callee into the caller.
    ///
    /// `plan` must be the current `plan(callee)`. The callee's function becomes a helper
    /// function of the caller, the caller's read of the callee becomes a call to it, and the
    /// callee compute unit is removed from the program.
    fn inline(&mut self, callee: Ref<ComputeUnit>, plan: InlinePlan, round: usize) -> InlineRecord {
        let caller = plan.caller;
        let callee_unit = self
            .program
            .compute_units
            .remove(callee)
            .expect("callee disappeared");
        let ComputeUnit::DensityFunction {
            function: callee_fn,
            density_inputs: callee_inputs,
            permutation_table_inputs: callee_perms,
            host_inputs: callee_hosts,
            helper_functions: callee_helpers,
            constants: callee_consts,
            source_hash: callee_hash,
        } = callee_unit;

        // --- Caller compute unit: absorb the callee's inputs and dependencies. ---
        let remap: HashMap<Ref<DensityInput>, Ref<DensityInput>> = {
            let ComputeUnit::DensityFunction {
                density_inputs,
                permutation_table_inputs,
                host_inputs,
                helper_functions,
                constants,
                source_hash,
                ..
            } = self.program.compute_units.get_mut(caller);

            // The callee's density inputs live in an arena owned by the callee, so they get
            // new refs in the caller's arena.
            let mut remap = HashMap::new();
            for (old, input) in callee_inputs.iter() {
                remap.insert(old, density_inputs.insert(input.clone()));
            }
            // The caller no longer reads the callee as a density input.
            density_inputs.remove(plan.input);

            // These refs point into program-wide arenas, so they can be shared as they are.
            extend_unique(permutation_table_inputs, callee_perms.iter().copied());
            extend_unique(host_inputs, callee_hosts.iter().copied());
            extend_unique(constants, callee_consts.iter().copied());
            extend_unique(
                helper_functions,
                std::iter::once(callee_fn).chain(callee_helpers.iter().copied()),
            );

            // The unit's contents changed, so its identity for caching must change too.
            let mut h = DefaultHasher::new();
            let old_hash = *source_hash;
            old_hash.hash(&mut h);
            callee_hash.hash(&mut h);
            *source_hash = h.finish();

            remap
        };

        // --- Callee functions: point their density reads at the caller's arena. ---
        // Refs not in `remap` (e.g. in a helper shared with another unit) are left untouched.
        for f in std::iter::once(callee_fn).chain(callee_helpers.iter().copied()) {
            let func = self.program.functions.get_mut(f);
            visit_stmts_mut(&mut func.body, &mut |e| {
                if let Expression::DensityVariable(r, _) = e {
                    if let Some(new) = remap.get(&*r).copied() {
                        *r = new;
                    }
                }
            });
        }

        // --- Call site: replace the density read with a call to the (now helper) function. ---
        let args: Vec<Expression> = self.program.functions[plan.site]
            .parameters
            .iter()
            .map(|p| Expression::Variable(*p))
            .collect();
        let callee_fn_hosts = self.program.functions[callee_fn].host_inputs.clone();
        let callee_ret = self.program.functions[callee_fn].return_type.clone();
        let call = Expression::FunctionCall {
            function: callee_fn,
            parameters: args,
        };

        let site_fn = self.program.functions.get_mut(plan.site);
        // The site now (transitively) uses the callee function's host inputs.
        extend_unique(&mut site_fn.host_inputs, callee_fn_hosts);

        let is_read =
            |e: &Expression| matches!(e, Expression::DensityVariable(r, None) if *r == plan.input);
        let mut replaced = 0;
        if plan.reads == 1 {
            // A single read: call in place, so it stays lazy if it sits in a branch.
            let mut call = Some(call);
            visit_stmts_mut(&mut site_fn.body, &mut |e| {
                if is_read(e) {
                    *e = call.take().expect("read visited twice");
                    replaced += 1;
                }
            });
        } else {
            // Several reads: compute once up front into a fresh variable and read that.
            let name = Name::Named(format!(
                "inlined_density_{}",
                site_fn.variables.iter().count()
            ));
            let var = site_fn.variables.insert(Var {
                name,
                t: callee_ret,
            });
            visit_stmts_mut(&mut site_fn.body, &mut |e| {
                if is_read(e) {
                    *e = Expression::Variable(var);
                    replaced += 1;
                }
            });
            site_fn.body.insert(
                0,
                Statement::Assign {
                    target: var,
                    value: call,
                },
            );
        }
        debug_assert_eq!(
            replaced, plan.reads,
            "every read of the inlined density must be replaced"
        );

        InlineRecord {
            round,
            caller,
            callee,
            callee_name: self.program.functions[callee_fn].canonical_name.clone(),
            callee_function: callee_fn,
            site: plan.site,
            reads: plan.reads,
        }
    }
}

fn extend_unique<T: PartialEq>(dst: &mut Vec<T>, src: impl IntoIterator<Item = T>) {
    for item in src {
        if !dst.contains(&item) {
            dst.push(item);
        }
    }
}

/// Counts reads of `input` in `body`, as (without index, with index).
fn count_density_uses(body: &[Statement], input: Ref<DensityInput>) -> (usize, usize) {
    let (mut identity, mut indexed) = (0, 0);
    visit_stmts(body, &mut |e| {
        if let Expression::DensityVariable(r, idx) = e {
            if *r == input {
                if idx.is_none() {
                    identity += 1;
                } else {
                    indexed += 1;
                }
            }
        }
    });
    (identity, indexed)
}

// ---------------------------------------------------------------------------------------------
// Tree walkers. Pre-order: the callback sees a node before its children. Every variant is
// listed explicitly so adding one to the model fails to compile here instead of being skipped.
// ---------------------------------------------------------------------------------------------

fn visit_stmts(stmts: &[Statement], f: &mut dyn FnMut(&Expression)) {
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
                visit_stmts(then_branch, f);
                visit_stmts(else_branch, f);
            }
            Statement::While { condition, body } => {
                visit_expr(condition, f);
                visit_stmts(body, f);
            }
            Statement::Repeat { body, .. } => visit_stmts(body, f),
            Statement::Break => {}
        }
    }
}

fn visit_expr(e: &Expression, f: &mut dyn FnMut(&Expression)) {
    f(e);
    match e {
        Expression::FunctionCall { parameters, .. } | Expression::ExternCall { parameters, .. } => {
            for p in parameters {
                visit_expr(p, f);
            }
        }
        Expression::DensityVariable(_, idx) => {
            if let Some(i) = idx {
                visit_expr(i, f);
            }
        }
        Expression::BinaryOp { left, right, .. } => {
            visit_expr(left, f);
            visit_expr(right, f);
        }
        Expression::UnaryOp { operand, .. } => visit_expr(operand, f),
        Expression::Field { base, .. } => visit_expr(base, f),
        Expression::ArrayAccess { array, index } => {
            visit_expr(array, f);
            visit_expr(index, f);
        }
        Expression::Construct { args, .. } => {
            for a in args {
                visit_expr(a, f);
            }
        }
        Expression::ConstructExtern { args, .. } => {
            for (_, a) in args {
                visit_expr(a, f);
            }
        }
        Expression::ArrayLiteral(items) => {
            for i in items {
                visit_expr(i, f);
            }
        }
        Expression::ExplicitCast { expr, .. } => visit_expr(expr, f),
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
        | Expression::HostInput(_) => {}
    }
}

fn visit_stmts_mut(stmts: &mut [Statement], f: &mut dyn FnMut(&mut Expression)) {
    for s in stmts {
        match s {
            Statement::Assign { value, .. } => visit_expr_mut(value, f),
            Statement::Return(e) => visit_expr_mut(e, f),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                visit_expr_mut(condition, f);
                visit_stmts_mut(then_branch, f);
                visit_stmts_mut(else_branch, f);
            }
            Statement::While { condition, body } => {
                visit_expr_mut(condition, f);
                visit_stmts_mut(body, f);
            }
            Statement::Repeat { body, .. } => visit_stmts_mut(body, f),
            Statement::Break => {}
        }
    }
}

fn visit_expr_mut(e: &mut Expression, f: &mut dyn FnMut(&mut Expression)) {
    f(e);
    match e {
        Expression::FunctionCall { parameters, .. } | Expression::ExternCall { parameters, .. } => {
            for p in parameters {
                visit_expr_mut(p, f);
            }
        }
        Expression::DensityVariable(_, idx) => {
            if let Some(i) = idx {
                visit_expr_mut(i, f);
            }
        }
        Expression::BinaryOp { left, right, .. } => {
            visit_expr_mut(left, f);
            visit_expr_mut(right, f);
        }
        Expression::UnaryOp { operand, .. } => visit_expr_mut(operand, f),
        Expression::Field { base, .. } => visit_expr_mut(base, f),
        Expression::ArrayAccess { array, index } => {
            visit_expr_mut(array, f);
            visit_expr_mut(index, f);
        }
        Expression::Construct { args, .. } => {
            for a in args {
                visit_expr_mut(a, f);
            }
        }
        Expression::ConstructExtern { args, .. } => {
            for (_, a) in args {
                visit_expr_mut(a, f);
            }
        }
        Expression::ArrayLiteral(items) => {
            for i in items {
                visit_expr_mut(i, f);
            }
        }
        Expression::ExplicitCast { expr, .. } => visit_expr_mut(expr, f),
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
        | Expression::HostInput(_) => {}
    }
}
