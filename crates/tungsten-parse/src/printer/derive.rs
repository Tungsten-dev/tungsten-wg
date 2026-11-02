//! The parser's type derivation (SPEC §6.3), replayed on the IR. The printer uses it to leave out
//! a field's base type (`::<T; B>`) when loading the text would derive the same `B`. It must
//! agree with `elaborate::body` node for node; the round-trip tests check that.

use std::cell::RefCell;
use std::collections::HashMap;

use tungsten_ir::*;

use crate::elaborate::types::{binary_type, index_type};
use crate::stack;
use crate::tables::ambient_type;

pub(super) struct Deriver<'a> {
    p: &'a OptimizedProgram,
    /// `None` while in progress, so a cycle derives nothing instead of looping.
    consts: RefCell<HashMap<Ref<Constant>, Option<VariableType>>>,
}

impl<'a> Deriver<'a> {
    pub fn new(p: &'a OptimizedProgram) -> Self {
        Self {
            p,
            consts: RefCell::new(HashMap::new()),
        }
    }

    fn constant(&self, c: Ref<Constant>) -> Option<VariableType> {
        if let Some(t) = self.consts.borrow().get(&c) {
            return t.clone();
        }
        self.consts.borrow_mut().insert(c, None);
        let e = super::expr::const_to_expression(&self.p.constants[c].value);
        let t = self.expr(None, &e);
        self.consts.borrow_mut().insert(c, t.clone());
        t
    }

    fn density_return(&self, d: Ref<DensityInput>) -> Option<VariableType> {
        self.p.compute_units.iter().find_map(|(_, u)| {
            let ComputeUnit::DensityFunction { density_inputs, .. } = u;
            if !density_inputs.contains(d) {
                return None;
            }
            let ComputeUnit::DensityFunction { function, .. } =
                &self.p.compute_units[density_inputs[d].density_function];
            Some(self.p.functions[*function].return_type.clone())
        })
    }

    /// The type the parser derives for `e`. `f` is the enclosing function (`None` in constants).
    pub fn expr(&self, f: Option<Ref<Function>>, e: &Expression) -> Option<VariableType> {
        stack::grow(|| self.expr_inner(f, e))
    }

    fn expr_inner(&self, f: Option<Ref<Function>>, e: &Expression) -> Option<VariableType> {
        use VariableType as T;
        Some(match e {
            Expression::Constant(c) => return self.constant(*c),
            Expression::Variable(v) => self.p.functions[f?].variables[*v].t.clone(),
            Expression::Float(_) => T::F32,
            Expression::Double(_) => T::F64,
            Expression::Byte(_) => T::I8,
            Expression::Int(_) => T::I32,
            Expression::Long(_) => T::I64,
            Expression::Bool(_) => T::Bool,
            Expression::FunctionCall { function, .. } => {
                self.p.functions[*function].return_type.clone()
            }
            Expression::ExternCall { return_type, .. } => return_type.clone(),
            Expression::DensityVariable(d, _) => return self.density_return(*d),
            Expression::PermutationTable(_) => T::PermutationTable,
            Expression::HostInput(h) => match &self.p.host_inputs[*h] {
                HostInput::Static { t, .. } => t.clone(),
                HostInput::Dynamic => return None,
            },
            Expression::Ambient(k) => ambient_type(*k),
            Expression::BinaryOp { op, left, right } => {
                let l = self.expr(f, left);
                let r = self.expr(f, right);
                return binary_type(*op, l.as_ref(), r.as_ref());
            }
            Expression::UnaryOp { operand, .. } => return self.expr(f, operand),
            Expression::Field { type_of_field, .. } => type_of_field.clone(),
            Expression::ArrayAccess { array, .. } => return index_type(&self.expr(f, array)?),
            Expression::Construct { t, .. } | Expression::ConstructExtern { t, .. } => t.clone(),
            Expression::ArrayLiteral(items) => {
                T::Array(Box::new(self.expr(f, items.first()?)?), items.len())
            }
            Expression::ExplicitCast { to, .. } => to.clone(),
        })
    }
}
