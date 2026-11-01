/*
Statement conversion from the optimized IR to CUDA C++ (SPEC.md §8). One IR statement becomes one
C++ statement: `Ref` temporaries are materialized inside expressions, so nothing is hoisted.
*/

use tungsten_ir::model::Statement;

use super::converter::FunctionConverter;
use crate::model as cuda;

impl<'a> FunctionConverter<'a> {
    /// Converts a list of statements in order.
    pub fn convert_block(&mut self, stmts: &[Statement]) -> Vec<cuda::Statement> {
        stmts.iter().map(|s| self.convert_statement(s)).collect()
    }

    /// Converts one statement. Values reaching a scalar sink are cast to its type when needed.
    pub fn convert_statement(&mut self, stmt: &Statement) -> cuda::Statement {
        match stmt {
            Statement::Assign { target, value } => {
                let value = self.convert_to_type(value, self.var_type(*target));
                let name = self.declare_var(*target);
                cuda::Statement::Assign { target: cuda::Expression::Name(name), value }
            }
            Statement::Return(e) => {
                let function = self.function;
                let value = self.convert_to_type(e, &function.return_type);
                cuda::Statement::Return(Some(value))
            }
            Statement::If { condition, then_branch, else_branch } => cuda::Statement::If {
                cond: self.convert_expression(condition),
                then: self.convert_block(then_branch),
                other: self.convert_block(else_branch),
            },
            Statement::While { condition, body } => cuda::Statement::While {
                cond: self.convert_expression(condition),
                body: self.convert_block(body),
            },
            Statement::Repeat { count, body } => {
                let var = self.fresh_name("repeat_counter");
                let end = cuda::Expression::IntLit(i32::try_from(*count).unwrap_or_else(|_| {
                    panic!("{}: repeat count {} exceeds int", self.function_name, count)
                }));
                cuda::Statement::ForRange { var, end, body: self.convert_block(body) }
            }
            Statement::Break => cuda::Statement::Break,
        }
    }
}
