/*
Statement conversion from optimized SPMT to RCL.
*/

use super::converter::FunctionConverter;
use crate::model as rcl;
use tungsten_ir::model::{Statement, VariableType};
use std::rc::Rc;

impl<'a> FunctionConverter<'a> {
    /// Converts a list of statements in order.
    pub fn convert_block<'m>(&mut self, stmts: &[Statement]) -> Vec<rcl::Statement<'m>> {
        stmts.iter().map(|s| self.convert_statement(s)).collect()
    }

    /// Converts one statement. Assigned values are cast to the target variable's type when it differs.
    pub fn convert_statement<'m>(&mut self, stmt: &Statement) -> rcl::Statement<'m> {
        match stmt {
            Statement::Assign { target, value } => {
                let var_type: VariableType = self.function.variables[*target].t.clone();
                let target = self.declare_var(*target);
                let value = self.convert_expression_to_type(value, &var_type);
                rcl::Statement::Assign { target, value }
            }
            Statement::Return(e) => rcl::Statement::Return(Some(self.convert_expression(e))),
            Statement::If { condition, then_branch, else_branch } => rcl::Statement::If {
                condition: self.convert_expression(condition),
                then_branch: self.convert_block(then_branch),
                else_branch: (!else_branch.is_empty()).then(|| self.convert_block(else_branch)),
            },
            Statement::While { condition, body } => rcl::Statement::While {
                condition: self.convert_expression(condition),
                body: self.convert_block(body),
            },
            Statement::Repeat { count, body } => {
                // while counter < count { body; counter += 1 }
                let name = self.fresh_name("repeat_counter");
                let counter = Rc::new(rcl::Variable {
                    name: Some(name),
                    t: rcl::Type::I32,
                    mutable: true,
                });
                let var = || Box::new(rcl::Expression::Variable(counter.clone()));
                let mut body = self.convert_block(body);
                body.push(rcl::Statement::Assign {
                    target: counter.clone(),
                    value: rcl::Expression::BinaryOp {
                        op: rcl::BinaryOperator::Add,
                        left: var(),
                        right: Box::new(rcl::Expression::I32Literal(1)),
                    },
                });
                rcl::Statement::Block(vec![
                    rcl::Statement::Declare {
                        variable: counter.clone(),
                        init: Some(rcl::Expression::I32Literal(0)),
                        mutable: true,
                    },
                    rcl::Statement::While {
                        condition: rcl::Expression::BinaryOp {
                            op: rcl::BinaryOperator::Less,
                            left: var(),
                            right: Box::new(rcl::Expression::I32Literal(*count as i32)),
                        },
                        body,
                    },
                ])
            }
            Statement::Break => rcl::Statement::Break,
        }
    }
}
