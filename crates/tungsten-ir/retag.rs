//! Deep clones that give every arena a fresh tag and rewrite the refs into it.
//!
//! Refs are rewritten by tag alone (old tag → new tag), so the walker doesn't need to know which
//! arena a ref points into. Every `Ref`-holding field of the IR is listed here explicitly; a new
//! one must be added, or clones will keep pointing into the source program.

use std::collections::HashMap;

use crate::model::*;

/// Every kind of `Ref` the IR embeds, as `method: Target, field, "readable name", ordinal;`.
/// Calls `$m!` with the list, so each `RefVisitor` impl is written once for all kinds.
macro_rules! ref_kinds {
    ($m:ident) => {
        $m! {
            compute_unit: ComputeUnit, cus, "compute unit", 1;
            function: Function, fns, "function", 2;
            constant: Constant, consts, "constant", 3;
            host_input: HostInput, hosts, "host input", 4;
            table: PermutationTableInput, tables, "permutation table", 5;
            var: Var, vars, "variable", 6;
            density_input: DensityInput, inputs, "density input", 7;
        }
    };
}
pub(crate) use ref_kinds;

macro_rules! visitor {
    ($($method:ident: $t:ty, $field:ident, $what:literal, $ord:literal;)*) => {
        /// Visits every `Ref` embedded in the IR, by type.
        pub(crate) trait RefVisitor {
            $(fn $method(&mut self, r: &mut Ref<$t>);)*
        }

        impl RefVisitor for Retag {
            $(fn $method(&mut self, r: &mut Ref<$t>) {
                self.retag(r)
            })*
        }
    };
}
ref_kinds!(visitor);

/// Rewrites tags through `map`. With `strict`, a tag missing from the map panics (used for whole
/// programs, where every tag must be known); otherwise it is left as is (partial clones, whose
/// refs into program-wide arenas stay valid).
struct Retag {
    map: HashMap<OwnerTag, OwnerTag>,
    strict: bool,
}

impl Retag {
    fn retag<T>(&self, r: &mut Ref<T>) {
        match self.map.get(&r.tag) {
            Some(new) => r.tag = *new,
            None if self.strict => panic!("Ref into an arena outside the cloned program"),
            None => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Raw clones: fresh tags for the nested arenas, embedded refs untouched.
// ---------------------------------------------------------------------------------------------

impl ComputeUnit {
    pub(crate) fn clone_raw(&self) -> Self {
        let ComputeUnit::DensityFunction {
            function,
            density_inputs,
            permutation_table_inputs,
            host_inputs,
            helper_functions,
            constants,
            source_hash,
        } = self;
        ComputeUnit::DensityFunction {
            function: *function,
            density_inputs: density_inputs.clone(),
            permutation_table_inputs: permutation_table_inputs.clone(),
            host_inputs: host_inputs.clone(),
            helper_functions: helper_functions.clone(),
            constants: constants.clone(),
            source_hash: *source_hash,
        }
    }

    fn density_inputs(&self) -> &Arena<DensityInput> {
        let ComputeUnit::DensityFunction { density_inputs, .. } = self;
        density_inputs
    }
}

impl Function {
    pub(crate) fn clone_raw(&self) -> Self {
        Self {
            canonical_name: self.canonical_name.clone(),
            host_inputs: self.host_inputs.clone(),
            body: self.body.clone(),
            variables: self.variables.clone(),
            parameters: self.parameters.clone(),
            return_type: self.return_type.clone(),
            source_hash: self.source_hash,
        }
    }
}

impl Clone for ComputeUnit {
    /// Refs into the unit's own `density_inputs` are rewritten; refs into program-wide arenas
    /// (and `DensityInput::var`, which points into a function's arena) are kept.
    fn clone(&self) -> Self {
        let mut c = self.clone_raw();
        let mut retag = Retag {
            map: HashMap::from([(self.density_inputs().tag(), c.density_inputs().tag())]),
            strict: false,
        };
        walk_compute_unit(&mut c, &mut retag);
        c
    }
}

impl Clone for Function {
    /// Refs into the function's own `variables` are rewritten; all other refs are kept.
    fn clone(&self) -> Self {
        let mut c = self.clone_raw();
        let mut retag = Retag {
            map: HashMap::from([(self.variables.tag(), c.variables.tag())]),
            strict: false,
        };
        walk_function(&mut c, &mut retag);
        c
    }
}

impl Clone for OptimizedProgram {
    fn clone(&self) -> Self {
        let mut c = OptimizedProgram {
            compute_units: self.compute_units.clone_with(ComputeUnit::clone_raw),
            functions: self.functions.clone_with(Function::clone_raw),
            entry_points: self.entry_points.clone(),
            constants: self.constants.clone(),
            host_inputs: self.host_inputs.clone(),
            permutation_tables: self.permutation_tables.clone(),
        };

        let mut map = HashMap::from([
            (self.compute_units.tag(), c.compute_units.tag()),
            (self.functions.tag(), c.functions.tag()),
            (self.constants.tag(), c.constants.tag()),
            (self.host_inputs.tag(), c.host_inputs.tag()),
            (self.permutation_tables.tag(), c.permutation_tables.tag()),
        ]);
        // `clone_with` keeps every slot in place, so live values pair up in iteration order.
        for ((_, old), (_, new)) in self.compute_units.iter().zip(c.compute_units.iter()) {
            map.insert(old.density_inputs().tag(), new.density_inputs().tag());
        }
        for ((_, old), (_, new)) in self.functions.iter().zip(c.functions.iter()) {
            map.insert(old.variables.tag(), new.variables.tag());
        }

        walk_program(&mut c, &mut Retag { map, strict: true });
        c
    }
}

// ---------------------------------------------------------------------------------------------
// Walkers. Every variant is listed explicitly so adding one fails to compile here.
// ---------------------------------------------------------------------------------------------

pub(crate) fn walk_program(p: &mut OptimizedProgram, v: &mut impl RefVisitor) {
    for e in &mut p.entry_points {
        v.compute_unit(&mut e.density_function);
    }
    for cu in p.compute_units.values_mut() {
        walk_compute_unit(cu, v);
    }
    for f in p.functions.values_mut() {
        walk_function(f, v);
    }
    for c in p.constants.values_mut() {
        walk_const_expr(&mut c.value, v);
    }
}

fn walk_compute_unit(cu: &mut ComputeUnit, v: &mut impl RefVisitor) {
    let ComputeUnit::DensityFunction {
        function,
        density_inputs,
        permutation_table_inputs,
        host_inputs,
        helper_functions,
        constants,
        source_hash: _,
    } = cu;
    v.function(function);
    for input in density_inputs.values_mut() {
        v.var(&mut input.var);
        v.compute_unit(&mut input.density_function);
    }
    permutation_table_inputs.iter_mut().for_each(|r| v.table(r));
    host_inputs.iter_mut().for_each(|r| v.host_input(r));
    helper_functions.iter_mut().for_each(|r| v.function(r));
    constants.iter_mut().for_each(|r| v.constant(r));
}

fn walk_function(f: &mut Function, v: &mut impl RefVisitor) {
    f.host_inputs.iter_mut().for_each(|r| v.host_input(r));
    f.parameters.iter_mut().for_each(|r| v.var(r));
    walk_stmts(&mut f.body, v);
}

fn walk_stmts(stmts: &mut [Statement], v: &mut impl RefVisitor) {
    for s in stmts {
        match s {
            Statement::Assign { target, value } => {
                v.var(target);
                walk_expr(value, v);
            }
            Statement::Return(e) => walk_expr(e, v),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                walk_expr(condition, v);
                walk_stmts(then_branch, v);
                walk_stmts(else_branch, v);
            }
            Statement::While { condition, body } => {
                walk_expr(condition, v);
                walk_stmts(body, v);
            }
            Statement::Repeat { count: _, body } => walk_stmts(body, v),
            Statement::Break => {}
        }
    }
}

fn walk_expr(e: &mut Expression, v: &mut impl RefVisitor) {
    match e {
        Expression::Constant(r) => v.constant(r),
        Expression::Variable(r) => v.var(r),
        Expression::FunctionCall {
            function,
            parameters,
        } => {
            v.function(function);
            parameters.iter_mut().for_each(|p| walk_expr(p, v));
        }
        Expression::ExternCall { parameters, .. } => {
            parameters.iter_mut().for_each(|p| walk_expr(p, v));
        }
        Expression::DensityVariable(r, idx) => {
            v.density_input(r);
            if let Some(i) = idx {
                walk_expr(i, v);
            }
        }
        Expression::PermutationTable(r) => v.table(r),
        Expression::HostInput(r) => v.host_input(r),
        Expression::Ambient(_) => {}
        Expression::BinaryOp { left, right, .. } => {
            walk_expr(left, v);
            walk_expr(right, v);
        }
        Expression::UnaryOp { operand, .. } => walk_expr(operand, v),
        Expression::Field { base, .. } => walk_expr(base, v),
        Expression::ArrayAccess { array, index } => {
            walk_expr(array, v);
            walk_expr(index, v);
        }
        Expression::Construct { args, .. } => args.iter_mut().for_each(|a| walk_expr(a, v)),
        Expression::ConstructExtern { args, .. } => {
            args.iter_mut().for_each(|(_, a)| walk_expr(a, v))
        }
        Expression::ArrayLiteral(items) => items.iter_mut().for_each(|i| walk_expr(i, v)),
        Expression::ExplicitCast { expr, .. } => walk_expr(expr, v),
        Expression::Float(_)
        | Expression::Double(_)
        | Expression::Byte(_)
        | Expression::Int(_)
        | Expression::Long(_)
        | Expression::Bool(_) => {}
    }
}

fn walk_const_expr(e: &mut ConstExpr, v: &mut impl RefVisitor) {
    match e {
        ConstExpr::Constant(r) => v.constant(r),
        ConstExpr::BinaryOp { left, right, .. } => {
            walk_const_expr(left, v);
            walk_const_expr(right, v);
        }
        ConstExpr::UnaryOp { operand, .. } => walk_const_expr(operand, v),
        ConstExpr::ExplicitCast { expr, .. } => walk_const_expr(expr, v),
        ConstExpr::Field { base, .. } => walk_const_expr(base, v),
        ConstExpr::ArrayAccess { array, index } => {
            walk_const_expr(array, v);
            walk_const_expr(index, v);
        }
        ConstExpr::ArrayLiteral(items) => items.iter_mut().for_each(|i| walk_const_expr(i, v)),
        ConstExpr::Construct { args, .. } => args.iter_mut().for_each(|a| walk_const_expr(a, v)),
        ConstExpr::Float(_)
        | ConstExpr::Double(_)
        | ConstExpr::Byte(_)
        | ConstExpr::Int(_)
        | ConstExpr::Long(_)
        | ConstExpr::Bool(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects every embedded ref, by type.
    #[derive(Default)]
    struct Collect {
        cus: Vec<Ref<ComputeUnit>>,
        fns: Vec<Ref<Function>>,
        consts: Vec<Ref<Constant>>,
        hosts: Vec<Ref<HostInput>>,
        tables: Vec<Ref<PermutationTableInput>>,
        vars: Vec<Ref<Var>>,
        inputs: Vec<Ref<DensityInput>>,
    }

    impl RefVisitor for Collect {
        fn compute_unit(&mut self, r: &mut Ref<ComputeUnit>) {
            self.cus.push(*r)
        }
        fn function(&mut self, r: &mut Ref<Function>) {
            self.fns.push(*r)
        }
        fn constant(&mut self, r: &mut Ref<Constant>) {
            self.consts.push(*r)
        }
        fn host_input(&mut self, r: &mut Ref<HostInput>) {
            self.hosts.push(*r)
        }
        fn table(&mut self, r: &mut Ref<PermutationTableInput>) {
            self.tables.push(*r)
        }
        fn var(&mut self, r: &mut Ref<Var>) {
            self.vars.push(*r)
        }
        fn density_input(&mut self, r: &mut Ref<DensityInput>) {
            self.inputs.push(*r)
        }
    }

    fn var(f: &mut Function, name: &str, t: VariableType) -> Ref<Var> {
        f.variables.insert(Var {
            name: Name::Named(name.into()),
            t,
        })
    }

    fn function(name: &str) -> Function {
        Function {
            canonical_name: Some(name.into()),
            host_inputs: vec![],
            body: vec![],
            variables: Arena::new(),
            parameters: vec![],
            return_type: VariableType::F64,
            source_hash: 0,
        }
    }

    /// leaf <- root (reads leaf, calls a helper that also reads leaf through root's inputs),
    /// plus a constant depending on a constant, a host input and a table.
    fn program() -> OptimizedProgram {
        let mut p = OptimizedProgram {
            compute_units: Arena::new(),
            functions: Arena::new(),
            entry_points: vec![],
            constants: Arena::new(),
            host_inputs: Arena::new(),
            permutation_tables: Arena::new(),
        };
        let a = p.constants.insert(Constant {
            value: ConstExpr::Double(0.5),
            name: "a".into(),
        });
        let b = p.constants.insert(Constant {
            value: ConstExpr::BinaryOp {
                op: BinaryOperator::Multiply,
                left: Box::new(ConstExpr::Constant(a)),
                right: Box::new(ConstExpr::Double(2.0)),
            },
            name: "b".into(),
        });
        let seed = p.host_inputs.insert(HostInput::Static {
            name: Name::Named("seed".into()),
            t: VariableType::I64,
        });
        let table = p
            .permutation_tables
            .insert(PermutationTableInput::Base3DNoise);

        // Remove one slot so generations and the free list are exercised.
        let gone = p.functions.insert(function("gone"));
        p.functions.remove(gone);

        let mut leaf_fn = function("leaf");
        leaf_fn.body = vec![Statement::Return(Expression::Constant(b))];
        let leaf_fn = p.functions.insert(leaf_fn);
        let leaf = p.compute_units.insert(ComputeUnit::DensityFunction {
            function: leaf_fn,
            density_inputs: Arena::new(),
            permutation_table_inputs: vec![],
            host_inputs: vec![],
            helper_functions: vec![],
            constants: vec![b],
            source_hash: 1,
        });

        let mut root_fn = function("root");
        let input_var = var(&mut root_fn, "in0", VariableType::DensityInput);
        let x = var(&mut root_fn, "x", VariableType::F64);
        let mut inputs = Arena::new();
        let input = inputs.insert(DensityInput {
            var: input_var,
            density_function: leaf,
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
            dimensions: (4, 4, 4),
        });

        let mut helper = function("helper");
        let t = var(&mut helper, "t", VariableType::F64);
        helper.parameters = vec![t];
        helper.host_inputs = vec![seed];
        helper.body = vec![Statement::Return(Expression::BinaryOp {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Variable(t)),
            right: Box::new(Expression::DensityVariable(
                input,
                Some(Box::new(Expression::HostInput(seed))),
            )),
        })];
        let helper = p.functions.insert(helper);

        root_fn.body = vec![
            Statement::Assign {
                target: x,
                value: Expression::ExternCall {
                    function_name: "sample".into(),
                    parameters: vec![
                        Expression::PermutationTable(table),
                        Expression::DensityVariable(input, None),
                    ],
                    parameter_types: vec![VariableType::PermutationTable, VariableType::F64],
                    return_type: VariableType::F64,
                },
            },
            Statement::Repeat {
                count: 2,
                body: vec![Statement::If {
                    condition: Expression::Variable(x),
                    then_branch: vec![Statement::Break],
                    else_branch: vec![Statement::Assign {
                        target: x,
                        value: Expression::FunctionCall {
                            function: helper,
                            parameters: vec![Expression::Variable(x)],
                        },
                    }],
                }],
            },
            Statement::Return(Expression::Variable(x)),
        ];
        let root_fn = p.functions.insert(root_fn);
        let root = p.compute_units.insert(ComputeUnit::DensityFunction {
            function: root_fn,
            density_inputs: inputs,
            permutation_table_inputs: vec![table],
            host_inputs: vec![seed],
            helper_functions: vec![helper],
            constants: vec![],
            source_hash: 2,
        });
        p.entry_points.push(EntryPoint {
            density_function: root,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        });
        p
    }

    #[test]
    fn clone_retags_every_ref() {
        let p = program();
        let mut c = p.clone();

        let mut refs = Collect::default();
        walk_program(&mut c, &mut refs);
        assert!(!refs.vars.is_empty() && !refs.inputs.is_empty());

        // Every embedded ref resolves in the clone's own arenas.
        for r in &refs.cus {
            c.compute_units.get(*r);
        }
        for r in &refs.fns {
            c.functions.get(*r);
        }
        for r in &refs.consts {
            c.constants.get(*r);
        }
        for r in &refs.hosts {
            c.host_inputs.get(*r);
        }
        for r in &refs.tables {
            c.permutation_tables.get(*r);
        }
        for r in &refs.vars {
            assert!(c.functions.values().any(|f| f.variables.contains(*r)));
        }
        for r in &refs.inputs {
            assert!(
                c.compute_units
                    .values()
                    .any(|cu| cu.density_inputs().contains(*r))
            );
        }

        // Top-level refs translate back to the originals.
        fn back<T>(orig: &Arena<T>, clone: &Arena<T>) {
            for ((o, _), (n, _)) in orig.iter().zip(clone.iter()) {
                assert_ne!(o, n);
                assert_eq!(orig.translate(n, clone), o);
            }
        }
        back(&p.compute_units, &c.compute_units);
        back(&p.functions, &c.functions);
        back(&p.constants, &c.constants);
        back(&p.host_inputs, &c.host_inputs);
        back(&p.permutation_tables, &c.permutation_tables);

        // The removed slot stays removed, and its stale ref is stale in the clone too.
        assert_eq!(p.functions.iter().count(), c.functions.iter().count());

        // A clone of a clone still translates back to the original.
        let cc = c.clone();
        let (r, _) = cc.compute_units.iter().next().unwrap();
        let o = p.compute_units.translate(r, &cc.compute_units);
        assert!(p.compute_units.contains(o));
    }

    #[test]
    #[should_panic(expected = "not clones")]
    fn translate_rejects_unrelated_arenas() {
        let a = program();
        let b = program();
        let (r, _) = b.compute_units.iter().next().unwrap();
        a.compute_units.translate(r, &b.compute_units);
    }

    #[test]
    fn function_clone_keeps_outside_refs() {
        let p = program();
        for (_, f) in p.functions.iter() {
            let c = f.clone();
            assert_ne!(c.variables.tag(), f.variables.tag());
            for r in &c.parameters {
                c.variables.get(*r);
            }
            assert_eq!(c.host_inputs, f.host_inputs);
        }
    }
}
