/*
Pretty-printer for the CUDA model, on top of `tungsten_codegen::Printer`. Expressions are
parenthesized by C++ precedence. Two exceptions add parentheses for clarity: negative literals
used as operands (`a - (-1)`), and bitwise, shift or logical operators whose operand is a
different binary operator (`(a & b) | c`), which is what `-Wparentheses` asks for. Literals are
printed so the compiler reads back exactly the same value.
*/

use tungsten_codegen::Printer;

use super::model::*;

/// Shortest round-trip decimal of a float, made a valid C++ literal (always has `.` or `e`).
fn decimal(debug: String) -> String {
    if debug.contains('.') || debug.contains('e') || debug.contains('E') {
        debug
    } else {
        debug + ".0"
    }
}

/// `double` literal; specials become the runtime's `TW_*_F64` macros.
pub fn double_literal(v: f64) -> String {
    if v.is_nan() {
        "TW_NAN_F64".to_string()
    } else if v == f64::INFINITY {
        "TW_INF_F64".to_string()
    } else if v == f64::NEG_INFINITY {
        "(-TW_INF_F64)".to_string()
    } else {
        decimal(format!("{:?}", v))
    }
}

/// `float` literal with an `f` suffix; specials become the runtime's `TW_*_F32` macros.
pub fn float_literal(v: f32) -> String {
    if v.is_nan() {
        "TW_NAN_F32".to_string()
    } else if v == f32::INFINITY {
        "TW_INF_F32".to_string()
    } else if v == f32::NEG_INFINITY {
        "(-TW_INF_F32)".to_string()
    } else {
        format!("{}f", decimal(format!("{:?}", v)))
    }
}

pub fn int_literal(v: i32) -> String {
    match v {
        i32::MIN => "(-2147483647 - 1)".to_string(),
        v => v.to_string(),
    }
}

pub fn long_literal(v: i64) -> String {
    match v {
        i64::MIN => "(-9223372036854775807LL - 1)".to_string(),
        v => format!("{}LL", v),
    }
}

pub fn type_string(t: &Type) -> String {
    match t {
        Type::Bool => "bool".into(),
        Type::Int => "int".into(),
        Type::SignedChar => "signed char".into(),
        Type::LongLong => "long long".into(),
        Type::Unsigned => "unsigned".into(),
        Type::UnsignedLongLong => "unsigned long long".into(),
        Type::Float => "float".into(),
        Type::Double => "double".into(),
        Type::Char => "char".into(),
        Type::Void => "void".into(),
        Type::Named(n) => n.clone(),
        Type::Ptr { inner, is_const, restrict } => format!(
            "{}{}*{}",
            if *is_const { "const " } else { "" },
            type_string(inner),
            if *restrict { " __restrict__" } else { "" }
        ),
        Type::ConstRef(inner) => format!("const {}&", type_string(inner)),
        Type::Array(inner, n) => format!("tw::Array<{}, {}>", type_string(inner), n),
        Type::CArray(..) => panic!("a C array type is only valid in a declarator"),
    }
}

/// The element type of a (nested) C array and its dimensions, outermost first (`[3][2]`).
pub fn split_c_array(t: &Type) -> (&Type, String) {
    let mut dims = String::new();
    let mut t = t;
    while let Type::CArray(inner, n) = t {
        dims.push_str(&format!("[{}]", n));
        t = inner;
    }
    (t, dims)
}

/// `T name`, or `T name[N]...` for a C array.
pub fn declarator(t: &Type, name: &str) -> String {
    let (base, dims) = split_c_array(t);
    format!("{} {}{}", type_string(base), name, dims)
}

fn binary_symbol(op: BinaryOperator) -> &'static str {
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
    }
}

fn binary_precedence(op: BinaryOperator) -> u8 {
    use BinaryOperator as B;
    match op {
        B::Multiply | B::Divide => 13,
        B::Add | B::Subtract => 12,
        B::ShiftLeft | B::ShiftRight => 11,
        B::Less | B::LessEqual | B::Greater | B::GreaterEqual => 9,
        B::Equal | B::NotEqual => 8,
        B::BitAnd => 7,
        B::BitXor => 6,
        B::BitOr => 5,
        B::And => 4,
        B::Or => 3,
    }
}

/// Operators whose operands get parentheses when they are a different binary operator.
fn wants_clarity(op: BinaryOperator) -> bool {
    use BinaryOperator as B;
    matches!(op, B::ShiftLeft | B::ShiftRight | B::BitAnd | B::BitOr | B::BitXor | B::And | B::Or)
}

const PRIMARY: u8 = 16;
const UNARY: u8 = 15;
/// Negative literals: parenthesized wherever they are an operand.
const NEGATIVE_LITERAL: u8 = 1;

fn precedence(e: &Expression) -> u8 {
    use Expression as E;
    match e {
        E::IntLit(v) if *v < 0 && *v != i32::MIN => NEGATIVE_LITERAL,
        E::LongLit(v) if *v < 0 && *v != i64::MIN => NEGATIVE_LITERAL,
        E::FloatLit(v) if v.is_sign_negative() && v.is_finite() => NEGATIVE_LITERAL,
        E::DoubleLit(v) if v.is_sign_negative() && v.is_finite() => NEGATIVE_LITERAL,
        E::Binary { op, .. } => binary_precedence(*op),
        E::Unary { .. } | E::Cast { .. } => UNARY,
        _ => PRIMARY,
    }
}

/// `e` printed, parenthesized if it binds looser than `min`.
fn operand(e: &Expression, min: u8) -> String {
    let s = expression(e);
    if precedence(e) < min { format!("({})", s) } else { s }
}

fn list(items: &[Expression]) -> String {
    items.iter().map(expression).collect::<Vec<_>>().join(", ")
}

pub fn expression(e: &Expression) -> String {
    use Expression as E;
    match e {
        E::BoolLit(v) => v.to_string(),
        E::IntLit(v) => int_literal(*v),
        E::LongLit(v) => long_literal(*v),
        E::FloatLit(v) => float_literal(*v),
        E::DoubleLit(v) => double_literal(*v),
        E::Raw(s) | E::Name(s) => s.clone(),
        E::Field { base, field, arrow } => {
            format!("{}{}{}", operand(base, PRIMARY), if *arrow { "->" } else { "." }, field)
        }
        E::Index { base, index } => format!("{}[{}]", operand(base, PRIMARY), expression(index)),
        E::Call { callee, template_args, args } => {
            let template = if template_args.is_empty() {
                String::new()
            } else {
                format!("<{}>", template_args.iter().map(type_string).collect::<Vec<_>>().join(", "))
            };
            format!("{}{}({})", callee, template, list(args))
        }
        E::Binary { op, left, right } => {
            let p = binary_precedence(*op);
            let side = |e: &Expression, min: u8| match e {
                E::Binary { op: inner, .. } if wants_clarity(*op) && inner != op => {
                    format!("({})", expression(e))
                }
                _ => operand(e, min),
            };
            format!("{} {} {}", side(left, p), binary_symbol(*op), side(right, p + 1))
        }
        E::Unary { op, operand: inner } => {
            let symbol = match op {
                UnaryOperator::Negate => "-",
                UnaryOperator::Not => "!",
                UnaryOperator::BitNot => "~",
                UnaryOperator::AddressOf => "&",
            };
            format!("{}{}", symbol, operand(inner, UNARY))
        }
        E::Cast { to, expr } => format!("({}){}", type_string(to), operand(expr, UNARY)),
        E::BraceInit { t, items } => format!("{}{{{}}}", type_string(t), list(items)),
        E::Braces(items) => format!("{{{}}}", list(items)),
        E::DesignatedInit { t, fields } => format!(
            "{}{{{}}}",
            type_string(t),
            fields
                .iter()
                .map(|(n, v)| format!(".{} = {}", n, expression(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Prints files of the CUDA model.
pub struct CudaCodeGenerator {
    p: Printer,
}

impl Default for CudaCodeGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl CudaCodeGenerator {
    pub fn new() -> Self {
        Self { p: Printer::new() }
    }

    pub fn generate(mut self, file: &File) -> String {
        for item in &file.items {
            self.item(item);
        }
        self.p.finish()
    }

    fn lines(&mut self, prefix: &str, text: &str) {
        for l in text.lines() {
            if l.is_empty() && prefix.is_empty() {
                self.p.line("");
            } else {
                self.p.line(&format!("{}{}", prefix, l).trim_end().to_string());
            }
        }
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Raw(s) => self.lines("", s),
            Item::Comment(s) => self.lines("// ", s),
            Item::Blank => self.p.line(""),
            Item::Function(f) => self.function(f),
            Item::Variable { qualifiers, t, name, init } => {
                let q = if qualifiers.is_empty() { String::new() } else { format!("{} ", qualifiers) };
                self.p.line(&format!("{}{} = {};", q, declarator(t, name), expression(init)));
            }
            Item::Field { t, name } => self.p.line(&format!("{};", declarator(t, name))),
            Item::Struct(s) => {
                if let Some(doc) = &s.doc {
                    self.lines("/// ", doc);
                }
                self.p.line(&format!("struct {} {{", s.name));
                self.p.indent();
                for m in &s.members {
                    self.item(m);
                }
                self.p.dedent();
                self.p.line("};");
            }
            Item::Namespace { name, items } => {
                self.p.line(&format!("namespace {} {{", name));
                self.p.line("");
                for i in items {
                    self.item(i);
                }
                self.p.line("");
                self.p.line(&format!("}} // namespace {}", name));
            }
        }
    }

    fn function(&mut self, f: &Function) {
        if let Some(doc) = &f.doc {
            self.lines("/// ", doc);
        }
        let params = f.params.iter().map(|(t, n)| declarator(t, n)).collect::<Vec<_>>().join(", ");
        let head = [f.qualifiers.as_str(), &type_string(&f.return_type), f.attributes.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let signature = format!("{} {}({})", head, f.name, params);
        match &f.body {
            None => self.p.line(&format!("{};", signature)),
            Some(body) => {
                self.p.line(&format!("{} {{", signature));
                self.block(body);
                self.p.line("}");
            }
        }
    }

    fn block(&mut self, body: &[Statement]) {
        self.p.indent();
        for s in body {
            self.statement(s);
        }
        self.p.dedent();
    }

    fn statement(&mut self, s: &Statement) {
        use Statement as S;
        match s {
            S::Declare { t, name, init, is_const } => {
                let c = if *is_const { "const " } else { "" };
                match init {
                    Some(init) => self.p.line(&format!("{}{} = {};", c, declarator(t, name), expression(init))),
                    None => self.p.line(&format!("{}{}{{}};", c, declarator(t, name))),
                }
            }
            S::Assign { target, value } => {
                self.p.line(&format!("{} = {};", expression(target), expression(value)))
            }
            S::Expr(e) => self.p.line(&format!("{};", expression(e))),
            S::Return(None) => self.p.line("return;"),
            S::Return(Some(e)) => self.p.line(&format!("return {};", expression(e))),
            S::If { cond, then, other } => {
                self.p.line(&format!("if ({}) {{", expression(cond)));
                self.block(then);
                if other.is_empty() {
                    self.p.line("}");
                } else {
                    self.p.line("} else {");
                    self.block(other);
                    self.p.line("}");
                }
            }
            S::While { cond, body } => {
                self.p.line(&format!("while ({}) {{", expression(cond)));
                self.block(body);
                self.p.line("}");
            }
            S::ForRange { var, end, body } => {
                self.p.line(&format!(
                    "for (int {v} = 0; {v} < {}; ++{v}) {{",
                    expression(end),
                    v = var
                ));
                self.block(body);
                self.p.line("}");
            }
            S::Break => self.p.line("break;"),
            S::Comment(c) => self.lines("// ", c),
            S::Raw(r) => self.lines("", r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Expression as E;

    #[test]
    fn literals() {
        assert_eq!(double_literal(1.0), "1.0");
        assert_eq!(double_literal(0.1), "0.1");
        assert_eq!(double_literal(1e-7), "1e-7");
        assert_eq!(double_literal(-0.0), "-0.0");
        assert_eq!(double_literal(f64::NAN), "TW_NAN_F64");
        assert_eq!(double_literal(f64::NEG_INFINITY), "(-TW_INF_F64)");
        assert_eq!(float_literal(1.0), "1.0f");
        assert_eq!(float_literal(1e-7), "1e-7f");
        assert_eq!(float_literal(f32::INFINITY), "TW_INF_F32");
        assert_eq!(int_literal(i32::MIN), "(-2147483647 - 1)");
        assert_eq!(long_literal(5), "5LL");
        assert_eq!(long_literal(i64::MIN), "(-9223372036854775807LL - 1)");
    }

    #[test]
    fn precedence_parentheses() {
        use BinaryOperator as B;
        let a = || E::name("a");
        let b = || E::name("b");
        // negative literal operand
        assert_eq!(expression(&E::binary(B::Subtract, a(), E::IntLit(-1))), "a - (-1)");
        assert_eq!(expression(&E::unary(UnaryOperator::Negate, E::DoubleLit(-0.0))), "-(-0.0)");
        // left-assoc: right operand of same precedence is parenthesized
        let sum = E::binary(B::Subtract, a(), E::binary(B::Subtract, b(), E::IntLit(1)));
        assert_eq!(expression(&sum), "a - (b - 1)");
        let sum = E::binary(B::Subtract, E::binary(B::Subtract, a(), b()), E::IntLit(1));
        assert_eq!(expression(&sum), "a - b - 1");
        // multiplication binds tighter
        let e = E::binary(B::Add, a(), E::binary(B::Multiply, b(), E::IntLit(2)));
        assert_eq!(expression(&e), "a + b * 2");
        // casts wrap compound operands
        let e = E::cast(Type::Int, E::binary(B::Add, E::cast(Type::Unsigned, a()), E::cast(Type::Unsigned, b())));
        assert_eq!(expression(&e), "(int)((unsigned)a + (unsigned)b)");
        // clarity around bitwise mixing
        let e = E::binary(B::BitOr, E::binary(B::BitAnd, a(), b()), E::IntLit(1));
        assert_eq!(expression(&e), "(a & b) | 1");
        let e = E::binary(B::And, E::binary(B::Less, a(), b()), E::name("c"));
        assert_eq!(expression(&e), "(a < b) && c");
        // top-level negative literal
        assert_eq!(expression(&E::call("f", vec![E::IntLit(-3)])), "f(-3)");
    }
}
