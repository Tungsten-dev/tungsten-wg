//! Statements and expressions.

use tungsten_ir::*;

use super::types::*;
use super::{Ctx, R, err};
use crate::ast::{self, Expr, ExprKind, Label, LitKind, StmtKind};
use crate::numlit;
use crate::source_map::{ExprOwner, ExprPath, ExprStep, StmtPath, StmtStep};
use crate::stack;
use crate::tables::ambient_type;

/// Where an expression lives.
pub(super) struct Scope {
    f: Option<Ref<Function>>,
    c: Option<Ref<Constant>>,
    stmt: StmtPath,
}

impl Scope {
    pub fn constant(c: Ref<Constant>) -> Scope {
        Scope {
            f: None,
            c: Some(c),
            stmt: StmtPath::default(),
        }
    }
}

type Typed = (Expression, Option<VariableType>);

impl<'a> Ctx<'a> {
    pub(super) fn stmts(
        &mut self,
        f: Ref<Function>,
        list: &[ast::Stmt],
        path: &mut Vec<StmtStep>,
    ) -> R<Vec<Statement>> {
        stack::grow(|| self.stmts_inner(f, list, path))
    }

    fn stmts_inner(
        &mut self,
        f: Ref<Function>,
        list: &[ast::Stmt],
        path: &mut Vec<StmtStep>,
    ) -> R<Vec<Statement>> {
        let mut out: Vec<Statement> = vec![];
        for s in list {
            // A declaration-only `let` produces no statement.
            if let StmtKind::Let { init: None, .. } = &s.kind {
                continue;
            }
            path.push(StmtStep::Index(out.len()));
            self.map
                .statements
                .insert((f, StmtPath(path.clone())), s.span);
            let sc = Scope {
                f: Some(f),
                c: None,
                stmt: StmtPath(path.clone()),
            };
            let st = match &s.kind {
                StmtKind::Let { label, init, .. } => {
                    let target = self.lookup_var(f, label)?;
                    let (value, _) = self.expr(&sc, init.as_ref().unwrap(), &mut vec![])?;
                    Statement::Assign { target, value }
                }
                StmtKind::Assign { target, value } => {
                    let target = self.lookup_var(f, target)?;
                    let (value, _) = self.expr(&sc, value, &mut vec![])?;
                    Statement::Assign { target, value }
                }
                StmtKind::Return(e) => Statement::Return(self.expr(&sc, e, &mut vec![])?.0),
                StmtKind::Break => Statement::Break,
                StmtKind::If { cond, then, els } => {
                    let condition = self.expr(&sc, cond, &mut vec![])?.0;
                    path.push(StmtStep::Then);
                    let then_branch = self.stmts(f, then, path)?;
                    path.pop();
                    path.push(StmtStep::Else);
                    let else_branch = match els {
                        Some(e) => self.stmts(f, e, path)?,
                        None => vec![],
                    };
                    path.pop();
                    Statement::If {
                        condition,
                        then_branch,
                        else_branch,
                    }
                }
                StmtKind::While { cond, body } => {
                    let condition = self.expr(&sc, cond, &mut vec![])?.0;
                    path.push(StmtStep::Body);
                    let body = self.stmts(f, body, path)?;
                    path.pop();
                    Statement::While { condition, body }
                }
                StmtKind::Repeat { count, body } => {
                    path.push(StmtStep::Body);
                    let body = self.stmts(f, body, path)?;
                    path.pop();
                    Statement::Repeat {
                        count: *count,
                        body,
                    }
                }
            };
            path.pop();
            out.push(st);
        }
        Ok(out)
    }

    fn lookup_var(&self, f: Ref<Function>, l: &Label) -> R<Ref<Var>> {
        match self.fn_vars[&f].get(&l.key()) {
            Some(v) => Ok(*v),
            None => err(l.span, format!("unknown variable `{}`", l.display())),
        }
    }

    fn record(&mut self, sc: &Scope, path: &[ExprStep], span: crate::span::Span) {
        let owner = match (sc.f, sc.c) {
            (Some(f), _) => ExprOwner::Stmt(f, sc.stmt.clone()),
            (None, Some(c)) => ExprOwner::Const(c),
            _ => return,
        };
        self.map
            .expressions
            .insert((owner, ExprPath(path.to_vec())), span);
    }

    fn child(
        &mut self,
        sc: &Scope,
        e: &Expr,
        path: &mut Vec<ExprStep>,
        step: ExprStep,
    ) -> R<Typed> {
        path.push(step);
        let r = self.expr(sc, e, path);
        path.pop();
        r
    }

    fn children(
        &mut self,
        sc: &Scope,
        es: &[Expr],
        path: &mut Vec<ExprStep>,
        step: fn(usize) -> ExprStep,
    ) -> R<(Vec<Expression>, Vec<Option<VariableType>>)> {
        let mut out = vec![];
        let mut tys = vec![];
        for (i, a) in es.iter().enumerate() {
            let (e, t) = self.child(sc, a, path, step(i))?;
            out.push(e);
            tys.push(t);
        }
        Ok((out, tys))
    }

    fn const_type(&mut self, c: Ref<Constant>, at: crate::span::Span) -> R<Option<VariableType>> {
        self.ensure_const(c, at)?;
        Ok(self.const_types[&c].clone())
    }

    pub(super) fn expr(&mut self, sc: &Scope, e: &Expr, path: &mut Vec<ExprStep>) -> R<Typed> {
        stack::grow(|| self.expr_inner(sc, e, path))
    }

    fn expr_inner(&mut self, sc: &Scope, e: &Expr, path: &mut Vec<ExprStep>) -> R<Typed> {
        self.record(sc, path, e.span);
        Ok(match &e.kind {
            ExprKind::Lit(l) => match &l.kind {
                LitKind::Bool(b) => (Expression::Bool(*b), Some(VariableType::Bool)),
                k => match numlit::convert(k, None) {
                    Ok(n) => (n.into_expression(), Some(n.kind().var_type())),
                    Err(m) => return err(l.span, m),
                },
            },
            ExprKind::Name(l) => {
                if let Some(f) = sc.f
                    && let Some(v) = self.fn_vars[&f].get(&l.key())
                {
                    let t = self.prog.functions[f].variables[*v].t.clone();
                    return Ok((Expression::Variable(*v), Some(t)));
                }
                let c = match self.consts.get(&l.key()) {
                    Some(c) => *c,
                    None => return err(l.span, format!("unknown name `{}`", l.display())),
                };
                (Expression::Constant(c), self.const_type(c, l.span)?)
            }
            ExprKind::Const(l) => {
                let c = self.lookup_const(l)?;
                (Expression::Constant(c), self.const_type(c, l.span)?)
            }
            ExprKind::Host(l) => {
                let h = self.lookup_host(l)?;
                let t = match &self.prog.host_inputs[h] {
                    HostInput::Static { t, .. } => Some(t.clone()),
                    HostInput::Dynamic => None,
                };
                (Expression::HostInput(h), t)
            }
            ExprKind::Table(l) => (
                Expression::PermutationTable(self.lookup_table(l)?),
                Some(VariableType::PermutationTable),
            ),
            ExprKind::Ambient(k) => (Expression::Ambient(*k), Some(ambient_type(*k))),
            ExprKind::Density { unit, input, index } => {
                let Some(f) = sc.f else {
                    return err(e.span, "not allowed in a constant expression");
                };
                let d = self.resolve_density(f, unit.as_ref(), input)?;
                let index = match index {
                    Some(i) => Some(Box::new(self.child(sc, i, path, ExprStep::DensityIndex)?.0)),
                    None => None,
                };
                let ret = self.density_return_type(d);
                (Expression::DensityVariable(d, index), Some(ret))
            }
            ExprKind::Call { callee, args } => {
                let function = self.lookup_fn(callee)?;
                let (parameters, _) = self.children(sc, args, path, ExprStep::Arg)?;
                let ret = self.prog.functions[function].return_type.clone();
                (
                    Expression::FunctionCall {
                        function,
                        parameters,
                    },
                    Some(ret),
                )
            }
            ExprKind::ExtCall { name, sig, args } => {
                let (parameter_types, return_type) = match sig {
                    Some((ps, r)) => (ps.iter().map(var_type).collect(), var_type(r)),
                    None => match self.env.extern_fn(name) {
                        Some(s) => (s.parameter_types, s.return_type),
                        None => {
                            return err(
                                e.span,
                                format!(
                                    "unknown extern `{name}`; write its signature: \
                                     `ext {name}::<(T, …) -> R>(…)`"
                                ),
                            );
                        }
                    },
                };
                let (parameters, _) = self.children(sc, args, path, ExprStep::Arg)?;
                (
                    Expression::ExternCall {
                        function_name: name.clone(),
                        parameters,
                        parameter_types,
                        return_type: return_type.clone(),
                    },
                    Some(return_type),
                )
            }
            ExprKind::Field {
                base,
                name,
                name_span,
                ty,
            } => {
                let (b, bt) = self.child(sc, base, path, ExprStep::Base)?;
                let explicit_base = ty.as_ref().and_then(|(_, b)| b.as_ref()).map(var_type);
                let Some(base_type) = explicit_base.or(bt) else {
                    return err(
                        *name_span,
                        format!(
                            "can't derive the type of the base of field `{name}`; \
                             write `.{name}::<T; B>`"
                        ),
                    );
                };
                let t = match ty {
                    Some((t, _)) => var_type(t),
                    None => {
                        let derived = field_type(&base_type, name, self.env);
                        match derived {
                            Some(t) => t,
                            None => {
                                return err(
                                    *name_span,
                                    format!(
                                        "can't derive the type of field `{name}`; write `.{name}::<T>`"
                                    ),
                                );
                            }
                        }
                    }
                };
                (
                    Expression::Field {
                        base: Box::new(b),
                        field: name.clone(),
                        type_of_field: t.clone(),
                        base_type,
                    },
                    Some(t),
                )
            }
            ExprKind::Index { base, index } => {
                let (a, at) = self.child(sc, base, path, ExprStep::Array)?;
                let (i, _) = self.child(sc, index, path, ExprStep::Index)?;
                (
                    Expression::ArrayAccess {
                        array: Box::new(a),
                        index: Box::new(i),
                    },
                    at.as_ref().and_then(index_type),
                )
            }
            ExprKind::Construct { ty, args } => {
                let (args, _) = self.children(sc, args, path, ExprStep::Arg)?;
                let t = var_type(ty);
                (Expression::Construct { t: t.clone(), args }, Some(t))
            }
            ExprKind::ConstructExtern { ty, fields } => {
                // `t` is normally an extern type; any other is the checker's business
                let t = var_type(ty);
                let mut args = vec![];
                for (i, (n, _, v)) in fields.iter().enumerate() {
                    let (v, _) = self.child(sc, v, path, ExprStep::FieldInit(i))?;
                    args.push((Self::intern_name(n), v));
                }
                (Expression::ConstructExtern { t: t.clone(), args }, Some(t))
            }
            ExprKind::Array(items) => {
                let (items, tys) = self.children(sc, items, path, ExprStep::Elem)?;
                let t = tys
                    .first()
                    .cloned()
                    .flatten()
                    .map(|t| VariableType::Array(Box::new(t), items.len()));
                (Expression::ArrayLiteral(items), t)
            }
            ExprKind::Cast(inner, ty) => {
                let (x, _) = self.child(sc, inner, path, ExprStep::Inner)?;
                let to = var_type(ty);
                (
                    Expression::ExplicitCast {
                        to: to.clone(),
                        expr: Box::new(x),
                    },
                    Some(to),
                )
            }
            ExprKind::Unary(op, inner) => {
                let (x, t) = self.child(sc, inner, path, ExprStep::Operand)?;
                (
                    Expression::UnaryOp {
                        op: *op,
                        operand: Box::new(x),
                    },
                    t,
                )
            }
            ExprKind::Binary(op, l, r) => {
                let (lx, lt) = self.child(sc, l, path, ExprStep::Left)?;
                let (rx, rt) = self.child(sc, r, path, ExprStep::Right)?;
                (
                    Expression::BinaryOp {
                        op: *op,
                        left: Box::new(lx),
                        right: Box::new(rx),
                    },
                    binary_type(*op, lt.as_ref(), rt.as_ref()),
                )
            }
        })
    }

    fn density_return_type(&self, d: Ref<DensityInput>) -> VariableType {
        for u in self.units.values() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = &self.prog.compute_units[u.r];
            if density_inputs.contains(d) {
                let target = density_inputs[d].density_function;
                let ComputeUnit::DensityFunction { function, .. } =
                    &self.prog.compute_units[target];
                return self.prog.functions[*function].return_type.clone();
            }
        }
        unreachable!("density input without a unit")
    }

    fn resolve_density(
        &self,
        f: Ref<Function>,
        unit: Option<&Label>,
        input: &Label,
    ) -> R<Ref<DensityInput>> {
        let u = match unit {
            Some(u) => self.lookup_unit(u)?,
            None => {
                let mut candidates = self
                    .unit_order
                    .iter()
                    .filter_map(|r| self.units.values().find(|u| u.r == *r))
                    .filter(|u| u.main == f || u.helpers.contains(&f));
                let first = candidates.next();
                match (first, candidates.next()) {
                    (Some(u), None) => u,
                    (None, _) => {
                        return err(
                            input.span,
                            "this function belongs to no unit; write `density(unit.input)`",
                        );
                    }
                    _ => {
                        return err(
                            input.span,
                            "this function belongs to several units; \
                             write `density(unit.input)`",
                        );
                    }
                }
            }
        };
        match u.inputs.get(&input.key()) {
            Some(d) => Ok(*d),
            None => err(
                input.span,
                format!("the unit has no input `{}`", input.display()),
            ),
        }
    }
}
