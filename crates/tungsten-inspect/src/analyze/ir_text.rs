//! Per-function IR text (SPEC.md §7.5). Deterministic and free of ref indices, so `diff` can
//! compare the text of a function across two programs.
//!
//! `render/ir_text.rs` assembles `ir/program.txt` from these strings and the rest of the model.

use tungsten_ir::model::{ConstExpr, Expression, Function, Ref, Statement, Var};

use super::Ctx;
use super::dataflow::{binary_symbol, unary_symbol};

const INDENT: &str = "    ";

/// `fn name(params) -> T { … }`; main functions are printed without a name (their unit block
/// names them).
pub(crate) fn function(ctx: &Ctx, f: Ref<Function>) -> String {
    let func = &ctx.program.functions[f];
    let params: Vec<String> = func
        .parameters
        .iter()
        .map(|p| format!("{}: {:?}", var(ctx, f, *p), func.variables[*p].t))
        .collect();
    let name = if ctx.main_of.contains_key(&f) {
        " ".to_string()
    } else {
        format!(" {}", ctx.functions[&f].display)
    };
    let mut out = format!(
        "fn{name}({}) -> {:?} {{\n",
        params.join(", "),
        func.return_type
    );
    let p = Printer { ctx, f };
    p.block(&func.body, 1, &mut out);
    out.push_str("}\n");
    out
}

/// A constant's value expression.
pub(crate) fn const_expr(ctx: &Ctx, e: &ConstExpr) -> String {
    match e {
        ConstExpr::Constant(c) => ctx.constants[c].display.clone(),
        ConstExpr::Float(x) => format!("{x:?}f"),
        ConstExpr::Double(x) => format!("{x:?}"),
        ConstExpr::Byte(x) => format!("{x}b"),
        ConstExpr::Int(x) => format!("{x}"),
        ConstExpr::Long(x) => format!("{x}L"),
        ConstExpr::Bool(x) => format!("{x}"),
        ConstExpr::BinaryOp { op, left, right } => format!(
            "({} {} {})",
            const_expr(ctx, left),
            binary_symbol(*op),
            const_expr(ctx, right)
        ),
        ConstExpr::UnaryOp { op, operand } => unary(unary_symbol(*op), const_expr(ctx, operand)),
        ConstExpr::ExplicitCast { to, expr } => format!("({} as {to:?})", const_expr(ctx, expr)),
        ConstExpr::Field { base, field, .. } => format!("{}.{field}", const_expr(ctx, base)),
        ConstExpr::ArrayAccess { array, index } => {
            format!("{}[{}]", const_expr(ctx, array), const_expr(ctx, index))
        }
        ConstExpr::ArrayLiteral(items) => format!("[{}]", list(items, |i| const_expr(ctx, i))),
        ConstExpr::Construct { t, args } => {
            format!("{t:?}({})", list(args, |a| const_expr(ctx, a)))
        }
    }
}

fn unary(symbol: &str, operand: String) -> String {
    if symbol.len() > 1 {
        format!("{symbol} {operand}")
    } else {
        format!("{symbol}{operand}")
    }
}

fn list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    items.iter().map(f).collect::<Vec<_>>().join(", ")
}

fn var(ctx: &Ctx, f: Ref<Function>, v: Ref<Var>) -> String {
    ctx.var_names[&f]
        .get(&v)
        .cloned()
        .unwrap_or_else(|| "<foreign var>".to_string())
}

struct Printer<'c, 'a> {
    ctx: &'c Ctx<'a>,
    f: Ref<Function>,
}

impl Printer<'_, '_> {
    fn line(&self, depth: usize, s: &str, out: &mut String) {
        for _ in 0..depth {
            out.push_str(INDENT);
        }
        out.push_str(s);
        out.push('\n');
    }

    fn block(&self, stmts: &[Statement], depth: usize, out: &mut String) {
        for s in stmts {
            self.stmt(s, depth, out);
        }
    }

    fn stmt(&self, s: &Statement, depth: usize, out: &mut String) {
        match s {
            Statement::Assign { target, value } => {
                let line = format!("{} = {}", var(self.ctx, self.f, *target), self.expr(value));
                self.line(depth, &line, out);
            }
            Statement::Return(e) => self.line(depth, &format!("return {}", self.expr(e)), out),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.line(depth, &format!("if {} {{", self.expr(condition)), out);
                self.block(then_branch, depth + 1, out);
                if !else_branch.is_empty() {
                    self.line(depth, "} else {", out);
                    self.block(else_branch, depth + 1, out);
                }
                self.line(depth, "}", out);
            }
            Statement::While { condition, body } => {
                self.line(depth, &format!("while {} {{", self.expr(condition)), out);
                self.block(body, depth + 1, out);
                self.line(depth, "}", out);
            }
            Statement::Repeat { count, body } => {
                self.line(depth, &format!("repeat {count} {{"), out);
                self.block(body, depth + 1, out);
                self.line(depth, "}", out);
            }
            Statement::Break => self.line(depth, "break", out),
        }
    }

    fn exprs(&self, items: &[Expression]) -> String {
        list(items, |e| self.expr(e))
    }

    fn expr(&self, e: &Expression) -> String {
        let ctx = self.ctx;
        match e {
            Expression::Constant(c) => ctx.constants[c].display.clone(),
            Expression::Variable(v) => var(ctx, self.f, *v),
            Expression::Float(x) => format!("{x:?}f"),
            Expression::Double(x) => format!("{x:?}"),
            Expression::Byte(x) => format!("{x}b"),
            Expression::Int(x) => format!("{x}"),
            Expression::Long(x) => format!("{x}L"),
            Expression::Bool(x) => format!("{x}"),
            Expression::FunctionCall {
                function,
                parameters,
            } => format!(
                "{}({})",
                ctx.functions[function].display,
                self.exprs(parameters)
            ),
            Expression::ExternCall {
                function_name,
                parameters,
                ..
            } => format!("extern {function_name}({})", self.exprs(parameters)),
            Expression::DensityVariable(r, idx) => {
                let input = &ctx.inputs[r];
                // Helpers may read inputs of a unit other than the one they are printed under.
                let name = if ctx.owners(self.f).contains(&input.unit) {
                    input.name.clone()
                } else {
                    format!("{}::{}", ctx.units[&input.unit].display, input.name)
                };
                match idx {
                    None => name,
                    Some(i) => format!("{name}[{}]", self.expr(i)),
                }
            }
            Expression::PermutationTable(t) => format!("table {}", ctx.tables[t].display),
            Expression::HostInput(h) => format!("host {}", ctx.host_inputs[h].display),
            Expression::BinaryOp { op, left, right } => format!(
                "({} {} {})",
                self.expr(left),
                binary_symbol(*op),
                self.expr(right)
            ),
            Expression::UnaryOp { op, operand } => unary(unary_symbol(*op), self.expr(operand)),
            Expression::Field { base, field, .. } => format!("{}.{field}", self.expr(base)),
            Expression::ArrayAccess { array, index } => {
                format!("{}[{}]", self.expr(array), self.expr(index))
            }
            Expression::Construct { t, args } => format!("{t:?}({})", self.exprs(args)),
            Expression::ConstructExtern { t, args } => format!(
                "{t:?}{{ {} }}",
                list(args, |(n, a)| format!("{n}: {}", self.expr(a)))
            ),
            Expression::ArrayLiteral(items) => format!("[{}]", self.exprs(items)),
            Expression::ExplicitCast { to, expr } => format!("({} as {to:?})", self.expr(expr)),
        }
    }
}
