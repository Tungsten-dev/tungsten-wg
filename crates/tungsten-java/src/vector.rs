/*
`Vec3` and `Pos3` split into their components.

Java has no value types, so every `Vec3`/`Pos3` value is a heap object, and escape analysis only
removes one when every method it reaches is inlined. The hot density functions are usually too big
to inline (`FreqInlineSize`), so the generator does the work instead:

- Every `Vec3`/`Pos3` local and parameter is three locals (`<name>_x`, `<name>_y`, `<name>_z`),
  `double` for `Vec3` and `int` for `Pos3`. A helper's vector parameter is three parameters.
- The position is the uniform `int px, py, pz` parameters, and the origin and scales are the
  `Config` fields (`config.origin.x`, ...), so neither needs an object per sample.
- Vector arithmetic, constructors, `Pos3 -> Vec3` casts, selects, field reads and equality on
  splittable operands are emitted per component, exactly as the runtime's `Vec3`/`Pos3` methods
  compute them (a `Pos3` component meeting a `Vec3` one is converted to `double` first).
- A vector is built as an object (`new Vec3(..)`) only where one is needed: an extern argument, a
  return value, a buffer element, an array or record element, or an operand that can't be split.

An expression is splittable when its components can be emitted without evaluating anything twice.
[`Split::Pure`] components have no side effects and can't throw, so they may be reordered or
dropped (field projection, equality). [`Split::Ordered`] components (a constructor with calls in
its arguments, or an operator with one such operand) are only used in order, each exactly once,
which is what `new Vec3(a, b, c)` does. An operator may combine one ordered operand with a pure
one, never two ordered ones.

A call whose vector arguments can't all be split goes through the callee's boxed adapter
(`<name>$boxed`, see [`boxed_adapter`]), which takes objects and forwards their components.
*/

use std::collections::HashSet;

use crate::converter::FunctionConverter;
use crate::expression::coerce;
use crate::model as java;
use crate::{PARAM_CONFIG, POSITION_PARAMS};
use tungsten_ir::model::{AmbientKind, BinaryOperator, Expression, Function, Ref, Statement, UnaryOperator, Var, VariableType};

use java::Expression as J;

pub const AXES: [&str; 3] = ["x", "y", "z"];

/// Suffix of a flattened method's adapter taking `Vec3`/`Pos3` objects.
pub const BOXED_SUFFIX: &str = "$boxed";

pub fn is_vector_type(t: &VariableType) -> bool {
    matches!(t, VariableType::Vec3 | VariableType::Pos3)
}

/// `F64` for `Vec3`, `I32` for `Pos3`.
pub fn component_type(t: &VariableType) -> VariableType {
    match t {
        VariableType::Vec3 => VariableType::F64,
        VariableType::Pos3 => VariableType::I32,
        other => panic!("{other:?} has no components"),
    }
}

/// The runtime class of a vector type.
pub fn class_of(t: &VariableType) -> &'static str {
    match t {
        VariableType::Vec3 => "Vec3",
        VariableType::Pos3 => "Pos3",
        other => panic!("{other:?} is not a vector type"),
    }
}

/// How the components of a splittable expression may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// No side effects and no exceptions: components may be dropped, reordered or combined.
    Pure,
    /// Each component must be evaluated exactly once, x then y then z.
    Ordered,
}

/// `start` is the index of a flattened group's x parameter and `t` its vector type.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorGroup {
    pub start: usize,
    pub t: VariableType,
}

/// `<method>$boxed`: the same parameters with each vector group taken as one object, forwarding
/// its components to `method`.
pub fn boxed_adapter(method: &java::Method, groups: &[VectorGroup]) -> java::Method {
    let mut params = vec![];
    let mut args = vec![];
    let mut i = 0;
    while i < method.params.len() {
        if let Some(g) = groups.iter().find(|g| g.start == i) {
            let name = format!("{}$v", method.params[i].1);
            for axis in AXES {
                args.push(J::field(J::name(&name), axis));
            }
            params.push((java::Type::class(class_of(&g.t)), name));
            i += 3;
        } else {
            args.push(J::name(&method.params[i].1));
            params.push(method.params[i].clone());
            i += 1;
        }
    }
    let call = J::call(None, &method.name, args);
    let body = match method.return_type {
        Some(java::Type::Void) | None => vec![java::Statement::Expr(call)],
        _ => vec![java::Statement::Return(Some(call))],
    };
    java::Method {
        doc: None,
        modifiers: "private static".into(),
        return_type: method.return_type.clone(),
        name: format!("{}{BOXED_SUFFIX}", method.name),
        params,
        body,
    }
}

/// Every variable `body` reads (as `Expression::Variable`).
pub fn read_variables(body: &[Statement]) -> HashSet<Ref<Var>> {
    let mut out = HashSet::new();
    for s in body {
        statement_reads(s, &mut out);
    }
    out
}

fn statement_reads(s: &Statement, out: &mut HashSet<Ref<Var>>) {
    match s {
        Statement::Assign { value, .. } => expression_reads(value, out),
        Statement::Return(e) => expression_reads(e, out),
        Statement::If { condition, then_branch, else_branch } => {
            expression_reads(condition, out);
            then_branch.iter().chain(else_branch).for_each(|s| statement_reads(s, out));
        }
        Statement::While { condition, body } => {
            expression_reads(condition, out);
            body.iter().for_each(|s| statement_reads(s, out));
        }
        Statement::Repeat { body, .. } => body.iter().for_each(|s| statement_reads(s, out)),
        Statement::Break => {}
    }
}

fn expression_reads(e: &Expression, out: &mut HashSet<Ref<Var>>) {
    use Expression as E;
    match e {
        E::Variable(r) => {
            out.insert(*r);
        }
        E::Constant(_)
        | E::Float(_)
        | E::Double(_)
        | E::Byte(_)
        | E::Int(_)
        | E::Long(_)
        | E::Bool(_)
        | E::AutoConst(_)
        | E::HostInput(_)
        | E::Ambient(_) => {}
        E::FunctionCall { parameters, .. } | E::ExternCall { parameters, .. } => {
            parameters.iter().for_each(|p| expression_reads(p, out))
        }
        E::DensityVariable(_, index) => {
            if let Some(i) = index {
                expression_reads(i, out)
            }
        }
        E::BinaryOp { left, right, .. } => {
            expression_reads(left, out);
            expression_reads(right, out);
        }
        E::UnaryOp { operand, .. } => expression_reads(operand, out),
        E::Field { base, .. } => expression_reads(base, out),
        E::ArrayAccess { array, index } => {
            expression_reads(array, out);
            expression_reads(index, out);
        }
        E::Construct { args, .. } | E::ArrayLiteral(args) => args.iter().for_each(|a| expression_reads(a, out)),
        E::ConstructExtern { args, .. } => args.iter().for_each(|(_, a)| expression_reads(a, out)),
        E::ExplicitCast { expr, .. } => expression_reads(expr, out),
        E::Eval { position, .. } => expression_reads(position, out),
        E::Select { cond, then, otherwise } => {
            expression_reads(cond, out);
            expression_reads(then, out);
            expression_reads(otherwise, out);
        }
    }
}

/// Every value assigned to `r` in `body`.
fn assigned_values<'b>(body: &'b [Statement], r: Ref<Var>, out: &mut Vec<&'b Expression>) {
    for s in body {
        match s {
            Statement::Assign { target, value } if *target == r => out.push(value),
            Statement::If { then_branch, else_branch, .. } => {
                assigned_values(then_branch, r, out);
                assigned_values(else_branch, r, out);
            }
            Statement::While { body, .. } | Statement::Repeat { body, .. } => assigned_values(body, r, out),
            _ => {}
        }
    }
}

impl<'a> FunctionConverter<'a> {
    /// The locals of `function` that are never read and only get pure values: their stores are
    /// dropped and they aren't declared. The frontend computes values such as `rpos3` in every
    /// function, read or not.
    pub fn dead_locals(&self, function: &Function) -> HashSet<Ref<Var>> {
        let read = read_variables(&function.body);
        function
            .variables
            .iter()
            .map(|(r, _)| r)
            .filter(|r| !function.parameters.contains(r) && !read.contains(r))
            .filter(|r| {
                let mut values = vec![];
                assigned_values(&function.body, *r, &mut values);
                values.iter().all(|v| self.is_pure(v))
            })
            .collect()
    }

    /// The component locals of vector variable `r`, named on first use.
    pub fn declare_vector_var(&mut self, r: Ref<Var>) -> [String; 3] {
        if let Some(names) = self.vector_vars.get(&r) {
            return names.clone();
        }
        let base = match &self.function.variables[r].name {
            tungsten_ir::model::Name::Named(n) | tungsten_ir::model::Name::Prefixed(n) => n.clone(),
            tungsten_ir::model::Name::Anonymous => "var".to_string(),
        };
        let names = AXES.map(|a| self.fresh_name(&format!("{base}_{a}")));
        self.vector_vars.insert(r, names.clone());
        names
    }

    /// The vector type of `e`, if it has one.
    pub fn vector_type(&self, e: &Expression) -> Option<VariableType> {
        self.derive_type(e).filter(is_vector_type)
    }

    /// Whether `e` has no side effects and can't throw. Calls, buffer and array reads, integer
    /// division and references are not.
    pub fn is_pure(&self, e: &Expression) -> bool {
        use Expression as E;
        match e {
            E::Constant(_)
            | E::Variable(_)
            | E::Float(_)
            | E::Double(_)
            | E::Byte(_)
            | E::Int(_)
            | E::Long(_)
            | E::Bool(_)
            | E::AutoConst(_)
            | E::HostInput(_)
            | E::Ambient(_) => true,
            E::FunctionCall { .. }
            | E::ExternCall { .. }
            | E::DensityVariable(..)
            | E::ArrayAccess { .. }
            | E::ConstructExtern { .. }
            | E::ArrayLiteral(_)
            | E::Eval { .. } => false,
            E::BinaryOp { op, left, right } => {
                let integer = |t: Option<VariableType>| {
                    matches!(t, Some(VariableType::I8 | VariableType::I32 | VariableType::I64 | VariableType::Pos3))
                };
                let traps = *op == BinaryOperator::Divide && integer(self.derive_type(left));
                !traps && self.is_pure(left) && self.is_pure(right)
            }
            E::UnaryOp { op: UnaryOperator::Ref(_), .. } => false,
            E::UnaryOp { operand, .. } => self.is_pure(operand),
            E::Field { base, .. } => self.is_pure(base),
            E::Construct { t, args } => is_vector_type(t) && args.iter().all(|a| self.is_pure(a)),
            E::ExplicitCast { expr, .. } => self.is_pure(expr),
            E::Select { cond, then, otherwise } => {
                self.is_pure(cond) && self.is_pure(then) && self.is_pure(otherwise)
            }
        }
    }

    /// Whether `e` is cheap and pure enough to be emitted once per component.
    fn is_simple(&self, e: &Expression) -> bool {
        use Expression as E;
        match e {
            E::Constant(_)
            | E::Float(_)
            | E::Double(_)
            | E::Byte(_)
            | E::Int(_)
            | E::Long(_)
            | E::Bool(_)
            | E::AutoConst(_)
            | E::HostInput(_) => true,
            E::Variable(r) => !is_vector_type(&self.function.variables[*r].t),
            E::Field { base, .. } => {
                matches!(**base, E::Variable(_) | E::Ambient(_) | E::HostInput(_) | E::AutoConst(_))
            }
            E::UnaryOp { op: UnaryOperator::Negate | UnaryOperator::Not, operand } => self.is_simple(operand),
            E::ExplicitCast { expr, .. } => self.is_simple(expr),
            _ => false,
        }
    }

    /// Whether (and how) vector expression `e` can be split; see the module docs.
    pub fn can_split(&self, e: &Expression) -> Option<Split> {
        use Expression as E;
        match e {
            E::Variable(r) => is_vector_type(&self.function.variables[*r].t).then_some(Split::Pure),
            E::Ambient(_) => Some(Split::Pure),
            E::Construct { t, args } if is_vector_type(t) && args.len() == 3 => {
                Some(if args.iter().all(|a| self.is_pure(a)) { Split::Pure } else { Split::Ordered })
            }
            E::BinaryOp { op, left, right }
                if matches!(
                    op,
                    BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide
                ) =>
            {
                let (lt, rt) = (self.vector_type(left)?, self.vector_type(right)?);
                // `Pos3 / Pos3` can throw, and the runtime has no such method.
                if *op == BinaryOperator::Divide && lt == VariableType::Pos3 && rt == VariableType::Pos3 {
                    return None;
                }
                // One ordered side keeps its order: the other side's components are pure, so
                // interleaving them changes nothing. Two would interleave their effects.
                match (self.can_split(left)?, self.can_split(right)?) {
                    (Split::Pure, Split::Pure) => Some(Split::Pure),
                    (Split::Ordered, Split::Ordered) => None,
                    _ => Some(Split::Ordered),
                }
            }
            E::Select { cond, then, otherwise } => {
                self.vector_type(e)?;
                let pure = |s: &Expression| self.can_split(s) == Some(Split::Pure);
                (self.is_simple(cond) && pure(then) && pure(otherwise)).then_some(Split::Pure)
            }
            E::ExplicitCast { to: VariableType::Vec3, expr } if self.vector_type(expr) == Some(VariableType::Pos3) => {
                self.can_split(expr)
            }
            _ => None,
        }
    }

    /// The components of `e`, which [`Self::can_split`] accepts, in its component type.
    pub fn split(&mut self, e: &Expression) -> [J; 3] {
        use Expression as E;
        match e {
            E::Variable(r) => self.declare_vector_var(*r).map(J::Name),
            E::Ambient(k) => {
                let field = match k {
                    AmbientKind::Pos3 => return POSITION_PARAMS.map(J::name),
                    AmbientKind::Origin => "origin",
                    AmbientKind::OriginScale => "origin_scale",
                    AmbientKind::PositionScale => "position_scale",
                };
                AXES.map(|a| J::field(J::field(J::name(PARAM_CONFIG), field), a))
            }
            E::Construct { t, args } => {
                let ct = component_type(t);
                [0, 1, 2].map(|i| self.convert_to_type(&args[i], &ct))
            }
            E::BinaryOp { op, left, right } => {
                let (lt, rt) = (self.vector_type(left).unwrap(), self.vector_type(right).unwrap());
                let real = lt == VariableType::Vec3 || rt == VariableType::Vec3;
                let widen = |c: J, t: &VariableType| {
                    if real && *t == VariableType::Pos3 { J::cast(java::Type::Double, c) } else { c }
                };
                let l = self.split(left).map(|c| widen(c, &lt));
                let r = self.split(right).map(|c| widen(c, &rt));
                let op = crate::types::convert_binary_op(*op);
                let [lx, ly, lz] = l;
                let [rx, ry, rz] = r;
                [J::binary(op, lx, rx), J::binary(op, ly, ry), J::binary(op, lz, rz)]
            }
            // The scalar `tw$select` overload per component: eager, like every select.
            E::Select { cond, then, otherwise } => {
                let ct = component_type(&self.vector_type(e).unwrap());
                if !self.used_selects.contains(&ct) {
                    self.used_selects.push(ct);
                }
                let c = self.convert_expression(cond);
                let [ax, ay, az] = self.split(then);
                let [bx, by, bz] = self.split(otherwise);
                let pick = |a: J, b: J| J::call(None, crate::expression::SELECT_FN, vec![c.clone(), a, b]);
                [pick(ax, bx), pick(ay, by), pick(az, bz)]
            }
            E::ExplicitCast { expr, .. } => self.split(expr).map(|c| J::cast(java::Type::Double, c)),
            _ => unreachable!("split of an expression can_split rejects"),
        }
    }

    /// `new Vec3(x, y, z)` (or `Pos3`) of splittable `e`.
    pub fn materialize(&mut self, e: &Expression, t: &VariableType) -> J {
        J::New { class: class_of(t).into(), args: self.split(e).into(), comment: None }
    }

    /// Component `axis` of `e` when it is splittable and pure.
    pub fn project(&mut self, e: &Expression, axis: usize) -> Option<J> {
        if self.can_split(e) != Some(Split::Pure) {
            return None;
        }
        let [x, y, z] = self.split(e);
        Some([x, y, z].into_iter().nth(axis).unwrap())
    }

    /// `a == b` (or `!=`) per component, with the runtime's `equals` semantics (`==` on each
    /// component). `None` unless both sides are pure splittable vectors of one type.
    pub fn vector_equality(&mut self, equal: bool, left: &Expression, right: &Expression) -> Option<J> {
        let t = self.vector_type(left)?;
        if self.vector_type(right)? != t
            || self.can_split(left) != Some(Split::Pure)
            || self.can_split(right) != Some(Split::Pure)
        {
            return None;
        }
        let l = self.split(left);
        let r = self.split(right);
        let (cmp, join) = if equal {
            (java::BinaryOperator::Equal, java::BinaryOperator::And)
        } else {
            (java::BinaryOperator::NotEqual, java::BinaryOperator::Or)
        };
        let [a, b, c] = [0, 1, 2].map(|i| J::binary(cmp, l[i].clone(), r[i].clone()));
        Some(J::binary(join, J::binary(join, a, b), c))
    }

    /// The statements of `target = value` for a vector variable.
    pub fn assign_vector(&mut self, target: Ref<Var>, value: &Expression) -> Vec<java::Statement> {
        use java::Statement as S;
        let t = self.var_type(target).clone();
        let ct = crate::types::convert_type(&component_type(&t));
        let names = self.declare_vector_var(target);
        let assign = |name: &str, value: J| S::Assign { target: J::name(name), value };
        let split = self.can_split(value).is_some();
        let (components, out) = if split {
            let components = self.split(value);
            let (out, post, written_back) = self.take_hoisted();
            if written_back.contains(&target) {
                panic!("Ref(true) on the variable that the same statement assigns is ambiguous");
            }
            (components, (out, post))
        } else {
            let object = coerce(self.convert_expression(value), self.derive_type(value), &t);
            let (mut out, post, _) = self.take_hoisted();
            let tmp = self.fresh_name(&format!("{}$v", names[0]));
            out.push(S::Declare {
                t: java::Type::class(class_of(&t)),
                name: tmp.clone(),
                init: Some(object),
                is_final: true,
            });
            (AXES.map(|a| J::field(J::name(&tmp), a)), (out, post))
        };
        let (mut out, post) = out;
        let mut reads = HashSet::new();
        expression_reads(value, &mut reads);
        if split && reads.contains(&target) {
            // `v = vec3(v.y, v.x, ..)`: every component is computed before any is stored.
            let temps: Vec<String> = names.iter().map(|n| self.fresh_name(&format!("{n}$next"))).collect();
            for (tmp, c) in temps.iter().zip(components) {
                out.push(S::Declare { t: ct.clone(), name: tmp.clone(), init: Some(c), is_final: true });
            }
            for (name, tmp) in names.iter().zip(&temps) {
                out.push(assign(name, J::name(tmp)));
            }
        } else {
            for (name, c) in names.iter().zip(components) {
                out.push(assign(name, c));
            }
        }
        out.extend(post);
        out
    }
}
