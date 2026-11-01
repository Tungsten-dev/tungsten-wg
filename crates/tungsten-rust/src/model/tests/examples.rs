/*
Example: Using ICL to create a simple low-level CPU function and generate Rust code
*/

#[cfg(test)]
mod examples {
    use crate::{
        rcl::{codegen, model::*},
        spmt::model::Interned,
    };
    use std::rc::Rc;

    #[test]
    fn example_create_simple_function() {
        // Create a simple function: add(a: i32, b: i32) -> i32
        let mut add_func = Function::new("add".to_string(), Type::I32);
        add_func.add_parameter("a".to_string(), Type::I32);
        add_func.add_parameter("b".to_string(), Type::I32);

        // Create variables for the function
        let result_var = Rc::new(Variable {
            name: Some("result".to_string()),
            t: Type::I32,
            mutable: true,
        });

        add_func.add_variable(result_var.clone());

        // Create the expression: a + b
        let a_var = Rc::new(Variable {
            name: Some("a".to_string()),
            t: Type::I32,
            mutable: false,
        });
        let b_var = Rc::new(Variable {
            name: Some("b".to_string()),
            t: Type::I32,
            mutable: false,
        });

        let add_expr = Expression::BinaryOp {
            op: BinaryOperator::Add,
            left: Box::new(Expression::Variable(a_var)),
            right: Box::new(Expression::Variable(b_var)),
        };

        // Add assignment: result = a + b
        add_func.add_statement(Statement::Assign {
            target: result_var.clone(),
            value: add_expr,
        });

        // Add return statement
        add_func.add_statement(Statement::Return(Some(Expression::Variable(result_var))));

        // Generate Rust code
        let rust_code = codegen::generate_rust_function(&Interned::new(&add_func));
        println!("Generated Rust code:\n{}", rust_code);

        // Verify the generated code contains expected elements
        assert!(rust_code.contains("pub fn add"));
        assert!(rust_code.contains("a: i32"));
        assert!(rust_code.contains("b: i32"));
        assert!(rust_code.contains("i32"));
    }

    #[test]
    fn example_create_function_with_control_flow() {
        // Create a function with if/else: max(a: i32, b: i32) -> i32
        let mut max_func = Function::new("max".to_string(), Type::I32);
        max_func.add_parameter("a".to_string(), Type::I32);
        max_func.add_parameter("b".to_string(), Type::I32);

        let a_var = Rc::new(Variable {
            name: Some("a".to_string()),
            t: Type::I32,
            mutable: false,
        });
        let b_var = Rc::new(Variable {
            name: Some("b".to_string()),
            t: Type::I32,
            mutable: false,
        });

        // Create condition: a > b
        let condition = Expression::BinaryOp {
            op: BinaryOperator::Greater,
            left: Box::new(Expression::Variable(a_var.clone())),
            right: Box::new(Expression::Variable(b_var.clone())),
        };

        // Then branch: return a
        let then_branch = vec![Statement::Return(Some(Expression::Variable(a_var.clone())))];

        // Else branch: return b
        let else_branch = Some(vec![Statement::Return(Some(Expression::Variable(
            b_var.clone(),
        )))]);

        // Add if statement
        max_func.add_statement(Statement::If {
            condition,
            then_branch,
            else_branch,
        });

        // Generate Rust code
        let rust_code = codegen::generate_rust_function(&Interned::new(&max_func));
        println!("Generated max function:\n{}", rust_code);

        assert!(rust_code.contains("pub fn max"));
        assert!(rust_code.contains("if"));
        assert!(rust_code.contains("a > b"));
    }

    #[test]
    fn example_create_struct_and_use() {
        // Create a Vec3 struct
        let mut vec3_struct = Struct::new("Vec3".to_string());
        vec3_struct.add_field("x".to_string(), Type::F32);
        vec3_struct.add_field("y".to_string(), Type::F32);
        vec3_struct.add_field("z".to_string(), Type::F32);

        // Create a function that uses the struct
        let mut length_func = Function::new("vec3_length".to_string(), Type::F32);
        length_func.add_parameter("v".to_string(), Type::Struct("Vec3".to_string()));

        // Generate Rust code for the struct
        let mut rcg = codegen::RustCodeGenerator::new();
        let rust_code = rcg.generate_struct(&Interned::new(&vec3_struct));
        println!("Generated struct:\n{}", rust_code);

        assert!(rust_code.contains("pub struct Vec3"));
        assert!(rust_code.contains("pub x: f32"));
    }

    #[test]
    fn example_spmt_to_icl_conversion() {
        // This would require SPMT context to test properly
        // For now, just show the conversion API exists
        use crate::rcl::convert;

        // Simple test to verify the converter compiles
        let _converter = convert::SPMTToICLConverter::new();
    }
}
