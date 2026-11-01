/*
Expression conversion from optimized SPMT to RCL.
Host inputs, permutation tables and density buffers are read from the uniform
`host_consts`, `auto_constants` and `buffers` parameters.
*/

use std::rc::Rc;

use super::converter::FunctionConverter;
use super::types::{convert_binary_op, convert_type, convert_unary_op, derive_const_type};
use crate::model as rcl;
use tungsten_interface::naming::sanitize_name;
use tungsten_interface::{Instance, Interface};
use tungsten_ir::model::{
    BinaryOperator, ConstExpr, Constant, Expression, OptimizedProgram, Ref, VariableType,
};

/// `base.name`
fn field<'m>(base: &Rc<rcl::Variable>, name: &str) -> rcl::Expression<'m> {
    rcl::Expression::Field {
        base: Box::new(rcl::Expression::Variable(base.clone())),
        field: name.to_string(),
    }
}

/// Type name used for `Type::new(..)`; only structured types can be constructed this way.
fn constructor_name(t: &VariableType) -> String {
    match t {
        VariableType::Vec3 => "Vec3".to_string(),
        VariableType::Pos3 => "Pos3".to_string(),
        VariableType::Extern(name) => sanitize_name(name),
        other => panic!("cannot construct {:?} with a Construct expression", other),
    }
}

fn constructor_call<'m>(t: &VariableType, args: Vec<rcl::Expression<'m>>) -> rcl::Expression<'m> {
    rcl::Expression::LateBoundCall {
        function_name: format!("{}::new", constructor_name(t)),
        argument_types: vec![convert_type(t); args.len()],
        return_type: convert_type(t),
        arguments: args,
    }
}

/// Reference to the module-level constant `r`, typed from its initializer.
pub fn constant_variable(
    program: &OptimizedProgram,
    naming: &Interface,
    r: Ref<Constant>,
) -> Rc<rcl::Variable> {
    let t = derive_const_type(program, &program.constants[r].value).unwrap_or_else(|| {
        panic!(
            "cannot derive the type of constant '{}'",
            program.constants[r].name
        )
    });
    Rc::new(rcl::Variable {
        name: Some(naming.constant(r).to_string()),
        t: convert_type(&t),
        mutable: false,
    })
}

/// Constants referenced directly by `e`.
pub fn const_refs(e: &ConstExpr, out: &mut Vec<Ref<Constant>>) {
    match e {
        ConstExpr::Constant(r) => out.push(*r),
        ConstExpr::BinaryOp { left, right, .. } => {
            const_refs(left, out);
            const_refs(right, out);
        }
        ConstExpr::UnaryOp { operand, .. } => const_refs(operand, out),
        ConstExpr::ExplicitCast { expr, .. } => const_refs(expr, out),
        ConstExpr::Field { base, .. } => const_refs(base, out),
        ConstExpr::ArrayAccess { array, index } => {
            const_refs(array, out);
            const_refs(index, out);
        }
        ConstExpr::ArrayLiteral(items) | ConstExpr::Construct { args: items, .. } => {
            items.iter().for_each(|i| const_refs(i, out))
        }
        ConstExpr::Float(_)
        | ConstExpr::Double(_)
        | ConstExpr::Byte(_)
        | ConstExpr::Int(_)
        | ConstExpr::Long(_)
        | ConstExpr::Bool(_) => {}
    }
}

/// Converts a constant initializer; constants may only reference other constants.
pub fn convert_const_expr<'m>(
    program: &OptimizedProgram,
    naming: &Interface,
    e: &ConstExpr,
) -> rcl::Expression<'m> {
    use rcl::Expression as R;
    let conv = |e: &ConstExpr| Box::new(convert_const_expr(program, naming, e));
    match e {
        ConstExpr::Constant(r) => R::Variable(constant_variable(program, naming, *r)),
        ConstExpr::Float(v) => R::F32Literal(*v),
        ConstExpr::Double(v) => R::F64Literal(*v),
        ConstExpr::Byte(v) => R::I8Literal(*v),
        ConstExpr::Int(v) => R::I32Literal(*v),
        ConstExpr::Long(v) => R::I64Literal(*v),
        ConstExpr::Bool(v) => R::BoolLiteral(*v),
        ConstExpr::BinaryOp { op, left, right } => R::BinaryOp {
            op: convert_binary_op(*op),
            left: conv(left),
            right: conv(right),
        },
        ConstExpr::UnaryOp { op, operand } => R::UnaryOp {
            op: convert_unary_op(*op),
            operand: conv(operand),
        },
        ConstExpr::ExplicitCast { to, expr } => R::Cast {
            expr: conv(expr),
            to_type: convert_type(to),
        },
        ConstExpr::Field { base, field, .. } => R::Field {
            base: conv(base),
            field: field.clone(),
        },
        ConstExpr::ArrayAccess { array, index } => R::Index {
            base: conv(array),
            index: conv(index),
        },
        ConstExpr::ArrayLiteral(items) => R::ArrayLiteral(items.iter().map(|i| *conv(i)).collect()),
        ConstExpr::Construct { t, args } => {
            constructor_call(t, args.iter().map(|a| *conv(a)).collect())
        }
    }
}

impl<'a> FunctionConverter<'a> {
    /// Converts an expression of the current function.
    ///
    /// Host inputs read `host_consts.<field>`, permutation tables borrow `auto_constants.<field>`,
    /// and density inputs index `buffers.<field>` (explicit index, `0` for single values, else `index(p)`).
    pub fn convert_expression<'m>(&mut self, expr: &Expression) -> rcl::Expression<'m> {
        use rcl::Expression as R;
        match expr {
            Expression::Constant(r) => {
                self.used_constants.push(*r);
                R::Variable(constant_variable(self.program, self.naming, *r))
            }
            Expression::Variable(r) => R::Variable(self.declare_var(*r)),
            Expression::Float(v) => R::F32Literal(*v),
            Expression::Double(v) => R::F64Literal(*v),
            Expression::Byte(v) => R::I8Literal(*v),
            Expression::Int(v) => R::I32Literal(*v),
            Expression::Long(v) => R::I64Literal(*v),
            Expression::Bool(v) => R::BoolLiteral(*v),
            Expression::FunctionCall {
                function,
                parameters,
            } => {
                self.called.push(*function);
                let callee = &self.program.functions[*function];
                // Uniform arguments first, then the helper's own parameters.
                let mut arguments = self.uniform.as_args();
                let mut argument_types: Vec<rcl::Type> = super::uniform_parameters()
                    .into_iter()
                    .map(|p| p.t)
                    .collect();
                for p in &callee.parameters {
                    argument_types.push(convert_type(&callee.variables[*p].t));
                }
                for p in parameters {
                    arguments.push(self.convert_expression(p));
                }
                R::LateBoundCall {
                    function_name: self.naming.helper(*function).to_string(),
                    argument_types,
                    return_type: convert_type(&callee.return_type),
                    arguments,
                }
            }
            Expression::ExternCall {
                function_name,
                parameters,
                parameter_types,
                return_type,
            } => {
                // Not a real extern: a call by name to a runtime function.
                let arguments = parameters
                    .iter()
                    .zip(parameter_types)
                    .map(|(p, t)| self.convert_argument(p, t))
                    .collect();
                R::LateBoundCall {
                    function_name: function_name.clone(),
                    argument_types: parameter_types.iter().map(convert_type).collect(),
                    return_type: convert_type(return_type),
                    arguments,
                }
            }
            Expression::DensityVariable(input, index) => {
                let di = self.find_density_input(*input);
                let buffer = self.naming.buffer_field(&Instance::from_input(di));
                let index = match index {
                    Some(i) => self.convert_expression(i),
                    None if di.dimensions == (1, 1, 1) => R::I32Literal(0),
                    None => R::LateBoundCall {
                        function_name: "index".to_string(),
                        argument_types: vec![rcl::Type::Struct("PositionIterator".to_string())],
                        return_type: rcl::Type::I32,
                        arguments: vec![R::Variable(self.uniform.it.clone())],
                    },
                };
                R::Index {
                    base: Box::new(field(&self.uniform.buffers, buffer)),
                    index: Box::new(index),
                }
            }
            Expression::PermutationTable(r) => {
                let name = self.naming.auto_const_field(*r);
                R::Ref(Box::new(field(&self.uniform.auto_consts, name)))
            }
            Expression::HostInput(r) => {
                field(&self.uniform.host_consts, self.naming.host_field(*r))
            }
            Expression::BinaryOp { op, left, right } => {
                let (l, r) = self.convert_binary_operands(*op, left, right);
                R::BinaryOp {
                    op: convert_binary_op(*op),
                    left: Box::new(l),
                    right: Box::new(r),
                }
            }
            Expression::UnaryOp { op, operand } => R::UnaryOp {
                op: convert_unary_op(*op),
                operand: Box::new(self.convert_expression(operand)),
            },
            Expression::Field { base, field, .. } => R::Field {
                base: Box::new(self.convert_expression(base)),
                field: field.clone(),
            },
            Expression::ArrayAccess { array, index } => R::Index {
                base: Box::new(self.convert_expression(array)),
                index: Box::new(self.convert_expression(index)),
            },
            Expression::Construct { t, args } => {
                let args = args.iter().map(|a| self.convert_expression(a)).collect();
                constructor_call(t, args)
            }
            Expression::ConstructExtern { t, args } => R::Construct {
                t: convert_type(t),
                args: args
                    .iter()
                    .map(|(n, a)| (*n, self.convert_expression(a)))
                    .collect(),
            },
            Expression::ArrayLiteral(items) => {
                R::ArrayLiteral(items.iter().map(|i| self.convert_expression(i)).collect())
            }
            Expression::ExplicitCast { to, expr } => R::Cast {
                expr: Box::new(self.convert_expression(expr)),
                to_type: convert_type(to),
            },
        }
    }

    /// Convert `source`, casting to `target` when its derived type differs.
    pub fn convert_expression_to_type<'m>(
        &mut self,
        source: &Expression,
        target: &VariableType,
    ) -> rcl::Expression<'m> {
        let converted = self.convert_expression(source);
        match self.derive_type(source) {
            Some(t) if t != *target => rcl::Expression::Cast {
                expr: Box::new(converted),
                to_type: convert_type(target),
            },
            _ => converted,
        }
    }

    /// Extern arguments are cast to f32/i32 unless they already are matching literals.
    fn convert_argument<'m>(
        &mut self,
        arg: &Expression,
        target: &VariableType,
    ) -> rcl::Expression<'m> {
        let converted = self.convert_expression(arg);
        match (target, arg) {
            (VariableType::F32, Expression::Float(_)) | (VariableType::I32, Expression::Int(_)) => {
                converted
            }
            (VariableType::F32 | VariableType::I32, _) => rcl::Expression::Cast {
                expr: Box::new(converted),
                to_type: convert_type(target),
            },
            _ => converted,
        }
    }

    /// Rust does not add `i32 + bool`; cast the bool side.
    fn convert_binary_operands<'m>(
        &mut self,
        op: BinaryOperator,
        left: &Expression,
        right: &Expression,
    ) -> (rcl::Expression<'m>, rcl::Expression<'m>) {
        let (lt, rt) = (self.derive_type(left), self.derive_type(right));
        let (l, r) = (
            self.convert_expression(left),
            self.convert_expression(right),
        );
        let arithmetic = matches!(
            op,
            BinaryOperator::Add
                | BinaryOperator::Subtract
                | BinaryOperator::Multiply
                | BinaryOperator::Divide
        );
        let to_i32 = |e: rcl::Expression<'m>| rcl::Expression::Cast {
            expr: Box::new(e),
            to_type: rcl::Type::I32,
        };
        match (arithmetic, lt, rt) {
            (true, Some(VariableType::I32), Some(VariableType::Bool)) => (l, to_i32(r)),
            (true, Some(VariableType::Bool), Some(VariableType::I32)) => (to_i32(l), r),
            _ => (l, r),
        }
    }
}
