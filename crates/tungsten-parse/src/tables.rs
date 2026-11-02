//! One list per fact. The lexer, parser, elaborator and printer take their spellings from these
//! tables, so they can't disagree about them (P2 depends on that).

use tungsten_ir::{AmbientKind, BinaryOperator, Expression, UnaryOperator, VariableType};

use crate::elaborate::Value;
use crate::lexer::Kw;
use crate::printer::float::{f32_lit, f64_lit};

macro_rules! numeric_kinds {
    ($($kind:ident($t:ty) as $num:ident: $vt:ident, $name:literal, $width:literal,
       float = $float:literal, short = [$($short:literal),*], long = [$($long:literal),*],
       spell = $spell:expr;)*) => {
        /// The kind of a numeric literal.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum NumKind { $($kind),* }

        impl NumKind {
            pub fn name(self) -> &'static str {
                match self { $(NumKind::$kind => $name),* }
            }
            pub fn width(self) -> u32 {
                match self { $(NumKind::$kind => $width),* }
            }
            pub fn is_float(self) -> bool {
                match self { $(NumKind::$kind => $float),* }
            }
            pub fn var_type(self) -> VariableType {
                match self { $(NumKind::$kind => VariableType::$vt),* }
            }
            pub fn of_type(t: &VariableType) -> Option<NumKind> {
                match t { $(VariableType::$vt => Some(NumKind::$kind),)* _ => None }
            }
            /// The kind a literal suffix selects, and whether it is a short suffix (`b`, `L`,
            /// `f`), which only decimal literals take.
            pub fn from_suffix(s: &str) -> Option<(NumKind, bool)> {
                $(
                    if [$($short),*].contains(&s) { return Some((NumKind::$kind, true)); }
                    if [$($long),*].contains(&s) { return Some((NumKind::$kind, false)); }
                )*
                None
            }
        }

        /// A converted numeric literal.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum Num { $($num($t)),* }

        impl Num {
            pub fn kind(self) -> NumKind {
                match self { $(Num::$num(_) => NumKind::$kind),* }
            }
            pub fn into_expression(self) -> Expression {
                match self { $(Num::$num(v) => Expression::$num(v)),* }
            }
            pub fn of_expression(e: &Expression) -> Option<Num> {
                match e { $(Expression::$num(v) => Some(Num::$num(*v)),)* _ => None }
            }
            pub fn into_value(self) -> Value {
                match self { $(Num::$num(v) => Value::$vt(v)),* }
            }
            pub fn of_value(v: &Value) -> Option<Num> {
                match v { $(Value::$vt(v) => Some(Num::$num(*v)),)* _ => None }
            }
            /// The canonical spelling.
            pub fn spell(self) -> String {
                match self { $(Num::$num(v) => ($spell)(v)),* }
            }
        }
    };
}

numeric_kinds! {
    Byte(i8) as Byte: I8, "i8", 8, float = false, short = ["b"], long = ["_i8"],
        spell = |v: i8| format!("{v}b");
    Int(i32) as Int: I32, "i32", 32, float = false, short = [], long = [],
        spell = |v: i32| v.to_string();
    Long(i64) as Long: I64, "i64", 64, float = false, short = ["L"], long = ["_i64"],
        spell = |v: i64| format!("{v}L");
    F32(f32) as Float: F32, "f32", 32, float = true, short = ["f"], long = ["_f32"],
        spell = f32_lit;
    F64(f64) as Double: F64, "f64", 64, float = true, short = [], long = ["_f64"],
        spell = f64_lit;
}

/// How a binary operator parses and types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// `&&`, `||`
    Logic,
    Cmp,
    /// `&`, `|`, `^`
    Bit,
    Shift,
    Arith,
}

impl OpClass {
    /// May not be mixed unparenthesized with any other binary operator (SPEC §6.2).
    pub fn restricted(self) -> bool {
        matches!(self, OpClass::Bit | OpClass::Shift)
    }
}

macro_rules! binary_ops {
    ($($op:ident = $text:literal, $prec:literal, $class:ident;)*) => {
        pub fn op_text(op: BinaryOperator) -> &'static str {
            match op { $(BinaryOperator::$op => $text),* }
        }
        /// Higher binds tighter.
        pub fn op_prec(op: BinaryOperator) -> u8 {
            match op { $(BinaryOperator::$op => $prec),* }
        }
        pub fn op_class(op: BinaryOperator) -> OpClass {
            match op { $(BinaryOperator::$op => OpClass::$class),* }
        }
        pub fn op_from_text(s: &str) -> Option<BinaryOperator> {
            match s { $($text => Some(BinaryOperator::$op),)* _ => None }
        }
    };
}

binary_ops! {
    Or = "||", 1, Logic;
    And = "&&", 2, Logic;
    Equal = "==", 3, Cmp;
    NotEqual = "!=", 3, Cmp;
    Less = "<", 3, Cmp;
    LessEqual = "<=", 3, Cmp;
    Greater = ">", 3, Cmp;
    GreaterEqual = ">=", 3, Cmp;
    BitOr = "|", 4, Bit;
    BitXor = "^", 5, Bit;
    BitAnd = "&", 6, Bit;
    ShiftLeft = "<<", 7, Shift;
    ShiftRight = ">>", 7, Shift;
    LogicalShiftRight = ">>>", 7, Shift;
    Add = "+", 8, Arith;
    Subtract = "-", 8, Arith;
    Multiply = "*", 9, Arith;
    Divide = "/", 9, Arith;
}

macro_rules! prim_types {
    ($($kw:ident => $vt:ident),* $(,)?) => {
        /// The type a type keyword names.
        pub fn prim_type(kw: Kw) -> Option<VariableType> {
            match kw { $(Kw::$kw => Some(VariableType::$vt),)* _ => None }
        }
        /// The keyword that spells a type (not for arrays and externs).
        pub fn prim_kw(t: &VariableType) -> Option<Kw> {
            match t { $(VariableType::$vt => Some(Kw::$kw),)* _ => None }
        }
    };
}

prim_types! {
    Bool => Bool, I8 => I8, I32 => I32, I64 => I64, F32 => F32, F64 => F64, Vec3 => Vec3,
    Pos3 => Pos3, Density => DensityInput, PermTable => PermutationTable,
    InterpolatedNoiseSampler => InterpolatedNoiseSampler,
}

macro_rules! ambient_kinds {
    ($($kind:ident = $text:literal: $vt:ident),* $(,)?) => {
        pub fn ambient_text(k: AmbientKind) -> &'static str {
            match k { $(AmbientKind::$kind => $text),* }
        }
        pub fn ambient_from_text(s: &str) -> Option<AmbientKind> {
            match s { $($text => Some(AmbientKind::$kind),)* _ => None }
        }
        pub fn ambient_type(k: AmbientKind) -> VariableType {
            match k { $(AmbientKind::$kind => VariableType::$vt),* }
        }
        pub const AMBIENT_EXPECTED: &str = concat!("one of" $(, " `", $text, "`")*);
    };
}

ambient_kinds! {
    Origin = "origin": Vec3,
    Pos3 = "pos3": Pos3,
    OriginScale = "origin_scale": Vec3,
    PositionScale = "position_scale": Vec3,
}

/// The canonical prefix of a unary operator; the operand follows in parentheses.
pub fn unary_text(op: UnaryOperator) -> &'static str {
    match op {
        UnaryOperator::Negate => "-",
        UnaryOperator::Not => "!",
        UnaryOperator::Ref(false) => "&",
        UnaryOperator::Ref(true) => "&mut ",
    }
}
