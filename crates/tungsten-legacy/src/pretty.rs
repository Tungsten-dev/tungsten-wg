use std::fmt::Write;

use crate::spmt::model::{
    BinaryOperator, DensityFunction,
    Expression::{self},
    Function, PermutationTableInput, SPMT, Statement, UnaryOperator, Var,
};

pub use tungsten_codegen::Printer;

pub trait PrettyPrint {
    fn pretty(&self, p: &mut Printer);
}

/* =========================
Top-level SPMT
========================= */
impl<'m> PrettyPrint for SPMT<'m> {
    fn pretty(&self, p: &mut Printer) {
        for f in &self.functions {
            f.pretty(p);
            p.line("");
        }

        for d in &self.density_functions {
            d.pretty(p);
            p.line("");
        }
    }
}

/* =========================
Functions
========================= */
impl<'m> PrettyPrint for Function<'m> {
    fn pretty(&self, p: &mut Printer) {
        p.push("fn ");
        if let Some(canonical_name) = self.canonical_name.as_deref() {
            p.push(canonical_name);
        } else {
            let name = p.anon_name(self, "function");

            p.push(&name);
        };
        p.push("(");

        for (i, param) in self.parameters.iter().enumerate() {
            if i > 0 {
                p.push(", ");
            }
            let param_name = param.concrete_name(p);
            write!(p.line, "{}: {:?}", param_name, param.t).unwrap();
        }

        p.line(") {");
        p.indent();

        for stmt in &self.body {
            stmt.pretty(p);
        }

        p.dedent();
        p.line("}");
    }
}

impl<'m> PrettyPrint for DensityFunction<'m> {
    fn pretty(&self, p: &mut Printer) {
        p.push("density ");

        if let Some(canonical_name) = self.canonical_name.as_deref() {
            p.push(canonical_name);
        } else {
            let name = p.anon_name(self, "density-function");
            p.push(&name);
        };

        p.line("(p: Vec3) {");
        p.indent();

        if !self.density_inputs.is_empty() {
            p.line("// density inputs:");
            for d in &self.density_inputs {
                let n = p.anon_name(d.density_function, "density-function");
                p.line(&format!("// - {}", n));
            }
        }

        if !self.permutation_table_inputs.is_empty() {
            p.line("// permutation table inputs:");
            for pt in &self.permutation_table_inputs {
                match pt {
                    PermutationTableInput::PerlinNoise {
                        ident,
                        subident,
                        subident_index,
                    } => match subident {
                        Some(sub) => {
                            p.line(&format!("// - {} {} [{}]", ident, sub, subident_index))
                        }
                        None => p.line(&format!("// - {}", ident)),
                    },
                    PermutationTableInput::Base3DNoise => {
                        p.line("// - base 3D noise");
                    }
                }
            }
        }

        if !self.density_inputs.is_empty() || !self.permutation_table_inputs.is_empty() {
            p.line("");
        }

        for stmt in &self.body {
            stmt.pretty(p);
        }

        p.dedent();
        p.line("}");
    }
}

/* =========================
Statements
========================= */
impl<'m> PrettyPrint for Statement<'m> {
    fn pretty(&self, p: &mut Printer) {
        match self {
            Statement::Assign { target, value } => {
                let target_name = target.concrete_name(p);
                p.push(&target_name);
                p.push(" = ");
                value.pretty(p);
                p.line(";");
            }

            Statement::Return(expr) => {
                p.push("return ");
                expr.pretty(p);
                p.line(";");
            }

            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                p.push("if ");
                condition.pretty(p);
                p.line(" {");
                p.indent();

                for s in then_branch {
                    s.pretty(p);
                }

                p.dedent();

                if !else_branch.is_empty() {
                    p.line("} else {");
                    p.indent();

                    for s in else_branch {
                        s.pretty(p);
                    }

                    p.dedent();
                }

                p.line("}");
            }

            Statement::While { condition, body } => {
                p.push("while ");
                condition.pretty(p);
                p.line(" {");
                p.indent();

                for s in body {
                    s.pretty(p);
                }

                p.dedent();
                p.line("}");
            }
            Statement::Repeat { count, body } => {
                p.push(&format!("repeat {}", count));
                p.line(" {");
                p.indent();

                for s in body {
                    s.pretty(p);
                }

                p.dedent();
                p.line("}");
            }
            Statement::Break => {
                p.line("break;");
            }
        }
    }
}

/* =========================
Expressions
========================= */
impl<'m> PrettyPrint for Expression<'m> {
    fn pretty(&self, p: &mut Printer) {
        match self {
            Expression::Variable(v) => {
                let name = v.concrete_name(p);
                p.push(&name);
            }
            Expression::Float(v) => {
                //write!(p.line, "{}", v).unwrap();
                let s = format!("{:.4}", v);
                let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
                p.push(&s);
            }
            Expression::Double(v) => {
                let s = format!("{:.4}", v);
                let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
                p.push(&s);
            }
            Expression::Byte(v) => {
                write!(p.line, "{}b", v).unwrap();
            }
            Expression::Int(v) => {
                write!(p.line, "{}", v).unwrap();
            }
            Expression::Long(v) => {
                write!(p.line, "{}", v).unwrap();
            }
            Expression::Bool(v) => {
                write!(p.line, "{}", v).unwrap();
            }
            Expression::FunctionCall {
                function,
                parameters,
            } => {
                if let Some(name) = function.canonical_name.as_deref() {
                    p.push(name);
                } else {
                    let name = p.anon_name(*function, "func");
                    p.push(&name);
                }
                p.push("(");

                for (i, param) in parameters.iter().enumerate() {
                    if i > 0 {
                        p.push(", ");
                    }
                    param.pretty(p);
                }

                p.push(")");
            }
            Expression::ExternCall {
                function_name,
                parameters,
                parameter_types: _,
                return_type: _,
            } => {
                p.push("extern ");
                p.push(function_name);
                p.push("(");
                for (i, param) in parameters.iter().enumerate() {
                    if i > 0 {
                        p.push(", ");
                    }
                    param.pretty(p);
                }
                p.push(")");
            }
            Expression::BinaryOp { op, left, right } => {
                p.push("(");
                left.pretty(p);
                p.push(match op {
                    BinaryOperator::Add => " + ",
                    BinaryOperator::Subtract => " - ",
                    BinaryOperator::Multiply => " * ",
                    BinaryOperator::Divide => " / ",
                    BinaryOperator::Equal => " == ",
                    BinaryOperator::NotEqual => " != ",
                    BinaryOperator::Less => " < ",
                    BinaryOperator::LessEqual => " <= ",
                    BinaryOperator::Greater => " > ",
                    BinaryOperator::GreaterEqual => " >= ",
                    BinaryOperator::And => " && ",
                    BinaryOperator::Or => " || ",
                    BinaryOperator::ShiftLeft => " << ",
                    BinaryOperator::ShiftRight => " >> ",
                    BinaryOperator::LogicalShiftRight => " >>> ",
                    BinaryOperator::BitOr => " | ",
                    BinaryOperator::BitAnd => " & ",
                    BinaryOperator::BitXor => " ^ ",
                });
                right.pretty(p);
                p.push(")");
            }
            Expression::DensityVariable(density_input, index) => {
                let name = p.anon_name(density_input.density_function, "density-function");
                p.push(&name);
                if let Some(index_expr) = index {
                    p.push("[");
                    index_expr.pretty(p);
                    p.push("]");
                }
            }
            Expression::PermutationTable(perm_table_input) => {
                let name = match &perm_table_input {
                    PermutationTableInput::PerlinNoise {
                        ident,
                        subident,
                        subident_index,
                    } => match subident {
                        Some(sub) => format!("perm-table-{}_{}[{}]", ident, sub, subident_index),
                        None => format!("perm-table-{}", ident),
                    },
                    PermutationTableInput::Base3DNoise => "base-3d-noise".to_string(),
                };

                p.push(&name);
            }
            Expression::UnaryOp { op, operand } => {
                p.push(match op {
                    UnaryOperator::Negate => "-",
                    UnaryOperator::Not => "!",
                    UnaryOperator::Ref => "&",
                    UnaryOperator::MutRef => "&mut ",
                });
                operand.pretty(p);
            }
            Expression::Field {
                base,
                field,
                type_of_field: _,
                known_idnex: _,
            } => {
                base.pretty(p);
                p.push(".");
                p.push(field);
            }
            Expression::Construct { t, args } => {
                p.push(&format!("{:?}(", t));
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        p.push(", ");
                    }
                    arg.pretty(p);
                }
                p.push(")");
            }
            Expression::ArrayAccess { array, index } => {
                array.pretty(p);
                p.push("[");
                index.pretty(p);
                p.push("]");
            }
            Expression::ConstructExtern { t, args } => {
                p.push(&format!("extern {:?}(", t));
                for (i, (arg_name, arg_expr)) in args.iter().enumerate() {
                    if i > 0 {
                        p.push(", ");
                    }
                    p.push(arg_name);
                    p.push(": ");
                    arg_expr.pretty(p);
                }
                p.push(")");
            }
            Expression::ArrayLiteral(expressions) => {
                p.push("[");
                for (i, expr) in expressions.iter().enumerate() {
                    if i > 0 {
                        p.push(", ");
                    }
                    expr.pretty(p);
                }
                p.push("]");
            }
            Expression::ExplicitCast { to, expr } => {
                p.push(&format!("({:?})", to));
                expr.pretty(p);
            }
        }
    }
}

impl<'m> DensityFunction<'m> {
    pub fn pretty_with_deps(&self, p: &mut Printer) {
        // Print helper function dependencies first
        if !self.helper_functions.is_empty() {
            p.line("// Helper function dependencies:");
            for func_ref in &self.helper_functions {
                func_ref.pretty(p);
                p.line(""); // Add spacing between helper functions
            }
        }

        // Print the density function itself
        self.pretty(p);
    }
}
pub trait ConcreteName {
    fn concrete_name(&self, p: &mut Printer) -> String;
}

impl ConcreteName for Var<'_> {
    fn concrete_name(&self, p: &mut Printer) -> String {
        match &self.clone().name {
            super::model::Name::Anonymous => p.anon_name(*self, "var"),
            super::model::Name::Prefixed(pre) => p.anon_name(*self, pre),
            super::model::Name::Named(n) => n.clone(),
        }
    }
}
