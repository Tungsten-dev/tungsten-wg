/*
Statement conversion from optimized SPMT to Java. One SPMT statement can become several Java
statements: holders for `Ref(true)` on primitives are declared before it and written back after it.
*/

use super::converter::FunctionConverter;
use crate::model as java;
use tungsten_ir::model::VariableType;
use tungsten_ir::model::Statement;

impl<'a> FunctionConverter<'a> {
    /// Converts a list of statements in order.
    pub fn convert_block(&mut self, stmts: &[Statement]) -> Vec<java::Statement> {
        stmts.iter().flat_map(|s| self.convert_statement(s)).collect()
    }

    /// Converts one statement. Values reaching a primitive sink are cast to its type when needed.
    pub fn convert_statement(&mut self, stmt: &Statement) -> Vec<java::Statement> {
        match stmt {
            Statement::Assign { target, value } => {
                let var_type: VariableType = self.var_type(*target).clone();
                let value = self.convert_to_type(value, &var_type);
                let (mut out, post, written_back) = self.take_hoisted();
                if written_back.contains(target) {
                    panic!("Ref(true) on the variable that the same statement assigns is ambiguous");
                }
                let name = self.declare_var(*target);
                out.push(java::Statement::Assign { target: java::Expression::Name(name), value });
                out.extend(post);
                out
            }
            Statement::Return(e) => {
                let return_type = self.function.return_type.clone();
                let value = self.convert_to_type(e, &return_type);
                // Locals die with the return, so write-backs are dropped.
                let (mut out, _, _) = self.take_hoisted();
                out.push(java::Statement::Return(Some(value)));
                out
            }
            Statement::If { condition, then_branch, else_branch } => {
                let cond = self.convert_expression(condition);
                let (mut out, post, _) = self.take_hoisted();
                // The condition's write-backs run first in whichever branch is taken.
                let mut then = post.clone();
                then.extend(self.convert_block(then_branch));
                let mut other = post;
                other.extend(self.convert_block(else_branch));
                out.push(java::Statement::If { cond, then, other });
                out
            }
            Statement::While { condition, body } => {
                let cond = self.convert_expression(condition);
                let (pre, post, _) = self.take_hoisted();
                if !pre.is_empty() || !post.is_empty() {
                    panic!("Ref(true) in while condition unsupported");
                }
                vec![java::Statement::While { cond, body: self.convert_block(body) }]
            }
            Statement::Repeat { count, body } => {
                let var = self.fresh_name("repeat_counter");
                let end = java::Expression::IntLit(i32::try_from(*count).expect("repeat count exceeds i32"));
                vec![java::Statement::ForRange { var, end, body: self.convert_block(body) }]
            }
            Statement::Break => vec![java::Statement::Break],
        }
    }
}
