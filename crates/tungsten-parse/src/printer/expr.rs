//! Expressions and types, fully parenthesized.

use tungsten_ir::*;

use super::derive::Deriver;
use super::float::quote_name;
use super::labels::Labels;
use crate::stack;
use crate::tables::{self, Num};

/// A type's canonical spelling.
pub fn type_text(t: &VariableType) -> String {
    stack::grow(|| match t {
        VariableType::Array(e, n) => format!("[{}; {n}]", type_text(e)),
        VariableType::Extern(n) => format!("extern {}", quote_name(n)),
        t => tables::prim_kw(t)
            .expect("every other type has a keyword")
            .as_str()
            .to_string(),
    })
}

/// `ConstExpr` shares the printer through its `Expression` twin.
pub(super) fn const_to_expression(c: &ConstExpr) -> Expression {
    stack::grow(|| const_to_expression_inner(c))
}

fn const_to_expression_inner(c: &ConstExpr) -> Expression {
    let b = |c: &ConstExpr| Box::new(const_to_expression(c));
    match c {
        ConstExpr::Constant(r) => Expression::Constant(*r),
        ConstExpr::Float(v) => Expression::Float(*v),
        ConstExpr::Double(v) => Expression::Double(*v),
        ConstExpr::Byte(v) => Expression::Byte(*v),
        ConstExpr::Int(v) => Expression::Int(*v),
        ConstExpr::Long(v) => Expression::Long(*v),
        ConstExpr::Bool(v) => Expression::Bool(*v),
        ConstExpr::BinaryOp { op, left, right } => Expression::BinaryOp {
            op: *op,
            left: b(left),
            right: b(right),
        },
        ConstExpr::UnaryOp { op, operand } => Expression::UnaryOp {
            op: *op,
            operand: b(operand),
        },
        ConstExpr::ExplicitCast { to, expr } => Expression::ExplicitCast {
            to: to.clone(),
            expr: b(expr),
        },
        ConstExpr::Field {
            base,
            field,
            type_of_field,
            base_type,
        } => Expression::Field {
            base: b(base),
            field: field.clone(),
            type_of_field: type_of_field.clone(),
            base_type: base_type.clone(),
        },
        ConstExpr::ArrayAccess { array, index } => Expression::ArrayAccess {
            array: b(array),
            index: b(index),
        },
        ConstExpr::ArrayLiteral(items) => {
            Expression::ArrayLiteral(items.iter().map(const_to_expression).collect())
        }
        ConstExpr::Construct { t, args } => Expression::Construct {
            t: t.clone(),
            args: args.iter().map(const_to_expression).collect(),
        },
    }
}

pub(super) struct ExprPrinter<'a> {
    pub labels: &'a Labels,
    /// Decides whether a field's base type has to be written.
    pub derive: &'a Deriver<'a>,
    /// The function the expression is in, for variable labels. `None` in constants.
    pub f: Option<Ref<Function>>,
}

impl ExprPrinter<'_> {
    pub fn to_string(&self, e: &Expression) -> String {
        let mut s = String::new();
        self.expr(e, &mut s);
        s
    }

    fn list(&self, items: &[Expression], out: &mut String) {
        for (i, a) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            self.expr(a, out);
        }
    }

    /// An operand of `.f` or `[i]`: anything that would bind looser than a postfix is wrapped.
    fn postfix_base(&self, e: &Expression, out: &mut String) {
        use Expression::*;
        if matches!(
            e,
            UnaryOp { .. } | Float(_) | Double(_) | Byte(_) | Int(_) | Long(_) | Bool(_)
        ) {
            out.push('(');
            self.expr(e, out);
            out.push(')');
        } else {
            self.expr(e, out);
        }
    }

    pub fn expr(&self, e: &Expression, out: &mut String) {
        stack::grow(|| self.expr_inner(e, out))
    }

    fn expr_inner(&self, e: &Expression, out: &mut String) {
        use Expression::*;
        if let Some(n) = Num::of_expression(e) {
            out.push_str(&n.spell());
            return;
        }
        match e {
            Constant(c) => out.push_str(&format!("const({})", self.labels.constant(*c))),
            Variable(v) => {
                let f = self.f.expect("a variable outside any function");
                out.push_str(self.labels.var(f, *v));
            }
            Float(_) | Double(_) | Byte(_) | Int(_) | Long(_) => unreachable!(),
            Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            FunctionCall {
                function,
                parameters,
            } => {
                out.push_str(self.labels.function(*function));
                out.push('(');
                self.list(parameters, out);
                out.push(')');
            }
            ExternCall {
                function_name,
                parameters,
                parameter_types,
                return_type,
            } => {
                out.push_str(&format!("ext {}::<(", quote_name(function_name)));
                let ps: Vec<String> = parameter_types.iter().map(type_text).collect();
                out.push_str(&ps.join(", "));
                out.push_str(&format!(") -> {}>(", type_text(return_type)));
                self.list(parameters, out);
                out.push(')');
            }
            DensityVariable(d, idx) => {
                let u = self.labels.input_unit(*d);
                out.push_str(&format!(
                    "density({}.{}",
                    self.labels.unit(u),
                    self.labels.input(*d)
                ));
                if let Some(i) = idx {
                    out.push_str(", ");
                    self.expr(i, out);
                }
                out.push(')');
            }
            PermutationTable(t) => out.push_str(&format!("table({})", self.labels.table(*t))),
            HostInput(h) => out.push_str(&format!("host({})", self.labels.host(*h))),
            Ambient(k) => out.push_str(&format!("ambient({})", tables::ambient_text(*k))),
            BinaryOp { op, left, right } => {
                out.push('(');
                self.expr(left, out);
                out.push_str(&format!(" {} ", tables::op_text(*op)));
                self.expr(right, out);
                out.push(')');
            }
            UnaryOp { op, operand } => {
                out.push_str(tables::unary_text(*op));
                out.push('(');
                self.expr(operand, out);
                out.push(')');
            }
            Field {
                base,
                field,
                type_of_field,
                base_type,
            } => {
                self.postfix_base(base, out);
                out.push_str(&format!(
                    ".{}::<{}",
                    quote_name(field),
                    type_text(type_of_field)
                ));
                if self.derive.expr(self.f, base).as_ref() != Some(base_type) {
                    out.push_str(&format!("; {}", type_text(base_type)));
                }
                out.push('>');
            }
            ArrayAccess { array, index } => {
                self.postfix_base(array, out);
                out.push('[');
                self.expr(index, out);
                out.push(']');
            }
            Construct { t, args } => {
                match t {
                    VariableType::Vec3 => out.push_str("vec3("),
                    VariableType::Pos3 => out.push_str("pos3("),
                    t => out.push_str(&format!("new {}(", type_text(t))),
                }
                self.list(args, out);
                out.push(')');
            }
            ConstructExtern { t, args } => {
                out.push_str(&format!("new {} {{", type_text(t)));
                for (i, (name, v)) in args.iter().enumerate() {
                    out.push_str(if i == 0 { " " } else { ", " });
                    out.push_str(&format!("{}: ", quote_name(name)));
                    self.expr(v, out);
                }
                out.push_str(if args.is_empty() { "}" } else { " }" });
            }
            ArrayLiteral(items) => {
                out.push('[');
                self.list(items, out);
                out.push(']');
            }
            ExplicitCast { to, expr } => {
                out.push('(');
                self.expr(expr, out);
                out.push_str(&format!(" as {})", type_text(to)));
            }
        }
    }
}
