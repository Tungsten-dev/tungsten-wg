/*
Conservative bytecode size estimates for generated methods. The JVM rejects methods over 64KB
(javac enforces it, including for the static initializer), and HotSpot never JIT-compiles methods
over 8000 bytes (`HugeMethodLimit`) unless run with -XX:-DontCompileHugeMethods. The per-node
costs are rough upper-ish figures, good enough to warn before javac or the JIT does.
*/

use crate::model::{BinaryOperator, Class, Expression, Field, Method, Statement, UnaryOperator};

/// HotSpot's `HugeMethodLimit`.
pub const HUGE_METHOD_LIMIT: usize = 8000;
/// A safety margin under the 65535-byte method limit.
pub const METHOD_LIMIT: usize = 60000;

fn expression(e: &Expression) -> usize {
    match e {
        Expression::BoolLit(_) | Expression::Null => 1,
        Expression::IntLit(_) => 2,
        Expression::LongLit(_) | Expression::FloatLit(_) | Expression::DoubleLit(_) => 3,
        Expression::Name(_) => 1,
        Expression::Field { base, .. } => expression(base) + 3,
        Expression::Index { base, index } => expression(base) + expression(index) + 1,
        Expression::Call { target, args, .. } => {
            target.as_deref().map_or(0, expression) + args.iter().map(expression).sum::<usize>() + 3
        }
        Expression::New { args, .. } => 7 + args.iter().map(expression).sum::<usize>(),
        // Each element: dup, index, value, store.
        Expression::NewArray { items, .. } => 4 + items.iter().map(|i| expression(i) + 4).sum::<usize>(),
        Expression::NewArraySized { .. } => 5,
        Expression::Binary { op, left, right } => {
            // Comparisons and logic materialize a boolean with branches.
            let extra = match op {
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
                | BinaryOperator::And
                | BinaryOperator::Or => 7,
                _ => 1,
            };
            expression(left) + expression(right) + extra
        }
        Expression::Unary { op, operand } => {
            expression(operand)
                + match op {
                    UnaryOperator::Negate => 1,
                    UnaryOperator::Not => 7,
                    UnaryOperator::BitNot => 2,
                }
        }
        Expression::Cast { expr, .. } => expression(expr) + 1,
        Expression::Conditional { cond, then, other } => {
            expression(cond) + expression(then) + expression(other) + 6
        }
    }
}

fn store_target(e: &Expression) -> usize {
    match e {
        Expression::Name(_) => 2,
        Expression::Field { base, .. } => expression(base) + 3,
        Expression::Index { base, index } => expression(base) + expression(index) + 1,
        other => expression(other),
    }
}

fn block(stmts: &[Statement]) -> usize {
    stmts.iter().map(statement).sum()
}

fn statement(s: &Statement) -> usize {
    match s {
        Statement::Declare { init, .. } => init.as_ref().map_or(0, |i| expression(i) + 2),
        Statement::Assign { target, value } => store_target(target) + expression(value),
        Statement::Expr(e) => expression(e) + 1,
        Statement::Return(e) => e.as_ref().map_or(0, expression) + 1,
        Statement::If { cond, then, other } => {
            expression(cond) + 3 + block(then) + if other.is_empty() { 0 } else { 3 + block(other) }
        }
        Statement::While { cond, body } => expression(cond) + 6 + block(body),
        Statement::ForRange { end, body, .. } => expression(end) + 11 + block(body),
        Statement::Break => 3,
        Statement::Block(body) => block(body),
        Statement::Comment(_) => 0,
    }
}

pub fn estimate_method(m: &Method) -> usize {
    block(&m.body) + 1
}

/// The static initializer assigns every field with an initializer (`putstatic` each).
pub fn estimate_static_init(fields: &[Field]) -> usize {
    fields.iter().filter_map(|f| f.init.as_ref()).map(|i| expression(i) + 3).sum::<usize>() + 1
}

fn check(what: &str, size: usize, warnings: &mut Vec<String>) {
    // TODO(auto-split): instead of only warning, outline large statement sub-blocks of an oversized
    // method into `private static` methods. Live-in variables become parameters; live-out variables
    // are returned (a holder object or small result class when there is more than one). Re-run the
    // estimate afterwards. The pass belongs here: after the Java AST is built, before printing.
    if size > METHOD_LIMIT {
        warnings.push(format!(
            "java: {} estimated at {} bytes likely exceeds the 64KB method limit; javac will reject it",
            what, size
        ));
    } else if size > HUGE_METHOD_LIMIT {
        warnings.push(format!(
            "java: {} estimated at {} bytes exceeds HotSpot HugeMethodLimit (8000); it will not be JIT-compiled unless the JVM runs with -XX:-DontCompileHugeMethods",
            what, size
        ));
    }
}

/// Size warnings for every method and static initializer in `class` and its nested classes.
pub fn check_class(class: &Class, warnings: &mut Vec<String>) {
    for m in &class.methods {
        check(&format!("method {}.{}", class.name, m.name), estimate_method(m), warnings);
    }
    let statics: Vec<Field> = class.fields.iter().filter(|f| f.modifiers.contains("static")).cloned().collect();
    check(&format!("static initializer of {}", class.name), estimate_static_init(&statics), warnings);
    for n in &class.nested {
        check_class(n, warnings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Type;

    fn method_with(statements: usize) -> Method {
        let s = Statement::Assign {
            target: Expression::name("a"),
            value: Expression::binary(BinaryOperator::Add, Expression::name("a"), Expression::DoubleLit(1.5)),
        };
        Method {
            doc: None,
            modifiers: "static".into(),
            return_type: Some(Type::Void),
            name: "m".into(),
            params: vec![],
            body: vec![s; statements],
        }
    }

    #[test]
    fn warns_on_huge_and_oversized_methods() {
        let class = |m: Method| Class { name: "C".into(), methods: vec![m], ..Default::default() };
        let mut w = Vec::new();
        check_class(&class(method_with(10)), &mut w);
        assert!(w.is_empty());
        check_class(&class(method_with(2000)), &mut w);
        assert!(w.len() == 1 && w[0].contains("HugeMethodLimit"), "{:?}", w);
        w.clear();
        check_class(&class(method_with(20000)), &mut w);
        assert!(w.len() == 1 && w[0].contains("64KB"), "{:?}", w);
    }
}
