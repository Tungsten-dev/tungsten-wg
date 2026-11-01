/*
Rust code generation from ICL.
This module generates Rust library code from the intermediate CPU language representation.
*/

use super::model as icl;
use tungsten_codegen::Printer;

/// Generate Rust code for ICL structures and functions
pub struct RustCodeGenerator;

impl RustCodeGenerator {
    pub fn new() -> Self {
        RustCodeGenerator
    }

    /// Generate a complete Rust module from RCL
    pub fn generate_module(&self, icl: &icl::RCL<'_>) -> String {
        let mut p = Printer::new();

        // Add common imports
        p.line("// Auto-generated Rust module from RCL");
        // ignore all warnings in the generated code
        p.line("#[allow(dead_code)]");
        p.line("#[allow(unused_variables)]");
        p.line("#[allow(unused_imports)]");
        p.line("#[allow(warnings)]");
        p.line("#[allow(clippy::all)]");
        p.line("#[allow(unne)]");

        p.line("");

        p.line("use crate::mathf64::*;");
        p.line("use crate::utilsf64::*;");

        for import in &icl.import_statements {
            p.push("use ");
            p.push(import);
            p.line(";");
        }

        p.line("");

        // Generate struct definitions
        for struct_def in &icl.structs {
            self.generate_struct(&mut p, struct_def);
            p.line("");
        }

        // deduplicate constants by name
        // they are guaranteed to have unique names
        let mut seen_constants = std::collections::HashSet::new();
        // Generate constant definitions
        for constant in &icl.constants {
            let var_name = self.variable_to_string(&constant.var, &mut p);
            if seen_constants.contains(&var_name) {
                continue;
            }
            seen_constants.insert(var_name.clone());
            print!("Generating constant: {}", var_name);

            p.push("const ");
            p.push(&var_name);
            p.push(": ");
            p.push(&self.type_to_rust_string(&constant.var.t));
            p.push(" = ");
            let value_str = self.expression_to_string_with_printer(&mut p, &constant.value);
            p.push(&value_str);
            p.line(";");
        }

        // Generate functions
        for func in &icl.functions {
            self.generate_function(&mut p, func, false);
            p.line("");
        }

        // Generate main functions
        for func in &icl.main_functions {
            self.generate_function(&mut p, func, true);
            p.line("");
        }

        for imp in &icl.impls {
            self.generate_impl(&mut p, imp);
            p.line("");
        }

        p.finish()
    }

    pub fn generate_inline_module(&self, icl: &icl::RCL<'_>, module_name: &str) -> String {
        let mut p = Printer::new();

        p.line(&format!("pub mod {} {{", module_name));
        p.indent();
        p.line("// Auto-generated inline Rust module from RCL");
        p.line("#[allow(dead_code)]");
        p.line("#[allow(unused_variables)]");
        p.line("#[allow(unused_imports)]");
        p.line("#[allow(warnings)]");
        p.line("#[allow(clippy::all)]");

        p.line("");

        p.line("use super::*;");

        for import in &icl.import_statements {
            p.push("use ");
            p.push(import);
            p.line(";");
        }

        p.line("");

        // Generate struct definitions
        for struct_def in &icl.structs {
            self.generate_struct(&mut p, struct_def);
            p.line("");
        }

        // Generate constant definitions
        let mut seen_constants = std::collections::HashSet::new();
        for constant in &icl.constants {
            let var_name = self.variable_to_string(&constant.var, &mut p);
            if seen_constants.contains(&var_name) {
                continue;
            }
            seen_constants.insert(var_name.clone());

            p.push("const ");
            p.push(&var_name);
            p.push(": ");
            p.push(&self.type_to_rust_string(&constant.var.t));
            p.push(" = ");
            let value_str = self.expression_to_string_with_printer(&mut p, &constant.value);
            p.push(&value_str);
            p.line(";");
        }

        // Generate functions (mark all as inline)
        for func in &icl.functions {
            self.generate_function(&mut p, func, false);
            p.line("");
        }

        // Generate main functions (mark all as inline)
        for func in &icl.main_functions {
            self.generate_function(&mut p, func, true);
            p.line("");
        }

        for imp in &icl.impls {
            self.generate_impl(&mut p, imp);
            p.line("");
        }

        p.dedent();
        p.line("}");

        p.finish()
    }

    /// Generate Rust code for a struct definition
    fn generate_struct(&self, p: &mut Printer, struct_def: &icl::StructRef<'_>) {
        if struct_def.is_repr_c {
            p.line("#[repr(C)]");
        }
        if struct_def.derivations.len() > 0 {
            p.line(&format!("#[derive({})]", struct_def.derivations.join(", ")));
        }

        p.push("pub struct ");
        p.push(&struct_def.name);
        p.line(" {");

        p.indent();
        for (field_name, field_type) in &struct_def.fields {
            p.push("pub ");
            p.push(field_name);
            p.push(": ");
            p.push(&self.type_to_rust_string(field_type));
            p.line(",");
        }
        p.dedent();

        p.line("}");
    }

    /// Generate Rust code for an impl block
    fn generate_impl(&self, p: &mut Printer, imp: &icl::Impl<'_>) {
        p.push("impl ");
        p.push(&imp.type_name);
        p.line(" {");
        p.indent();
        for (i, method) in imp.methods.iter().enumerate() {
            if i > 0 {
                p.line("");
            }
            self.generate_function(p, method, true);
        }
        p.dedent();
        p.line("}");
    }

    /// Generate Rust code for a function
    fn generate_function(&self, p: &mut Printer, func: &icl::FunctionRef<'_>, public: bool) {
        // Function signature
        if func.inline {
            p.line("#[inline]");
        }

        p.push(if public { "pub fn " } else { "fn " });

        // Handle optional function name
        if let Some(name) = &func.name {
            p.push(name);
        } else {
            let anon_name = sanitize_anon_name(&p.anon_name(*func, "func"));
            p.push(&anon_name);
        }

        p.push("(");

        if let Some(receiver) = func.receiver {
            p.push(match receiver {
                icl::Receiver::Value => "self",
                icl::Receiver::Ref => "&self",
                icl::Receiver::MutRef => "&mut self",
            });
        }

        for (i, param) in func.parameters.iter().enumerate() {
            if i > 0 || func.receiver.is_some() {
                p.push(", ");
            }
            p.push(&param.name);
            p.push(": ");
            p.push(&self.type_to_rust_string(&param.t));
        }

        if let Some(return_type) = &func.return_type {
            p.push(") -> ");
            p.push(&self.type_to_rust_string(return_type));
        } else {
            p.push(")");
        }
        p.line(" {");

        p.indent();

        // declare variables for all used variables
        for v in &func.variables {
            p.push("let mut ");
            let var_name = self.variable_to_string(v, p);
            p.push(&var_name);
            p.push(": ");
            p.push(&self.type_to_rust_string(&v.t));
            p.line(";");
        }

        // Function body
        for stmt in &func.body {
            self.generate_statement(p, stmt);
        }

        p.dedent();
        p.line("}");
    }

    /// Generate Rust code for a statement
    fn generate_statement(&self, p: &mut Printer, stmt: &icl::Statement<'_>) {
        match stmt {
            icl::Statement::Assign { target, value } => {
                let var_name = self.variable_to_string(target, p);
                p.push(&var_name);
                p.push(" = ");
                let value_str = self.expression_to_string_with_printer(p, value);
                p.push(&value_str);
                p.line(";");
            }
            icl::Statement::Return(Some(expr)) => {
                p.push("return ");
                let expr_str = self.expression_to_string_with_printer(p, expr);
                p.push(&expr_str);
                p.line(";");
            }
            icl::Statement::Return(None) => {
                p.line("return;");
            }
            icl::Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                p.push("if ");
                let cond_str = self.expression_to_string_with_printer(p, condition);
                p.push(&cond_str);
                p.line(" {");

                p.indent();
                for stmt in then_branch {
                    self.generate_statement(p, stmt);
                }
                p.dedent();

                if let Some(else_stmts) = else_branch {
                    p.line("} else {");
                    p.indent();
                    for stmt in else_stmts {
                        self.generate_statement(p, stmt);
                    }
                    p.dedent();
                    p.line("}");
                } else {
                    p.line("}");
                }
            }
            icl::Statement::While { condition, body } => {
                p.push("while ");
                let cond_str = self.expression_to_string_with_printer(p, condition);
                p.push(&cond_str);
                p.line(" {");

                p.indent();
                for stmt in body {
                    self.generate_statement(p, stmt);
                }
                p.dedent();

                p.line("}");
            }
            icl::Statement::Declare {
                variable,
                init,
                mutable,
            } => {
                p.push("let ");
                if *mutable {
                    p.push("mut ");
                }
                let var_name = self.variable_to_string(variable, p);
                p.push(&var_name);
                p.push(": ");
                p.push(&self.type_to_rust_string(&variable.t));

                if let Some(expr) = init {
                    p.push(" = ");
                    let expr_str = self.expression_to_string_with_printer(p, expr);
                    p.push(&expr_str);
                }

                p.line(";");
            }
            icl::Statement::FunctionCall {
                function,
                arguments,
            } => {
                // Handle optional function name
                if let Some(name) = &function.name {
                    p.push(name);
                } else {
                    let anon_name = sanitize_anon_name(&p.anon_name(*function, "func"));
                    p.push(&anon_name);
                }

                p.push("(");

                for (i, arg) in arguments.iter().enumerate() {
                    if i > 0 {
                        p.push(", ");
                    }
                    let arg_str = self.expression_to_string_with_printer(p, arg);
                    p.push(&arg_str);
                }

                p.line(");");
            }
            icl::Statement::Block(stmts) => {
                p.line("{");
                p.indent();
                for stmt in stmts {
                    self.generate_statement(p, stmt);
                }
                p.dedent();
                p.line("}");
            }
            icl::Statement::For {
                init,
                condition,
                increment,
                body,
            } => {
                p.push("for ");
                if let Some(init_stmt) = init {
                    // For loop with initialization
                    p.push("{ ");
                    self.generate_statement(p, init_stmt);
                    p.push(" }");
                } else {
                    p.push("; ");
                }

                if let Some(cond_expr) = condition {
                    let cond_str = self.expression_to_string_with_printer(p, cond_expr);
                    p.push(&cond_str);
                }
                p.push("; ");

                if let Some(incr_stmt) = increment {
                    p.push("{ ");
                    self.generate_statement(p, incr_stmt);
                    p.push(" }");
                }

                p.line(" {");

                p.indent();
                for stmt in body {
                    self.generate_statement(p, stmt);
                }
                p.dedent();

                p.line("}");
            }
            icl::Statement::ForIn {
                variable,
                iterable,
                body,
            } => {
                p.push("for ");
                let var_name = self.variable_to_string(variable, p);
                p.push(&var_name);
                p.push(" in ");
                let iterable_str = self.expression_to_string_with_printer(p, iterable);
                p.push(&iterable_str);
                p.line(" {");

                p.indent();
                for stmt in body {
                    self.generate_statement(p, stmt);
                }
                p.dedent();

                p.line("}");
            }
            icl::Statement::ArrayAssign {
                target,
                index,
                value,
            } => {
                let var_name = self.variable_to_string(target, p);
                p.push(&var_name);
                p.push("[");
                let index_str = self.expression_to_string_with_printer(p, index);
                p.push(&index_str);
                p.push("] = ");
                let value_str = self.expression_to_string_with_printer(p, value);
                p.push(&value_str);
                p.line(";");
            }

            icl::Statement::AssignPlace { target, value } => {
                let target_str = self.expression_to_string_with_printer(p, target);
                p.push(&target_str);
                p.push(" = ");
                let value_str = self.expression_to_string_with_printer(p, value);
                p.push(&value_str);
                p.line(";");
            }

            icl::Statement::InlineRust(s) => {
                p.line(s);
            }
            icl::Statement::Break => p.line("break;"),
            icl::Statement::Continue => p.line("continue;"),
        }
    }

    /// Convert expression to Rust code string
    fn expression_to_string_with_printer(
        &self,
        p: &mut Printer,
        expr: &icl::Expression<'_>,
    ) -> String {
        match expr {
            icl::Expression::Variable(var) => self.variable_to_string(var, p),
            icl::Expression::I8Literal(val) => format!("{}_i8", val),
            icl::Expression::I32Literal(val) => format!("{}_i32", val),
            icl::Expression::I64Literal(val) => format!("{}_i64", val),
            icl::Expression::F32Literal(val) => self.format_f32_literal(*val),
            icl::Expression::F64Literal(val) => self.format_f64_literal(*val),
            icl::Expression::BoolLiteral(val) => format!("{}", val),
            icl::Expression::BinaryOp { op, left, right } => {
                format!(
                    "({} {} {})",
                    self.expression_to_string_with_printer(p, left),
                    self.binary_op_to_string(*op),
                    self.expression_to_string_with_printer(p, right)
                )
            }
            icl::Expression::UnaryOp { op, operand } => {
                format!(
                    "({}{})",
                    self.unary_op_to_string(*op),
                    self.expression_to_string_with_printer(p, operand)
                )
            }
            icl::Expression::FunctionCall {
                function,
                arguments,
            } => {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|a| self.expression_to_string_with_printer(p, a))
                    .collect();
                let func_name = if let Some(name) = &function.name {
                    name.clone()
                } else {
                    sanitize_anon_name(&p.anon_name(*function, "func"))
                };
                format!("{}({})", func_name, args.join(", "))
            }
            icl::Expression::LateBoundCall {
                function_name,
                arguments,
                ..
            } => {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|a| self.expression_to_string_with_printer(p, a))
                    .collect();
                format!("{}({})", function_name, args.join(", "))
            }
            icl::Expression::Field { base, field } => {
                format!(
                    "{}.{}",
                    self.expression_to_string_with_printer(p, base),
                    field
                )
            }
            icl::Expression::Index { base, index } => {
                format!(
                    "{}[{} as usize]",
                    self.expression_to_string_with_printer(p, base),
                    self.expression_to_string_with_printer(p, index)
                )
            }
            icl::Expression::Deref(expr) => {
                format!("(*{})", self.expression_to_string_with_printer(p, expr))
            }
            icl::Expression::Ref(expr) => {
                format!("(&{})", self.expression_to_string_with_printer(p, expr))
            }
            icl::Expression::MutRef(expr) => {
                format!("(&mut {})", self.expression_to_string_with_printer(p, expr))
            }
            icl::Expression::Cast { expr, to_type } => {
                format!(
                    "({} as {})",
                    self.expression_to_string_with_printer(p, expr),
                    self.type_to_rust_string(to_type)
                )
            }
            icl::Expression::StructInit {
                struct_name,
                fields,
            } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(name, expr)| {
                        format!(
                            "{}: {}",
                            name,
                            self.expression_to_string_with_printer(p, expr)
                        )
                    })
                    .collect();
                format!("{} {{ {} }}", struct_name, field_strs.join(", "))
            }
            icl::Expression::ArrayInit {
                element_type: _,
                element,
                count,
            } => {
                format!(
                    "[{}; {}]",
                    self.expression_to_string_with_printer(p, element),
                    count
                )
            }
            icl::Expression::TupleInit(expressions) => {
                let expr_strs: Vec<String> = expressions
                    .iter()
                    .map(|e| self.expression_to_string_with_printer(p, e))
                    .collect();
                format!("({})", expr_strs.join(", "))
            }
            icl::Expression::InlineRust(s) => s.clone(),
            icl::Expression::ArrayLiteral(expressions) => {
                let expr_strs: Vec<String> = expressions
                    .iter()
                    .map(|e| self.expression_to_string_with_printer(p, e))
                    .collect();
                format!("[{}]", expr_strs.join(", "))
            }
            icl::Expression::Construct { t, args } => {
                let arg_strs: Vec<String> = args
                    .iter()
                    .map(|(name, expr)| {
                        format!(
                            "{}: {}",
                            name,
                            self.expression_to_string_with_printer(p, expr)
                        )
                    })
                    .collect();
                format!(
                    "{} {{ {} }}",
                    self.type_to_rust_string(t),
                    arg_strs.join(", ")
                )
            }
        }
    }

    /// Convert variable to string
    fn variable_to_string(&self, var: &std::rc::Rc<icl::Variable>, p: &mut Printer) -> String {
        if let Some(name) = &var.name {
            name.clone()
        } else {
            sanitize_anon_name(&p.anon_name(var.clone(), "var"))
        }
    }

    /// Convert binary operator to Rust string
    fn binary_op_to_string(&self, op: icl::BinaryOperator) -> &'static str {
        match op {
            icl::BinaryOperator::Add => "+",
            icl::BinaryOperator::Subtract => "-",
            icl::BinaryOperator::Multiply => "*",
            icl::BinaryOperator::Divide => "/",
            icl::BinaryOperator::Modulo => "%",
            icl::BinaryOperator::Equal => "==",
            icl::BinaryOperator::NotEqual => "!=",
            icl::BinaryOperator::Less => "<",
            icl::BinaryOperator::LessEqual => "<=",
            icl::BinaryOperator::Greater => ">",
            icl::BinaryOperator::GreaterEqual => ">=",
            icl::BinaryOperator::And => "&&",
            icl::BinaryOperator::Or => "||",
            icl::BinaryOperator::BitwiseAnd => "&",
            icl::BinaryOperator::BitwiseOr => "|",
            icl::BinaryOperator::BitwiseXor => "^",
            icl::BinaryOperator::LeftShift => "<<",
            icl::BinaryOperator::RightShift => ">>",
        }
    }

    /// Convert unary operator to Rust string
    fn unary_op_to_string(&self, op: icl::UnaryOperator) -> &'static str {
        match op {
            icl::UnaryOperator::Negate => "-",
            icl::UnaryOperator::Not => "!",
            icl::UnaryOperator::BitwiseNot => "~",
            icl::UnaryOperator::Ref(false) => "&",
            icl::UnaryOperator::Ref(true) => "&mut ",
        }
    }

    /// Convert type to Rust type string
    fn type_to_rust_string(&self, t: &icl::Type) -> String {
        match t {
            icl::Type::U8 => "u8".to_string(),
            icl::Type::U16 => "u16".to_string(),
            icl::Type::U32 => "u32".to_string(),
            icl::Type::U64 => "u64".to_string(),
            icl::Type::I8 => "i8".to_string(),
            icl::Type::I16 => "i16".to_string(),
            icl::Type::I32 => "i32".to_string(),
            icl::Type::I64 => "i64".to_string(),
            icl::Type::F32 => "f32".to_string(),
            icl::Type::F64 => "f64".to_string(),
            icl::Type::Bool => "bool".to_string(),
            icl::Type::Void => "()".to_string(),
            icl::Type::Ref(inner) => format!("&{}", self.type_to_rust_string(inner)),
            icl::Type::MutRef(inner) => format!("&mut {}", self.type_to_rust_string(inner)),
            icl::Type::Box(inner) => format!("Box<{}>", self.type_to_rust_string(inner)),
            icl::Type::Pointer(inner) => {
                format!("*const {}", self.type_to_rust_string(inner))
            }
            icl::Type::Struct(name) => name.clone(),
            icl::Type::Array(inner, size) => {
                format!("[{}; {}]", self.type_to_rust_string(inner), size)
            }
            icl::Type::ArrayRef(inner, size) => {
                format!("&[{}; {}]", self.type_to_rust_string(inner), size)
            }
            icl::Type::MutArrayRef(inner, size) => {
                format!("&mut [{}; {}]", self.type_to_rust_string(inner), size)
            }
            icl::Type::Tuple(items) => {
                let item_strs: Vec<String> =
                    items.iter().map(|t| self.type_to_rust_string(t)).collect();
                format!("({})", item_strs.join(", "))
            }
        }
    }

    /// Format float literal with appropriate suffix
    fn format_f64_literal(&self, val: f64) -> String {
        if val == f64::INFINITY {
            "f64::INFINITY".to_string()
        } else if val == f64::NEG_INFINITY {
            "f64::NEG_INFINITY".to_string()
        } else if val.is_nan() {
            "f64::NAN".to_string()
        } else {
            format!("{}_f64", val)
        }
    }

    fn format_f32_literal(&self, val: f32) -> String {
        if val == f32::INFINITY {
            "f32::INFINITY".to_string()
        } else if val == f32::NEG_INFINITY {
            "f32::NEG_INFINITY".to_string()
        } else if val.is_nan() {
            "f32::NAN".to_string()
        } else {
            format!("{}_f32", val)
        }
    }
}

/// Sanitize a printer-generated anonymous name (e.g. `<func-0>`) into a valid Rust identifier.
fn sanitize_anon_name(name: &str) -> String {
    name.replace(':', "_")
        .replace('/', "_")
        .replace('<', "_")
        .replace('>', "_")
        .replace('-', "_")
}

pub fn generate_rust_module(icl: &icl::RCL<'_>) -> String {
    let rcg = RustCodeGenerator::new();
    rcg.generate_module(icl)
}
