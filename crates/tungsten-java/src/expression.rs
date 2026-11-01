/*
Expression conversion from optimized SPMT to Java.
Host inputs, permutation tables and density buffers are read from the uniform
`host_consts`, `auto_constants` and `buffers` parameters. Constants are always qualified with
the generated class so locals cannot shadow them.
*/

use super::converter::FunctionConverter;
use super::types::{boxed_class, find_density_input, convert_binary_op, convert_type, holder_class, is_object, numeric_literal};
use super::{HELPERS_CLASS, PARAM_AUTO_CONSTS, PARAM_BUFFERS, PARAM_HOST_CONSTS, PARAM_POS, uniform_arguments};
use tungsten_interface::Instance;
use tungsten_interface::naming::java_sanitize;
use crate::model as java;
use tungsten_ir::model::VariableType;
use tungsten_ir::model::{BinaryOperator, ConstExpr, Expression, UnaryOperator};

use java::Expression as J;

/// `ConstExpr` as an ordinary expression, so constants share the expression conversion.
pub fn const_to_expression(e: &ConstExpr) -> Expression {
    let b = |e: &ConstExpr| Box::new(const_to_expression(e));
    match e {
        ConstExpr::Constant(r) => Expression::Constant(*r),
        ConstExpr::Float(v) => Expression::Float(*v),
        ConstExpr::Double(v) => Expression::Double(*v),
        ConstExpr::Byte(v) => Expression::Byte(*v),
        ConstExpr::Int(v) => Expression::Int(*v),
        ConstExpr::Long(v) => Expression::Long(*v),
        ConstExpr::Bool(v) => Expression::Bool(*v),
        ConstExpr::BinaryOp { op, left, right } => {
            Expression::BinaryOp { op: *op, left: b(left), right: b(right) }
        }
        ConstExpr::UnaryOp { op, operand } => Expression::UnaryOp { op: *op, operand: b(operand) },
        ConstExpr::ExplicitCast { to, expr } => Expression::ExplicitCast { to: to.clone(), expr: b(expr) },
        ConstExpr::Field { base, field, type_of_field, known_idnex } => Expression::Field {
            base: b(base),
            field: field.clone(),
            type_of_field: type_of_field.clone(),
            known_idnex: *known_idnex,
        },
        ConstExpr::ArrayAccess { array, index } => {
            Expression::ArrayAccess { array: b(array), index: b(index) }
        }
        ConstExpr::ArrayLiteral(items) => {
            Expression::ArrayLiteral(items.iter().map(const_to_expression).collect())
        }
        ConstExpr::Construct { t, args } => {
            Expression::Construct { t: t.clone(), args: args.iter().map(const_to_expression).collect() }
        }
    }
}

fn is_vector(t: &Option<VariableType>) -> bool {
    matches!(t, Some(VariableType::Vec3 | VariableType::Pos3))
}

fn is_integer(t: &Option<VariableType>) -> bool {
    matches!(t, Some(VariableType::I8 | VariableType::I32 | VariableType::I64))
}

/// `(b ? 1 : 0)` in Java type `t`.
fn bool_to_number(b: java::Expression, t: &java::Type) -> java::Expression {
    J::Conditional {
        cond: Box::new(b),
        then: Box::new(numeric_literal(t, 1)),
        other: Box::new(numeric_literal(t, 0)),
    }
}

/// Java promotes `byte` operands to `int`; narrows the result back when the IR type is `I8`.
fn narrow_byte(e: java::Expression, t: &Option<VariableType>) -> java::Expression {
    if *t == Some(VariableType::I8) { J::cast(java::Type::Byte, e) } else { e }
}

/// Converts `value` (of derived type `from`) for a sink of type `to`: unchanged if the types agree
/// or the sink is an object, `bool -> number` via a conditional, otherwise an explicit cast.
pub fn coerce(value: java::Expression, from: Option<VariableType>, to: &VariableType) -> java::Expression {
    let target = convert_type(to);
    if !target.is_primitive() || from.as_ref() == Some(to) {
        return value;
    }
    if from == Some(VariableType::Bool) && target != java::Type::Boolean {
        return bool_to_number(value, &target);
    }
    J::cast(target, value)
}

impl<'a> FunctionConverter<'a> {
    /// Converts `source` for a sink of type `target` (see [`coerce`]).
    pub fn convert_to_type(&mut self, source: &Expression, target: &VariableType) -> java::Expression {
        let converted = self.convert_expression(source);
        coerce(converted, self.derive_type(source), target)
    }

    /// Converts an expression of the current function.
    pub fn convert_expression(&mut self, expr: &Expression) -> java::Expression {
        match expr {
            Expression::Constant(r) => {
                self.used_constants.push(*r);
                J::field(J::name(self.class_name), self.naming.constant(*r))
            }
            Expression::Variable(r) => J::Name(self.declare_var(*r)),
            Expression::Float(v) => J::FloatLit(*v),
            Expression::Double(v) => J::DoubleLit(*v),
            Expression::Byte(v) => numeric_literal(&java::Type::Byte, *v as i32),
            Expression::Int(v) => J::IntLit(*v),
            Expression::Long(v) => J::LongLit(*v),
            Expression::Bool(v) => J::BoolLit(*v),
            Expression::FunctionCall { function, parameters } => {
                self.called.push(*function);
                let program = self.program;
                let callee = &program.functions[*function];
                // Uniform arguments first, then the helper's own parameters.
                let mut args = uniform_arguments();
                for (p, arg) in callee.parameters.iter().zip(parameters) {
                    args.push(self.convert_to_type(arg, &callee.variables[*p].t));
                }
                J::call(None, self.naming.helper(*function), args)
            }
            Expression::ExternCall { function_name, parameters, parameter_types, .. } => {
                let args = parameters
                    .iter()
                    .zip(parameter_types)
                    .map(|(p, t)| self.convert_to_type(p, t))
                    .collect();
                J::call(Some(J::name(HELPERS_CLASS)), &java_sanitize(function_name), args)
            }
            Expression::DensityVariable(input, index) => {
                let di = find_density_input(self.program, *input);
                let buffer = self.naming.buffer_field(&Instance::from_input(di)).to_string();
                let index = match index {
                    Some(i) => self.convert_to_type(i, &VariableType::I32),
                    None if di.dimensions == (1, 1, 1) => J::IntLit(0),
                    None => J::field(J::name(PARAM_POS), "index"),
                };
                J::Index { base: Box::new(J::field(J::name(PARAM_BUFFERS), &buffer)), index: Box::new(index) }
            }
            Expression::PermutationTable(r) => {
                J::field(J::name(PARAM_AUTO_CONSTS), self.naming.auto_const_field(*r))
            }
            Expression::HostInput(r) => J::field(J::name(PARAM_HOST_CONSTS), self.naming.host_field(*r)),
            Expression::BinaryOp { op, left, right } => self.convert_binary(*op, left, right),
            Expression::UnaryOp { op, operand } => self.convert_unary(*op, operand),
            Expression::Field { base, field, .. } => J::field(self.convert_expression(base), field),
            Expression::ArrayAccess { array, index } => {
                let base = self.convert_expression(array);
                let index = self.convert_to_type(index, &VariableType::I32);
                if is_vector(&self.derive_type(array)) {
                    J::call(Some(base), "get", vec![index])
                } else {
                    J::Index { base: Box::new(base), index: Box::new(index) }
                }
            }
            Expression::Construct { t, args } => {
                let class = match convert_type(t) {
                    java::Type::Class(c) if matches!(t, VariableType::Vec3 | VariableType::Pos3 | VariableType::Extern(_)) => c,
                    _ => panic!("cannot construct {:?} with a Construct expression", t),
                };
                let args = args.iter().map(|a| self.convert_expression(a)).collect();
                J::New { class, args, comment: None }
            }
            Expression::ConstructExtern { t, args } => {
                let java::Type::Class(class) = convert_type(t) else {
                    panic!("cannot construct {:?} with a ConstructExtern expression", t);
                };
                let names = args.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
                let args = args.iter().map(|(_, a)| self.convert_expression(a)).collect();
                J::New { class, args, comment: Some(names) }
            }
            Expression::ArrayLiteral(items) => {
                let element = items
                    .first()
                    .and_then(|i| self.derive_type(i))
                    .unwrap_or_else(|| panic!("cannot derive the element type of an array literal"));
                let items = items.iter().map(|i| self.convert_to_type(i, &element)).collect();
                J::NewArray { element: convert_type(&element), items }
            }
            Expression::ExplicitCast { to, expr } => {
                let from = self.derive_type(expr);
                let converted = self.convert_expression(expr);
                if is_object(to) {
                    panic!("cannot cast to the object type {:?}", to);
                }
                if *to == VariableType::Bool && from.as_ref().is_some_and(|f| *f != VariableType::Bool) {
                    panic!("cannot cast {:?} to bool", from);
                }
                match from {
                    Some(VariableType::Bool) if *to != VariableType::Bool => {
                        bool_to_number(converted, &convert_type(to))
                    }
                    // `(byte)` on a float goes through `int` and wraps; Rust `as i8` saturates.
                    Some(VariableType::F32 | VariableType::F64) if *to == VariableType::I8 => {
                        let int = J::cast(java::Type::Int, converted);
                        let min = J::call(Some(J::name("Math")), "min", vec![J::IntLit(127), int]);
                        let clamped = J::call(Some(J::name("Math")), "max", vec![J::IntLit(-128), min]);
                        J::cast(java::Type::Byte, clamped)
                    }
                    _ => J::cast(convert_type(to), converted),
                }
            }
        }
    }

    fn convert_binary(&mut self, op: BinaryOperator, left: &Expression, right: &Expression) -> java::Expression {
        use BinaryOperator as B;
        let (lt, rt) = (self.derive_type(left), self.derive_type(right));
        let (l, r) = (self.convert_expression(left), self.convert_expression(right));
        let arithmetic = matches!(op, B::Add | B::Subtract | B::Multiply | B::Divide);

        // Vec3/Pos3 arithmetic goes through the runtime's methods.
        if arithmetic && (is_vector(&lt) || is_vector(&rt)) {
            let method = match op {
                B::Add => "add",
                B::Subtract => "sub",
                B::Multiply => "mul",
                _ => "div",
            };
            return J::call(Some(l), method, vec![r]);
        }

        // Objects compare by value, as Rust's PartialEq does.
        if matches!(op, B::Equal | B::NotEqual) {
            let object = |t: &Option<VariableType>| t.as_ref().is_some_and(is_object);
            if object(&lt) || object(&rt) {
                let array = matches!(lt, Some(VariableType::Array(..))) || matches!(rt, Some(VariableType::Array(..)));
                let eq = if array {
                    J::call(Some(J::name("java.util.Arrays")), "equals", vec![l, r])
                } else {
                    J::call(Some(l), "equals", vec![r])
                };
                return if op == B::Equal {
                    eq
                } else {
                    J::Unary { op: java::UnaryOperator::Not, operand: Box::new(eq) }
                };
            }
        }

        // Java does not add `int + boolean`; the bool side becomes `(b ? 1 : 0)`.
        let (l, r) = match (arithmetic, &lt, &rt) {
            (true, Some(VariableType::I32), Some(VariableType::Bool)) => (l, bool_to_number(r, &java::Type::Int)),
            (true, Some(VariableType::Bool), Some(VariableType::I32)) => (bool_to_number(l, &java::Type::Int), r),
            _ => (l, r),
        };
        let comparison = matches!(
            op,
            B::Equal | B::NotEqual | B::Less | B::LessEqual | B::Greater | B::GreaterEqual | B::And | B::Or
        );
        let result = J::binary(convert_binary_op(op), l, r);
        if comparison { result } else { narrow_byte(result, &lt) }
    }

    fn convert_unary(&mut self, op: UnaryOperator, operand: &Expression) -> java::Expression {
        let t = self.derive_type(operand);
        match op {
            UnaryOperator::Negate => narrow_byte(
                J::Unary { op: java::UnaryOperator::Negate, operand: Box::new(self.convert_expression(operand)) },
                &t,
            ),
            // Rust `!` on an integer is bitwise not.
            UnaryOperator::Not => narrow_byte(
                J::Unary {
                    op: if is_integer(&t) { java::UnaryOperator::BitNot } else { java::UnaryOperator::Not },
                    operand: Box::new(self.convert_expression(operand)),
                },
                &t,
            ),
            // Objects are references already.
            UnaryOperator::Ref(_) if t.as_ref().is_none_or(is_object) => self.convert_expression(operand),
            UnaryOperator::Ref(false) => {
                let boxed = boxed_class(&convert_type(t.as_ref().unwrap()));
                J::call(Some(J::name(boxed)), "valueOf", vec![self.convert_expression(operand)])
            }
            UnaryOperator::Ref(true) => {
                // A mutable holder, written back after the statement (see statement.rs).
                let Expression::Variable(v) = operand else {
                    panic!("Ref(true) on a non-place primitive");
                };
                let var = self.declare_var(*v);
                let java_t = self.java_var_type(*v);
                let holder = holder_class(&java_t);
                let holder_name = self.fresh_name(&format!("{}_ref", var));
                self.pre.push(java::Statement::Declare {
                    t: java::Type::class(holder),
                    name: holder_name.clone(),
                    init: Some(J::New { class: holder.to_string(), args: vec![J::Name(var.clone())], comment: None }),
                    is_final: true,
                });
                self.post.push(java::Statement::Assign {
                    target: J::Name(var),
                    value: J::field(J::Name(holder_name.clone()), "value"),
                });
                self.written_back.push(*v);
                J::Name(holder_name)
            }
        }
    }
}
