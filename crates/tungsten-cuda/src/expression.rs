/*
Expression conversion from the optimized IR to CUDA C++ (SPEC.md §7).
Host inputs, permutation tables and density buffers are read through the uniform `ctx`. Integer
arithmetic wraps and shifts are masked (Java semantics), and float -> int casts saturate.
*/

use tungsten_interface::Instance;
use tungsten_interface::naming::cuda_sanitize;
use tungsten_ir::model::{BinaryOperator, ConstExpr, Expression, UnaryOperator, VariableType};

use super::converter::FunctionConverter;
use super::types::{
    IntWidth, OperandKind, convert_binary_op, device_type, find_density_input, is_float, is_integer,
    is_pointer, is_scalar, is_vector, operand_kind, unit_return_type,
};
use super::{BUFFER_LAYOUT, PARAM_CTX, PARAM_POS, uniform_arguments};
use crate::model as cuda;

use cuda::Expression as C;

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

/// Converts `value` (of derived type `from`) for a sink of type `to`: unchanged if the types agree
/// or the sink isn't a scalar, `tw::f2i8`/`tw::f2i32`/`tw::f2i64` for float -> int (saturating, NaN -> 0),
/// otherwise a C cast.
pub fn coerce(value: cuda::Expression, from: Option<&VariableType>, to: &VariableType) -> cuda::Expression {
    if from == Some(to) || !is_scalar(to) {
        return value;
    }
    match (from, IntWidth::of(to)) {
        (Some(f), Some(IntWidth::I8)) if is_float(f) => C::call("tw::f2i8", vec![value]),
        (Some(f), Some(IntWidth::I32)) if is_float(f) => C::call("tw::f2i32", vec![value]),
        (Some(f), Some(IntWidth::I64)) if is_float(f) => C::call("tw::f2i64", vec![value]),
        _ => C::cast(device_type(to), value),
    }
}

/// `(T)((U)a op (U)b)`: wrapping integer arithmetic.
fn wrapping(width: IntWidth, op: cuda::BinaryOperator, a: cuda::Expression, b: cuda::Expression) -> cuda::Expression {
    let u = |e| C::cast(width.unsigned(), e);
    C::cast(width.signed(), C::binary(op, u(a), u(b)))
}

/// An `I8` literal: `(signed char)v`.
fn byte_literal(v: i8) -> cuda::Expression {
    C::cast(cuda::Type::SignedChar, C::IntLit(v as i32))
}

/// C++ promotes `signed char` operands to `int`; narrows an `I8` result back.
fn narrow(width: Option<IntWidth>, e: cuda::Expression) -> cuda::Expression {
    match width {
        Some(IntWidth::I8) => C::cast(cuda::Type::SignedChar, e),
        _ => e,
    }
}

/// Value of an integer literal, if `e` is one.
fn int_literal(e: &Expression) -> Option<i64> {
    match e {
        Expression::Byte(v) => Some(*v as i64),
        Expression::Int(v) => Some(*v as i64),
        Expression::Long(v) => Some(*v),
        _ => None,
    }
}

impl<'a> FunctionConverter<'a> {
    /// Converts `source` for a sink of type `target` (see [`coerce`]).
    pub fn convert_to_type(&mut self, source: &Expression, target: &VariableType) -> cuda::Expression {
        let converted = self.convert_expression(source);
        coerce(converted, self.derive_type(source).as_ref(), target)
    }

    /// `index` as an `int`, wrapped in `TW_CHECK_INDEX` when debug checks are on.
    fn checked_index(&mut self, index: cuda::Expression, len: usize) -> cuda::Expression {
        if !self.config.debug_checks {
            return index;
        }
        C::call("TW_CHECK_INDEX", vec![index, C::IntLit(len as i32), self.location()])
    }

    /// Converts an expression of the current function.
    pub fn convert_expression(&mut self, expr: &Expression) -> cuda::Expression {
        match expr {
            Expression::Constant(r) => {
                self.used_constants.push(*r);
                C::name(self.naming.constant(*r))
            }
            Expression::Variable(r) => C::Name(self.declare_var(*r)),
            Expression::Float(v) => C::FloatLit(*v),
            Expression::Double(v) => C::DoubleLit(*v),
            Expression::Byte(v) => byte_literal(*v),
            Expression::Int(v) => C::IntLit(*v),
            Expression::Long(v) => C::LongLit(*v),
            Expression::Bool(v) => C::BoolLit(*v),
            Expression::FunctionCall { function, parameters } => {
                self.called.push(*function);
                let program = self.program;
                let callee = &program.functions[*function];
                // Uniform arguments first, then the helper's own parameters.
                let mut args = uniform_arguments();
                for (p, arg) in callee.parameters.iter().zip(parameters) {
                    args.push(self.convert_to_type(arg, &callee.variables[*p].t));
                }
                C::call(self.naming.helper(*function), args)
            }
            Expression::ExternCall { function_name, parameters, parameter_types, .. } => {
                let args = parameters
                    .iter()
                    .zip(parameter_types)
                    .map(|(p, t)| self.convert_to_type(p, t))
                    .collect();
                C::call(&format!("tw::{}", cuda_sanitize(function_name)), args)
            }
            Expression::DensityVariable(input, index) => {
                let program = self.program;
                let di = find_density_input(program, *input);
                let element = device_type(unit_return_type(program, di.density_function));
                let buffer = self.naming.buffer_field(&Instance::from_input(di)).to_string();
                let (dx, dy, dz) = di.dimensions;
                let index = match index {
                    Some(i) => self.convert_to_type(i, &VariableType::I32),
                    None if di.dimensions == (1, 1, 1) => C::IntLit(0),
                    None => C::call("tw::index", vec![C::name(PARAM_POS)]),
                };
                let index = self.checked_index(index, (dx * dy * dz) as usize);
                C::index(buffer_pointer(element, C::field(C::name(PARAM_CTX), "buffers"), &buffer), index)
            }
            Expression::PermutationTable(r) => C::address_of(C::arrow(
                C::field(C::name(PARAM_CTX), "auto_constants"),
                self.naming.auto_const_field(*r),
            )),
            Expression::HostInput(r) => {
                C::arrow(C::field(C::name(PARAM_CTX), "host_consts"), self.naming.host_field(*r))
            }
            Expression::BinaryOp { op, left, right } => self.convert_binary(*op, left, right),
            Expression::UnaryOp { op, operand } => self.convert_unary(*op, operand),
            Expression::Field { base, field, known_idnex, .. } => {
                let base_type = self.derive_type(base);
                let name = match (known_idnex, base_type.as_ref()) {
                    (Some(i), Some(t)) if is_vector(t) => ["x", "y", "z"][*i].to_string(),
                    _ => field.clone(),
                };
                let base = self.convert_expression(base);
                if base_type.as_ref().is_some_and(is_pointer) {
                    C::arrow(base, &name)
                } else {
                    C::field(base, &name)
                }
            }
            Expression::ArrayAccess { array, index } => {
                let array_type = self.derive_type(array);
                let base = self.convert_expression(array);
                let index = self.convert_to_type(index, &VariableType::I32);
                match array_type {
                    Some(t) if is_vector(&t) => C::call("tw::get", vec![base, index]),
                    Some(VariableType::Array(_, n)) => {
                        let index = self.checked_index(index, n);
                        C::index(base, index)
                    }
                    _ => C::index(base, index),
                }
            }
            Expression::Construct { t, args } => {
                let component = match t {
                    VariableType::Vec3 => VariableType::F64,
                    VariableType::Pos3 => VariableType::I32,
                    VariableType::Extern(_) => {
                        let args = args.iter().map(|a| self.convert_expression(a)).collect();
                        let cuda::Type::Named(name) = device_type(t) else { unreachable!() };
                        return C::call(&format!("{}::make", name), args);
                    }
                    _ => panic!("{}: cannot construct {:?} with a Construct expression", self.function_name, t),
                };
                let items = args.iter().map(|a| self.convert_to_type(a, &component)).collect();
                C::BraceInit { t: device_type(t), items }
            }
            Expression::ConstructExtern { t, args } => {
                if !matches!(t, VariableType::Extern(_)) {
                    panic!("{}: cannot construct {:?} with a ConstructExtern expression", self.function_name, t);
                }
                let fields = args.iter().map(|(n, a)| (n.to_string(), self.convert_expression(a))).collect();
                C::DesignatedInit { t: device_type(t), fields }
            }
            Expression::ArrayLiteral(items) => {
                let element = items
                    .first()
                    .and_then(|i| self.derive_type(i))
                    .unwrap_or_else(|| {
                        panic!("{}: cannot derive the element type of an array literal", self.function_name)
                    });
                let converted = items.iter().map(|i| self.convert_to_type(i, &element)).collect();
                C::BraceInit {
                    t: cuda::Type::Array(Box::new(device_type(&element)), items.len()),
                    items: vec![C::Braces(converted)],
                }
            }
            Expression::ExplicitCast { to, expr } => {
                let from = self.derive_type(expr);
                if !is_scalar(to) && from.as_ref() != Some(to) {
                    panic!("{}: cannot cast {:?} to {:?}", self.function_name, from, to);
                }
                let converted = self.convert_expression(expr);
                coerce(converted, from.as_ref(), to)
            }
        }
    }

    fn convert_binary(&mut self, op: BinaryOperator, left: &Expression, right: &Expression) -> cuda::Expression {
        use BinaryOperator as B;
        let (lt, rt) = (self.derive_type(left), self.derive_type(right));
        let l = self.convert_expression(left);
        let cuda_op = convert_binary_op(op);

        match op {
            B::Add | B::Subtract | B::Multiply | B::Divide => {
                let r = self.convert_expression(right);
                match operand_kind(lt.as_ref(), rt.as_ref()) {
                    OperandKind::Integer(width) if op == B::Divide => {
                        let quotient = if self.config.debug_checks {
                            C::call("TW_CHECK_DIV", vec![l, r, self.location()])
                        } else {
                            C::binary(cuda_op, l, r)
                        };
                        narrow(Some(width), quotient)
                    }
                    OperandKind::Integer(width) => wrapping(width, cuda_op, l, r),
                    OperandKind::Plain => C::binary(cuda_op, l, r),
                    OperandKind::Unknown => {
                        self.warn(format!("cannot derive the operand types of `{:?}`; emitted a plain operator", op));
                        C::binary(cuda_op, l, r)
                    }
                }
            }
            B::ShiftLeft | B::ShiftRight | B::LogicalShiftRight => {
                let width = match lt.as_ref().and_then(IntWidth::of) {
                    Some(w) => w,
                    None => match operand_kind(lt.as_ref(), None) {
                        OperandKind::Integer(w) => w,
                        _ => {
                            self.warn(format!("cannot derive the width of `{:?}`; emitted a plain shift", op));
                            let r = self.convert_expression(right);
                            return C::binary(cuda_op, l, r);
                        }
                    },
                };
                // Java masks the shift count; a literal count is folded.
                let count = match int_literal(right) {
                    Some(v) => C::IntLit((v & width.shift_mask()) as i32),
                    None => {
                        let r = self.convert_expression(right);
                        C::binary(cuda::BinaryOperator::BitAnd, r, C::IntLit(width.shift_mask() as i32))
                    }
                };
                match op {
                    // Arithmetic shift of a signed value; defined in C++20.
                    B::ShiftRight => narrow(Some(width), C::binary(cuda_op, l, count)),
                    _ => C::cast(width.signed(), C::binary(cuda_op, C::cast(width.unsigned(), l), count)),
                }
            }
            _ => {
                let r = self.convert_expression(right);
                let bitwise = matches!(op, B::BitAnd | B::BitOr | B::BitXor);
                let result = C::binary(cuda_op, l, r);
                if bitwise { narrow(lt.as_ref().and_then(IntWidth::of), result) } else { result }
            }
        }
    }

    /// Whether `e` converts to a C++ lvalue whose address may be taken.
    fn is_lvalue(&self, e: &Expression) -> bool {
        match e {
            Expression::Variable(_) | Expression::DensityVariable(..) => true,
            Expression::Field { base, .. } => {
                self.is_lvalue(base) || self.derive_type(base).as_ref().is_some_and(is_pointer)
            }
            Expression::ArrayAccess { array, .. } => {
                !self.derive_type(array).as_ref().is_some_and(is_vector) && self.is_lvalue(array)
            }
            _ => false,
        }
    }

    fn convert_unary(&mut self, op: UnaryOperator, operand: &Expression) -> cuda::Expression {
        let t = self.derive_type(operand);
        match op {
            UnaryOperator::Negate => match t.as_ref().and_then(IntWidth::of) {
                Some(width) => match int_literal(operand) {
                    Some(v) if width == IntWidth::I8 => byte_literal((v as i8).wrapping_neg()),
                    Some(v) if width == IntWidth::I32 => C::IntLit((v as i32).wrapping_neg()),
                    Some(v) => C::LongLit(v.wrapping_neg()),
                    None => {
                        let a = self.convert_expression(operand);
                        C::cast(
                            width.signed(),
                            C::binary(cuda::BinaryOperator::Subtract, C::IntLit(0), C::cast(width.unsigned(), a)),
                        )
                    }
                },
                None => C::unary(cuda::UnaryOperator::Negate, self.convert_expression(operand)),
            },
            // IR `!` on an integer is bitwise not.
            UnaryOperator::Not => narrow(
                t.as_ref().and_then(IntWidth::of),
                C::unary(
                    if t.as_ref().is_some_and(is_integer) {
                        cuda::UnaryOperator::BitNot
                    } else {
                        cuda::UnaryOperator::Not
                    },
                    self.convert_expression(operand),
                ),
            ),
            // Samplers are pointers already.
            UnaryOperator::Ref(_) if t.as_ref().is_some_and(is_pointer) => self.convert_expression(operand),
            UnaryOperator::Ref(_) => {
                let lvalue = self.is_lvalue(operand);
                let converted = self.convert_expression(operand);
                if lvalue {
                    C::address_of(converted)
                } else {
                    // Materialized in the expression: lives until the end of the full expression.
                    C::address_of(C::call("tw::lval", vec![converted]))
                }
            }
        }
    }
}

/// `tw::buffer<T>(base, BufferLayout::field)`: pointer to an instance's buffer in the slab.
pub fn buffer_pointer(element: cuda::Type, base: cuda::Expression, field: &str) -> cuda::Expression {
    C::template_call(
        "tw::buffer",
        vec![element],
        vec![base, C::Name(format!("{}::{}", BUFFER_LAYOUT, field))],
    )
}
