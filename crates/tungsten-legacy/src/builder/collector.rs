use crate::spmt::model::*;
use std::collections::HashSet;

type E<'m> = Expression<'m>;
type S<'m> = Statement<'m>;

// ============================================================================
// Dependency collection
// ============================================================================

#[derive(Default)]
pub struct Collector<'m> {
    /// Callee-before-caller order, so GLSL/WGSL-style backends can emit them as-is.
    pub helpers: Vec<FunctionRef<'m>>,
    pub seen_fns: HashSet<FunctionRef<'m>>,
    pub density_inputs: Vec<DensityInput<'m>>,
    pub seen_density_vars: HashSet<Var<'m>>,
    pub perm_tables: Vec<PermutationTableInput>,
    /// >0 while walking a helper's body. Inputs found there belong to the helper
    /// (passed as arguments), not to the enclosing density function.
    pub in_helper: u32,
}

impl<'m> Collector<'m> {
    pub fn stmts(&mut self, ss: &[Statement<'m>]) {
        for s in ss {
            self.stmt(s);
        }
    }

    pub fn stmt(&mut self, s: &Statement<'m>) {
        match s {
            S::Assign { value, .. } => self.expr(value),
            S::Return(e) => self.expr(e),
            S::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(condition);
                self.stmts(then_branch);
                self.stmts(else_branch);
            }
            S::While { condition, body } => {
                self.expr(condition);
                self.stmts(body);
            }
            S::Repeat { body, .. } => self.stmts(body),
            S::Break => {}
        }
    }

    pub fn expr(&mut self, e: &Expression<'m>) {
        match e {
            E::Variable(_) | E::Float(_) | E::Double(_) | E::Byte(_) | E::Int(_) | E::Long(_) | E::Bool(_) => {}
            E::FunctionCall {
                function,
                parameters,
            } => {
                self.function(*function);
                for p in parameters {
                    self.expr(p);
                }
            }
            E::ExternCall { parameters, .. } => {
                for p in parameters {
                    self.expr(p);
                }
            }
            E::DensityVariable(input, index) => {
                if self.in_helper == 0 && self.seen_density_vars.insert(input.var) {
                    self.density_inputs.push(input.clone());
                }
                if let Some(i) = index {
                    self.expr(i);
                }
            }
            E::PermutationTable(p) => {
                if self.in_helper == 0 {
                    self.perm_tables.push(p.clone());
                }
            }
            E::BinaryOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            E::UnaryOp { operand, .. } => self.expr(operand),
            E::Field { base, .. } => self.expr(base),
            E::ArrayAccess { array, index } => {
                self.expr(array);
                self.expr(index);
            }
            E::Construct { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            E::ConstructExtern { args, .. } => {
                for (_, a) in args {
                    self.expr(a);
                }
            }
            E::ArrayLiteral(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            E::ExplicitCast { to: _, expr } => {
                self.expr(expr);
            }
        }
    }

    pub fn function(&mut self, f: FunctionRef<'m>) {
        if !self.seen_fns.insert(f) {
            return;
        }
        self.in_helper += 1;
        self.stmts(&f.body); // callees first (post-order)
        self.in_helper -= 1;
        self.helpers.push(f);
    }
}
