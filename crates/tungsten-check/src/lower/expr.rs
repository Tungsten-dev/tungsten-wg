//! Expressions. Errors on a node are reported before its children's.

use tungsten_core::{
    Ambient, Axis, BinOp, Expr, ExternId, FieldInit, FieldSel, InputId, Lit, RecordId, UnOp,
};
use tungsten_ir::{AmbientKind, BinaryOperator, Expression, UnaryOperator, VariableType};

use super::Ctx;
use super::types::type_name;
use crate::error::{EntityKind, LowerErrorKind, Step};

/// Stands in for a subexpression that failed to lower. Never escapes: a lowering with errors
/// returns no program.
fn placeholder() -> Expr {
    Expr::Lit(Lit::Bool(false))
}

fn axis(field: &str) -> Option<Axis> {
    match field {
        "x" => Some(Axis::X),
        "y" => Some(Axis::Y),
        "z" => Some(Axis::Z),
        _ => None,
    }
}

fn bin_op(op: BinaryOperator) -> BinOp {
    use BinaryOperator as B;
    match op {
        B::Add => BinOp::Add,
        B::Subtract => BinOp::Subtract,
        B::Multiply => BinOp::Multiply,
        B::Divide => BinOp::Divide,
        B::Equal => BinOp::Equal,
        B::NotEqual => BinOp::NotEqual,
        B::Less => BinOp::Less,
        B::LessEqual => BinOp::LessEqual,
        B::Greater => BinOp::Greater,
        B::GreaterEqual => BinOp::GreaterEqual,
        B::And => BinOp::And,
        B::Or => BinOp::Or,
        B::ShiftLeft => BinOp::ShiftLeft,
        B::ShiftRight => BinOp::ShiftRight,
        B::LogicalShiftRight => BinOp::LogicalShiftRight,
        B::BitOr => BinOp::BitOr,
        B::BitAnd => BinOp::BitAnd,
        B::BitXor => BinOp::BitXor,
    }
}

impl<'p, 'r> Ctx<'p, 'r> {
    fn boxed(&mut self, step: Step, e: &Expression) -> Box<Expr> {
        Box::new(self.at(step, |cx| cx.expr(e)))
    }

    fn list(&mut self, es: &[Expression], step: fn(usize) -> Step) -> Vec<Expr> {
        es.iter()
            .enumerate()
            .map(|(i, e)| self.at(step(i), |cx| cx.expr(e)))
            .collect()
    }

    pub(crate) fn expr(&mut self, e: &Expression) -> Expr {
        match e {
            Expression::Constant(r) => Expr::Const(self.const_ref(*r)),
            Expression::Variable(r) => Expr::Local(self.local(*r)),
            Expression::Float(v) => Expr::Lit(Lit::F32(v.to_bits())),
            Expression::Double(v) => Expr::Lit(Lit::F64(v.to_bits())),
            Expression::Byte(v) => Expr::Lit(Lit::I8(*v)),
            Expression::Int(v) => Expr::Lit(Lit::I32(*v)),
            Expression::Long(v) => Expr::Lit(Lit::I64(*v)),
            Expression::Bool(v) => Expr::Lit(Lit::Bool(*v)),
            Expression::FunctionCall {
                function,
                parameters,
            } => {
                let func = self.fn_ref(*function);
                let args = self.list(parameters, Step::Arg);
                Expr::Call { func, args }
            }
            Expression::ExternCall {
                function_name,
                parameters,
                parameter_types,
                return_type,
            } => {
                let ext = self.reg.extern_id(function_name).unwrap_or_else(|| {
                        self.err(LowerErrorKind::UnknownExtern(function_name.clone()));
                        ExternId(0)
                    });
                let params = parameter_types
                    .iter()
                    .enumerate()
                    .map(|(i, t)| self.at(Step::Arg(i), |cx| cx.at(Step::Type, |cx| cx.ty(t))))
                    .collect();
                let ret = self.at(Step::Ret, |cx| cx.ty(return_type));
                let args = self.list(parameters, Step::Arg);
                Expr::ExternCall {
                    ext,
                    params,
                    ret,
                    args,
                }
            }
            Expression::DensityVariable(r, index) => {
                let input = self.map.inputs.id(*r).unwrap_or_else(|| {
                    self.err(LowerErrorKind::DanglingRef {
                        kind: EntityKind::DensityInput,
                    });
                    InputId(0)
                });
                let index = index.as_ref().map(|i| self.boxed(Step::Index, i));
                Expr::Density { input, index }
            }
            Expression::PermutationTable(r) => Expr::Table(self.table_ref(*r)),
            Expression::HostInput(r) => Expr::Host(self.host_ref(*r)),
            Expression::Ambient(k) => Expr::Ambient(match k {
                AmbientKind::Origin => Ambient::Origin,
                AmbientKind::Pos3 => Ambient::Pos3,
                AmbientKind::OriginScale => Ambient::OriginScale,
                AmbientKind::PositionScale => Ambient::PositionScale,
            }),
            Expression::BinaryOp { op, left, right } => Expr::Binary {
                op: bin_op(*op),
                left: self.boxed(Step::Left, left),
                right: self.boxed(Step::Right, right),
            },
            Expression::UnaryOp { op, operand } => {
                let op = match op {
                    UnaryOperator::Negate => UnOp::Negate,
                    UnaryOperator::Not => UnOp::Not,
                    // The identity.
                    UnaryOperator::Ref(_) => return self.at(Step::Operand, |cx| cx.expr(operand)),
                };
                Expr::Unary {
                    op,
                    operand: self.boxed(Step::Operand, operand),
                }
            }
            Expression::Field {
                base,
                field,
                type_of_field,
                base_type,
            } => {
                let sel = self.field_sel(base_type, field);
                let ty = self.at(Step::Type, |cx| cx.ty(type_of_field));
                let base = self.boxed(Step::Base, base);
                Expr::Field { base, sel, ty }
            }
            Expression::ArrayAccess { array, index } => Expr::Index {
                array: self.boxed(Step::Base, array),
                index: self.boxed(Step::Index, index),
            },
            Expression::Construct { t, args } => self.construct(t, args),
            Expression::ConstructExtern { t, args } => self.construct_extern(t, args),
            Expression::ArrayLiteral(elems) => Expr::ArrayLit(self.list(elems, Step::Element)),
            Expression::ExplicitCast { to, expr } => {
                let to = self.at(Step::Type, |cx| cx.ty(to));
                let operand = self.boxed(Step::Operand, expr);
                Expr::Cast { to, operand }
            }
        }
    }

    /// Picks the selector from the IR's `base_type`. That annotation is trusted only for this:
    /// if it is wrong, the selector doesn't fit the base and the checker rejects. The field's own
    /// type is lowered separately.
    fn field_sel(&mut self, bt: &VariableType, field: &str) -> FieldSel {
        let fallback = FieldSel::Vec3(Axis::X);
        let unknown = |cx: &mut Self| {
            cx.err(LowerErrorKind::UnknownField {
                base: type_name(bt),
                field: field.to_string(),
            });
            fallback.clone()
        };
        match bt {
            // Reported where the density input is read.
            VariableType::DensityInput => fallback,
            VariableType::Vec3 => match axis(field) {
                Some(a) => FieldSel::Vec3(a),
                None => unknown(self),
            },
            VariableType::Pos3 => match axis(field) {
                Some(a) => FieldSel::Pos3(a),
                None => unknown(self),
            },
            VariableType::Extern(name) => {
                let Some(r) = self.reg.record_id(name) else {
                    self.err(LowerErrorKind::UnknownRecord(name.to_string()));
                    return fallback;
                };
                match self.reg.field_index(r, field) {
                    Some(i) => FieldSel::Record(r, i),
                    None => unknown(self),
                }
            }
            other => {
                self.err(LowerErrorKind::FieldOnNonStruct {
                    base: type_name(other),
                });
                fallback
            }
        }
    }

    fn construct(&mut self, t: &VariableType, args: &[Expression]) -> Expr {
        match t {
            VariableType::Vec3 | VariableType::Pos3 => {
                if args.len() != 3 {
                    self.err(LowerErrorKind::ConstructArity {
                        expected: 3,
                        found: args.len(),
                    });
                    self.list(args, Step::Arg);
                    return placeholder();
                }
                let x = self.boxed(Step::Arg(0), &args[0]);
                let y = self.boxed(Step::Arg(1), &args[1]);
                let z = self.boxed(Step::Arg(2), &args[2]);
                if *t == VariableType::Vec3 {
                    Expr::MakeVec3 { x, y, z }
                } else {
                    Expr::MakePos3 { x, y, z }
                }
            }
            VariableType::Extern(name) => {
                let record = match self.reg.record_id(name) {
                    Some(r) => {
                        if let Some(n) = self.reg.record_decl(r).map(|d| d.fields.len())
                            && args.len() > n
                        {
                            self.err(LowerErrorKind::ConstructArity {
                                expected: n,
                                found: args.len(),
                            });
                        }
                        r
                    }
                    None => {
                        self.err(LowerErrorKind::UnknownRecord(name.to_string()));
                        RecordId(0)
                    }
                };
                let fields = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| FieldInit {
                        field: i,
                        value: self.at(Step::Arg(i), |cx| cx.expr(a)),
                    })
                    .collect();
                Expr::MakeRecord { record, fields }
            }
            other => {
                self.err(LowerErrorKind::BadConstruct {
                    t: type_name(other),
                });
                self.list(args, Step::Arg);
                placeholder()
            }
        }
    }

    fn construct_extern(
        &mut self,
        t: &VariableType,
        args: &[(&'static str, Expression)],
    ) -> Expr {
        let record = match t {
            VariableType::Extern(name) => match self.reg.record_id(name) {
                Some(r) => Some(r),
                None => {
                    self.err(LowerErrorKind::UnknownRecord(name.to_string()));
                    None
                }
            },
            other => {
                self.err(LowerErrorKind::BadConstruct {
                    t: type_name(other),
                });
                None
            }
        };
        let fields = args
            .iter()
            .map(|(name, e)| {
                self.at(Step::Field(name.to_string()), |cx| {
                    let field = match record {
                        Some(r) => cx.reg.field_index(r, name).unwrap_or_else(|| {
                                cx.err(LowerErrorKind::UnknownField {
                                    base: type_name(t),
                                    field: name.to_string(),
                                });
                                0
                            }),
                        None => 0,
                    };
                    FieldInit {
                        field,
                        value: cx.expr(e),
                    }
                })
            })
            .collect();
        match record {
            Some(record) => Expr::MakeRecord { record, fields },
            None => placeholder(),
        }
    }
}
