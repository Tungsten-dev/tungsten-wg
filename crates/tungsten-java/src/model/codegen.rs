/*
Pretty-printer for the Java model. Every compound sub-expression is parenthesized, so operator
precedence never matters (`a - -b`, casts inside arithmetic, ...). Literals are printed so javac
reads back exactly the same value.
*/

use super::model::*;

const INDENT: &str = "    ";

#[derive(Default)]
pub struct JavaCodeGenerator {
    out: String,
    depth: usize,
}

/// Shortest round-trip decimal of a float, made a valid Java literal (always has `.` or `e`).
fn decimal(debug: String) -> String {
    if debug.contains('.') || debug.contains('e') || debug.contains('E') {
        debug
    } else {
        debug + ".0"
    }
}

/// `double` literal; specials become `Double.*`, negatives are parenthesized.
pub fn double_literal(v: f64) -> String {
    if v.is_nan() {
        "Double.NaN".to_string()
    } else if v == f64::INFINITY {
        "Double.POSITIVE_INFINITY".to_string()
    } else if v == f64::NEG_INFINITY {
        "Double.NEGATIVE_INFINITY".to_string()
    } else if v.is_sign_negative() {
        format!("({})", decimal(format!("{:?}", v)))
    } else {
        decimal(format!("{:?}", v))
    }
}

/// `float` literal with an `f` suffix; specials become `Float.*`, negatives are parenthesized.
pub fn float_literal(v: f32) -> String {
    if v.is_nan() {
        "Float.NaN".to_string()
    } else if v == f32::INFINITY {
        "Float.POSITIVE_INFINITY".to_string()
    } else if v == f32::NEG_INFINITY {
        "Float.NEGATIVE_INFINITY".to_string()
    } else if v.is_sign_negative() {
        format!("({}f)", decimal(format!("{:?}", v)))
    } else {
        format!("{}f", decimal(format!("{:?}", v)))
    }
}

pub fn int_literal(v: i32) -> String {
    match v {
        i32::MIN => "Integer.MIN_VALUE".to_string(),
        v if v < 0 => format!("({})", v),
        v => v.to_string(),
    }
}

pub fn long_literal(v: i64) -> String {
    match v {
        i64::MIN => "Long.MIN_VALUE".to_string(),
        v if v < 0 => format!("({}L)", v),
        v => format!("{}L", v),
    }
}

pub fn type_string(t: &Type) -> String {
    match t {
        Type::Boolean => "boolean".into(),
        Type::Byte => "byte".into(),
        Type::Int => "int".into(),
        Type::Long => "long".into(),
        Type::Float => "float".into(),
        Type::Double => "double".into(),
        Type::Void => "void".into(),
        Type::Class(n) => n.clone(),
        Type::Array(e) => format!("{}[]", type_string(e)),
    }
}

fn binary_op(op: BinaryOperator) -> &'static str {
    use BinaryOperator as B;
    match op {
        B::Add => "+",
        B::Subtract => "-",
        B::Multiply => "*",
        B::Divide => "/",
        B::Equal => "==",
        B::NotEqual => "!=",
        B::Less => "<",
        B::LessEqual => "<=",
        B::Greater => ">",
        B::GreaterEqual => ">=",
        B::And => "&&",
        B::Or => "||",
        B::BitAnd => "&",
        B::BitOr => "|",
        B::BitXor => "^",
        B::ShiftLeft => "<<",
        B::ShiftRight => ">>",
        B::UnsignedShiftRight => ">>>",
    }
}

/// `new T[]{..}` needs the element's base type and the extra dimensions after the brackets.
fn new_array_type(element: &Type) -> String {
    format!("{}[]", type_string(element))
}

pub fn expression(e: &Expression) -> String {
    match e {
        Expression::BoolLit(b) => b.to_string(),
        Expression::IntLit(v) => int_literal(*v),
        Expression::LongLit(v) => long_literal(*v),
        Expression::FloatLit(v) => float_literal(*v),
        Expression::DoubleLit(v) => double_literal(*v),
        Expression::Null => "null".into(),
        Expression::Name(n) => n.clone(),
        Expression::Field { base, field } => format!("{}.{}", expression(base), field),
        Expression::Index { base, index } => format!("{}[{}]", expression(base), bare(index)),
        Expression::Call { target, name, args } => {
            let args = args.iter().map(bare).collect::<Vec<_>>().join(", ");
            match target {
                Some(t) => format!("{}.{}({})", expression(t), name, args),
                None => format!("{}({})", name, args),
            }
        }
        Expression::New { class, args, comment } => {
            let args = args.iter().map(bare).collect::<Vec<_>>().join(", ");
            match comment {
                Some(c) => format!("new {}({}) /* {} */", class, args, c),
                None => format!("new {}({})", class, args),
            }
        }
        Expression::NewArray { element, items } => {
            let items = items.iter().map(bare).collect::<Vec<_>>().join(", ");
            format!("new {}{{{}}}", new_array_type(element), items)
        }
        Expression::NewArraySized { element, len } => {
            // `new double[64]`, `new double[64][]` for nested arrays.
            let mut base = element;
            let mut dims = String::new();
            while let Type::Array(inner) = base {
                dims.push_str("[]");
                base = inner;
            }
            format!("new {}[{}]{}", type_string(base), len, dims)
        }
        Expression::Binary { op, left, right } => {
            format!("({} {} {})", expression(left), binary_op(*op), expression(right))
        }
        Expression::Unary { op, operand } => {
            let op = match op {
                UnaryOperator::Negate => "-",
                UnaryOperator::Not => "!",
                UnaryOperator::BitNot => "~",
            };
            format!("({}{})", op, expression(operand))
        }
        Expression::Cast { to, expr } => format!("(({}) {})", type_string(to), expression(expr)),
        Expression::Conditional { cond, then, other } => {
            format!("({} ? {} : {})", expression(cond), expression(then), expression(other))
        }
    }
}

/// An expression in a position that needs no parentheses (statement level, arguments, indices).
pub fn bare(e: &Expression) -> String {
    let s = expression(e);
    let compound = matches!(
        e,
        Expression::Binary { .. }
            | Expression::Unary { .. }
            | Expression::Cast { .. }
            | Expression::Conditional { .. }
    );
    if compound { s[1..s.len() - 1].to_string() } else { s }
}

impl JavaCodeGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    fn line(&mut self, s: &str) {
        if s.is_empty() {
            self.out.push('\n');
            return;
        }
        for _ in 0..self.depth {
            self.out.push_str(INDENT);
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn doc(&mut self, doc: &Option<String>) {
        if let Some(d) = doc {
            let lines: Vec<&str> = d.lines().collect();
            if lines.len() == 1 {
                self.line(&format!("/** {} */", lines[0]));
            } else {
                self.line("/**");
                for l in lines {
                    self.line(&format!(" * {}", l).trim_end().to_string());
                }
                self.line(" */");
            }
        }
    }

    fn block(&mut self, body: &[Statement]) {
        self.depth += 1;
        for s in body {
            self.statement(s);
        }
        self.depth -= 1;
    }

    fn statement(&mut self, s: &Statement) {
        match s {
            Statement::Declare { t, name, init, is_final } => {
                let fin = if *is_final { "final " } else { "" };
                match init {
                    Some(i) => self.line(&format!("{}{} {} = {};", fin, type_string(t), name, bare(i))),
                    None => self.line(&format!("{}{} {};", fin, type_string(t), name)),
                }
            }
            Statement::Assign { target, value } => {
                self.line(&format!("{} = {};", expression(target), bare(value)))
            }
            Statement::Expr(e) => self.line(&format!("{};", bare(e))),
            Statement::Return(Some(e)) => self.line(&format!("return {};", bare(e))),
            Statement::Return(None) => self.line("return;"),
            Statement::If { cond, then, other } => {
                self.line(&format!("if ({}) {{", bare(cond)));
                self.block(then);
                if other.is_empty() {
                    self.line("}");
                } else {
                    self.line("} else {");
                    self.block(other);
                    self.line("}");
                }
            }
            Statement::While { cond, body } => {
                self.line(&format!("while ({}) {{", bare(cond)));
                self.block(body);
                self.line("}");
            }
            Statement::ForRange { var, end, body } => {
                self.line(&format!("for (int {v} = 0; {v} < {}; {v}++) {{", bare(end), v = var));
                self.block(body);
                self.line("}");
            }
            Statement::Break => self.line("break;"),
            Statement::Block(body) => {
                self.line("{");
                self.block(body);
                self.line("}");
            }
            Statement::Comment(c) => {
                for l in c.lines() {
                    self.line(&format!("// {}", l));
                }
            }
        }
    }

    fn field(&mut self, f: &Field) {
        let mods = if f.modifiers.is_empty() { String::new() } else { format!("{} ", f.modifiers) };
        match &f.init {
            Some(i) => self.line(&format!("{}{} {} = {};", mods, type_string(&f.t), f.name, bare(i))),
            None => self.line(&format!("{}{} {};", mods, type_string(&f.t), f.name)),
        }
    }

    fn method(&mut self, m: &Method) {
        self.doc(&m.doc);
        let mods = if m.modifiers.is_empty() { String::new() } else { format!("{} ", m.modifiers) };
        let ret = m.return_type.as_ref().map(|t| format!("{} ", type_string(t))).unwrap_or_default();
        let params = m
            .params
            .iter()
            .map(|(t, n)| format!("{} {}", type_string(t), n))
            .collect::<Vec<_>>()
            .join(", ");
        self.line(&format!("{}{}{}({}) {{", mods, ret, m.name, params));
        self.block(&m.body);
        self.line("}");
    }

    fn class(&mut self, c: &Class) {
        self.doc(&c.doc);
        let mods = if c.modifiers.is_empty() { String::new() } else { format!("{} ", c.modifiers) };
        self.line(&format!("{}class {} {{", mods, c.name));
        self.depth += 1;
        for f in &c.fields {
            self.field(f);
        }
        for n in &c.nested {
            self.line("");
            self.class(n);
        }
        for m in &c.methods {
            self.line("");
            self.method(m);
        }
        self.depth -= 1;
        self.line("}");
    }

    /// Renders one compilation unit as Java source.
    pub fn generate(mut self, unit: &CompilationUnit) -> String {
        for h in &unit.header {
            self.line(&format!("// {}", h));
        }
        if let Some(p) = &unit.package {
            self.line(&format!("package {};", p));
            self.line("");
        }
        for i in &unit.imports {
            self.line(&format!("import {};", i));
        }
        if !unit.imports.is_empty() {
            self.line("");
        }
        self.class(&unit.class);
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_literals_round_trip() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "(-0.0)"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (-1.5, "(-1.5)"),
            (1e-7, "1e-7"),
            (1e20, "1e20"),
            (f64::MAX, "1.7976931348623157e308"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (5e-324, "5e-324"),
            (f64::NAN, "Double.NaN"),
            (f64::INFINITY, "Double.POSITIVE_INFINITY"),
            (f64::NEG_INFINITY, "Double.NEGATIVE_INFINITY"),
        ];
        for (v, expected) in cases {
            assert_eq!(double_literal(*v), *expected, "for {:?}", v);
        }
        // The printed decimal parses back to the same bits (Java's parser is correctly rounded too).
        for v in [0.1, 1e-7, 1e20, f64::MAX, f64::MIN_POSITIVE, 5e-324, 123.456, 1.0 / 3.0] {
            assert_eq!(double_literal(v).parse::<f64>().unwrap().to_bits(), v.to_bits());
        }
    }

    #[test]
    fn float_literals_round_trip() {
        let cases: &[(f32, &str)] = &[
            (0.0, "0.0f"),
            (-0.0, "(-0.0f)"),
            (1.0, "1.0f"),
            (0.1, "0.1f"),
            (1e-7, "1e-7f"),
            (1e20, "1e20f"),
            (f32::MAX, "3.4028235e38f"),
            (f32::MIN_POSITIVE, "1.1754944e-38f"),
            (1e-45, "1e-45f"),
            (f32::NAN, "Float.NaN"),
            (f32::INFINITY, "Float.POSITIVE_INFINITY"),
            (f32::NEG_INFINITY, "Float.NEGATIVE_INFINITY"),
        ];
        for (v, expected) in cases {
            assert_eq!(float_literal(*v), *expected, "for {:?}", v);
        }
        for v in [0.1f32, 1e-7, 1e20, f32::MAX, f32::MIN_POSITIVE, 1e-45, 1.0 / 3.0] {
            let s = float_literal(v);
            assert_eq!(s.trim_end_matches('f').parse::<f32>().unwrap().to_bits(), v.to_bits());
        }
    }

    #[test]
    fn integer_literals() {
        assert_eq!(int_literal(5), "5");
        assert_eq!(int_literal(-5), "(-5)");
        assert_eq!(int_literal(i32::MIN), "Integer.MIN_VALUE");
        assert_eq!(long_literal(5), "5L");
        assert_eq!(long_literal(-5), "(-5L)");
        assert_eq!(long_literal(i64::MIN), "Long.MIN_VALUE");
    }

    #[test]
    fn nested_operators_are_parenthesized() {
        use Expression as E;
        let neg = E::Unary { op: UnaryOperator::Negate, operand: Box::new(E::name("b")) };
        let e = E::binary(BinaryOperator::Subtract, E::name("a"), neg);
        assert_eq!(bare(&e), "a - (-b)");
        let e = E::binary(BinaryOperator::Subtract, E::name("a"), E::DoubleLit(-1.0));
        assert_eq!(bare(&e), "a - (-1.0)");
        let e = E::cast(Type::Int, E::binary(BinaryOperator::Add, E::name("a"), E::name("b")));
        assert_eq!(bare(&e), "(int) (a + b)");
    }
}
