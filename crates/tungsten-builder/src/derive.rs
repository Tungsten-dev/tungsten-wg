//! Unit and function lists, derived from bodies (spec §5).
//!
//! "First use" follows evaluation order, entering a callee at its first call, after the call's
//! arguments. Each finished function stores its ordered [`Summary`], so a caller merges it at the
//! call without walking the callee again.

use tungsten_ir::{
    ConstExpr, Constant, DensityInput, Expression, Function, HostInput, PermutationTableInput,
    Ref, Statement,
};

use crate::program::ProgramBuilder;

/// Everything a function reaches, in first-use order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Summary {
    /// Callees before callers.
    pub(crate) helpers: Vec<Ref<Function>>,
    pub(crate) hosts: Vec<Ref<HostInput>>,
    pub(crate) tables: Vec<Ref<PermutationTableInput>>,
    /// Dependencies first.
    pub(crate) consts: Vec<Ref<Constant>>,
    pub(crate) inputs: Vec<Ref<DensityInput>>,
}

fn add<T>(v: &mut Vec<Ref<T>>, r: Ref<T>) {
    if !v.contains(&r) {
        v.push(r);
    }
}

struct Walk<'a> {
    pb: &'a ProgramBuilder,
    s: Summary,
    direct_hosts: Vec<Ref<HostInput>>,
    foreign: Vec<&'static str>,
}

impl Walk<'_> {
    fn foreign(&mut self, what: &'static str) {
        if !self.foreign.contains(&what) {
            self.foreign.push(what);
        }
    }

    fn constant(&mut self, c: Ref<Constant>) {
        if !self.pb.program.constants.contains(c) {
            return self.foreign("a constant");
        }
        if self.s.consts.contains(&c) {
            return;
        }
        for d in &self.pb.const_deps[&c] {
            add(&mut self.s.consts, *d);
        }
        add(&mut self.s.consts, c);
    }

    fn stmts(&mut self, body: &[Statement]) {
        for s in body {
            match s {
                Statement::Assign { value, .. } => self.expr(value),
                Statement::Return(e) => self.expr(e),
                Statement::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    self.expr(condition);
                    self.stmts(then_branch);
                    self.stmts(else_branch);
                }
                Statement::While { condition, body } => {
                    self.expr(condition);
                    self.stmts(body);
                }
                Statement::Repeat { body, .. } => self.stmts(body),
                Statement::Break => {}
            }
        }
    }

    fn expr(&mut self, e: &Expression) {
        match e {
            Expression::Constant(c) => self.constant(*c),
            Expression::FunctionCall {
                function,
                parameters,
            } => {
                for p in parameters {
                    self.expr(p);
                }
                if !self.pb.program.functions.contains(*function) {
                    return self.foreign("a helper");
                }
                if self.s.helpers.contains(function) {
                    return; // merged at its first call
                }
                let callee = &self.pb.summaries[function];
                for h in &callee.helpers {
                    add(&mut self.s.helpers, *h);
                }
                add(&mut self.s.helpers, *function);
                for h in &callee.hosts {
                    add(&mut self.s.hosts, *h);
                }
                for t in &callee.tables {
                    add(&mut self.s.tables, *t);
                }
                for c in &callee.consts {
                    add(&mut self.s.consts, *c);
                }
                for i in &callee.inputs {
                    add(&mut self.s.inputs, *i);
                }
            }
            Expression::DensityVariable(r, index) => {
                if let Some(i) = index {
                    self.expr(i);
                }
                if self.pb.program.density_inputs.contains(*r) {
                    add(&mut self.s.inputs, *r);
                } else {
                    self.foreign("an input");
                }
            }
            Expression::PermutationTable(t) => {
                if self.pb.program.permutation_tables.contains(*t) {
                    add(&mut self.s.tables, *t);
                } else {
                    self.foreign("a table");
                }
            }
            Expression::HostInput(h) => {
                if self.pb.program.host_inputs.contains(*h) {
                    add(&mut self.s.hosts, *h);
                    add(&mut self.direct_hosts, *h);
                } else {
                    self.foreign("a host");
                }
            }
            Expression::Variable(_)
            | Expression::Float(_)
            | Expression::Double(_)
            | Expression::Byte(_)
            | Expression::Int(_)
            | Expression::Long(_)
            | Expression::Bool(_)
            | Expression::Ambient(_) => {}
            Expression::ExternCall { parameters, .. } => {
                for p in parameters {
                    self.expr(p);
                }
            }
            Expression::BinaryOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expression::UnaryOp { operand, .. } => self.expr(operand),
            Expression::Field { base, .. } => self.expr(base),
            Expression::ArrayAccess { array, index } => {
                self.expr(array);
                self.expr(index);
            }
            Expression::Construct { args, .. } | Expression::ArrayLiteral(args) => {
                for a in args {
                    self.expr(a);
                }
            }
            Expression::ConstructExtern { args, .. } => {
                for (_, a) in args {
                    self.expr(a);
                }
            }
            Expression::ExplicitCast { expr, .. } => self.expr(expr),
        }
    }
}

/// Summarizes a body: what it reaches (through callees too), the hosts it reads directly, and
/// the kinds of handles from another program builder it uses.
pub(crate) fn summarize(
    pb: &ProgramBuilder,
    body: &[Statement],
) -> (Summary, Vec<Ref<HostInput>>, Vec<&'static str>) {
    let mut w = Walk {
        pb,
        s: Summary::default(),
        direct_hosts: Vec::new(),
        foreign: Vec::new(),
    };
    w.stmts(body);
    (w.s, w.direct_hosts, w.foreign)
}

/// The constants `value` reads, transitively, dependencies first. Every constant it names is in
/// `pb` (the caller checked).
pub(crate) fn const_deps(pb: &ProgramBuilder, value: &ConstExpr) -> Vec<Ref<Constant>> {
    fn go(pb: &ProgramBuilder, e: &ConstExpr, out: &mut Vec<Ref<Constant>>) {
        match e {
            ConstExpr::Constant(c) => {
                if !out.contains(c) {
                    for d in &pb.const_deps[c] {
                        add(out, *d);
                    }
                    add(out, *c);
                }
            }
            ConstExpr::Float(_)
            | ConstExpr::Double(_)
            | ConstExpr::Byte(_)
            | ConstExpr::Int(_)
            | ConstExpr::Long(_)
            | ConstExpr::Bool(_) => {}
            ConstExpr::BinaryOp { left, right, .. } => {
                go(pb, left, out);
                go(pb, right, out);
            }
            ConstExpr::UnaryOp { operand, .. } => go(pb, operand, out),
            ConstExpr::ExplicitCast { expr, .. } => go(pb, expr, out),
            ConstExpr::Field { base, .. } => go(pb, base, out),
            ConstExpr::ArrayAccess { array, index } => {
                go(pb, array, out);
                go(pb, index, out);
            }
            ConstExpr::ArrayLiteral(es) | ConstExpr::Construct { args: es, .. } => {
                for e in es {
                    go(pb, e, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    go(pb, value, &mut out);
    out
}

/// Calls `f` on `e` and every subexpression, parents first.
pub(crate) fn each_child(e: &Expression, f: &mut dyn FnMut(&Expression)) {
    f(e);
    match e {
        Expression::FunctionCall { parameters, .. } | Expression::ExternCall { parameters, .. } => {
            for p in parameters {
                each_child(p, f);
            }
        }
        Expression::DensityVariable(_, Some(i)) => each_child(i, f),
        Expression::BinaryOp { left, right, .. } => {
            each_child(left, f);
            each_child(right, f);
        }
        Expression::UnaryOp { operand, .. } => each_child(operand, f),
        Expression::Field { base, .. } => each_child(base, f),
        Expression::ArrayAccess { array, index } => {
            each_child(array, f);
            each_child(index, f);
        }
        Expression::Construct { args, .. } | Expression::ArrayLiteral(args) => {
            for a in args {
                each_child(a, f);
            }
        }
        Expression::ConstructExtern { args, .. } => {
            for (_, a) in args {
                each_child(a, f);
            }
        }
        Expression::ExplicitCast { expr, .. } => each_child(expr, f),
        _ => {}
    }
}
