pub use crate::spmt::builder::SPMTBuilder;
use crate::spmt::model::*;

#[macro_export]
macro_rules! impl_binop {
    ($($trait:ident :: $method:ident => $op:ident),* $(,)?) => {$(
        impl<'m, R: Into<Expression<'m>>> std::ops::$trait<R> for Expression<'m> {
            type Output = Expression<'m>;
            fn $method(self, rhs: R) -> Expression<'m> {
                Expression::BinaryOp {
                    op: BinaryOperator::$op,
                    left: Box::new(self),
                    right: Box::new(rhs.into()),
                }
            }
        }

        impl<'m, R: Into<Expression<'m>>> std::ops::$trait<R> for Var<'m> {
            type Output = Expression<'m>;
            fn $method(self, rhs: R) -> Expression<'m> {
                Expression::BinaryOp {
                    op: BinaryOperator::$op,
                    left: Box::new(self.into()),
                    right: Box::new(rhs.into()),
                }
            }
        }
    )*};
}
impl_binop!(Add::add => Add, Sub::sub => Subtract, Mul::mul => Multiply, Div::div => Divide);

impl<'m> std::ops::Neg for Expression<'m> {
    type Output = Expression<'m>;
    fn neg(self) -> Self::Output {
        Expression::UnaryOp {
            op: UnaryOperator::Negate,
            operand: Box::new(self),
        }
    }
}

impl<'m> std::ops::Neg for Var<'m> {
    type Output = Expression<'m>;
    fn neg(self) -> Self::Output {
        Expression::UnaryOp {
            op: UnaryOperator::Negate,
            operand: Box::new(self.into()),
        }
    }
}

impl<'m> std::ops::Not for Expression<'m> {
    type Output = Expression<'m>;
    fn not(self) -> Self::Output {
        Expression::UnaryOp {
            op: UnaryOperator::Not,
            operand: Box::new(self),
        }
    }
}

impl<'m> std::ops::Not for Var<'m> {
    type Output = Expression<'m>;
    fn not(self) -> Self::Output {
        Expression::UnaryOp {
            op: UnaryOperator::Not,
            operand: Box::new(self.into()),
        }
    }
}

impl<'m> Expression<'m> {
    pub fn index(self, index: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::ArrayAccess {
            array: Box::new(self),
            index: Box::new(index.into()),
        }
    }

    pub fn field(self, field: impl Into<String>, type_of_field: VariableType) -> Expression<'m> {
        Expression::Field {
            base: Box::new(self.into()),
            field: field.into(),
            type_of_field,
            known_idnex: None,
        }
    }

    pub fn cast(self, to_type: VariableType) -> Expression<'m> {
        Expression::ExplicitCast {
            to: to_type,
            expr: Box::new(self.into()),
        }
    }

    pub fn shl(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::ShiftLeft,
            left: Box::new(self),
            right: Box::new(rhs.into()),
        }
    }

    pub fn shr(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::ShiftRight,
            left: Box::new(self),
            right: Box::new(rhs.into()),
        }
    }

    pub fn ushr(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::LogicalShiftRight,
            left: Box::new(self),
            right: Box::new(rhs.into()),
        }
    }

    pub fn bitor(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::BitOr,
            left: Box::new(self),
            right: Box::new(rhs.into()),
        }
    }

    pub fn bitand(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::BitAnd,
            left: Box::new(self),
            right: Box::new(rhs.into()),
        }
    }
    pub fn bitxor(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::BitXor,
            left: Box::new(self),
            right: Box::new(rhs.into()),
        }
    }
}

impl<'m> Var<'m> {
    pub fn index(self, index: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::ArrayAccess {
            array: Box::new(self.into()),
            index: Box::new(index.into()),
        }
    }

    pub fn field(self, field: impl Into<String>, type_of_field: VariableType) -> Expression<'m> {
        Expression::Field {
            base: Box::new(self.into()),
            field: field.into(),
            type_of_field,
            known_idnex: None,
        }
    }
    pub fn cast(self, to_type: VariableType) -> Expression<'m> {
        Expression::ExplicitCast {
            to: to_type,
            expr: Box::new(self.into()),
        }
    }

    pub fn shl(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::ShiftLeft,
            left: Box::new(self.into()),
            right: Box::new(rhs.into()),
        }
    }

    pub fn shr(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::ShiftRight,
            left: Box::new(self.into()),
            right: Box::new(rhs.into()),
        }
    }
    pub fn ushr(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::LogicalShiftRight,
            left: Box::new(self.into()),
            right: Box::new(rhs.into()),
        }
    }

    pub fn bitor(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::BitOr,
            left: Box::new(self.into()),
            right: Box::new(rhs.into()),
        }
    }

    pub fn bitand(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::BitAnd,
            left: Box::new(self.into()),
            right: Box::new(rhs.into()),
        }
    }

    pub fn bitxor(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
        Expression::BinaryOp {
            op: BinaryOperator::BitXor,
            left: Box::new(self.into()),
            right: Box::new(rhs.into()),
        }
    }

    pub fn as_mut_ref(self) -> Expression<'m> {
        Expression::UnaryOp {
            op: UnaryOperator::MutRef,
            operand: Box::new(self.into()),
        }
    }
}

impl<'a, 'm> FunctionRef<'m> {
    pub fn call(self, args: impl IntoIterator<Item = Expression<'m>>) -> Expression<'m> {
        let mut parameters = vec![];
        parameters.extend(args.into_iter());
        Expression::FunctionCall {
            function: self,
            parameters,
        }
    }
}

// impl<'m> From<Var<'m>> for Expression<'m> {
//     fn from(v: Var<'m>) -> Self {
//         Expression::Variable(v)
//     }
// }

macro_rules! impl_from_lit {
    ($($ty:ty => $variant:ident),*) => {$(
        impl<'m> From<$ty> for Expression<'m> {
            fn from(v: $ty) -> Self { Expression::$variant(v) }
        }
    )*};
}
impl_from_lit!(f32 => Float, f64 => Double, i8 => Byte, i32 => Int, i64 => Long, bool => Bool);

impl<'m> From<Var<'m>> for Expression<'m> {
    fn from(v: Var<'m>) -> Self {
        Expression::Variable(v)
    }
}

macro_rules! impl_cmp {
    ($($name:ident => $op:ident),*) => {
        impl<'m> Expression<'m> {$(
            pub fn $name(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
                Expression::BinaryOp {
                    op: BinaryOperator::$op,
                    left: Box::new(self),
                    right: Box::new(rhs.into()),
                }
            }
        )*}

        impl<'m> Var<'m> {$(
            pub fn $name(self, rhs: impl Into<Expression<'m>>) -> Expression<'m> {
                Expression::BinaryOp {
                    op: BinaryOperator::$op,
                    left: Box::new(self.into()),
                    right: Box::new(rhs.into()),
                }
            }
        )*}
    };
}
impl_cmp!(lt => Less, le => LessEqual, gt => Greater, ge => GreaterEqual,
          eq_expr => Equal, ne => NotEqual, and => And, or => Or);

#[macro_export]
macro_rules! vt {
    // [T; N]  -> Array(Box<T>, N)   (nests: [[F32; 3]; 3])
    ([$t:tt; $n:expr]) => {
        $crate::spmt::VariableType::Array(Box::new($crate::vt!($t)), $n)
    };
    // "Name"  -> Extern("Name")
    ($s:literal) => { $crate::spmt::VariableType::Extern($s) };
    // (T)     -> T   (parenthesize anything the other rules can't take as one token)
    (($($inner:tt)*)) => { $crate::vt!($($inner)*) };
    // F32, I32, Pos3, Bool, DensityInput, ...
    ($id:ident) => { $crate::spmt::VariableType::$id };
}

#[macro_export]
macro_rules! extern_fns {
    ($($name:ident($($arg:ident : $aty:tt),* $(,)?) : $ret:tt),* $(,)?) => {$(
        pub fn $name<'m>($($arg: impl Into<$crate::spmt::Expression<'m>>),*) -> $crate::spmt::Expression<'m> {
            $crate::spmt::Expression::ExternCall {
                function_name: stringify!($name).to_string(),
                parameters: vec![$($arg.into()),*],
                parameter_types: vec![$($crate::vt!($aty)),*],
                return_type: $crate::vt!($ret),
            }
        }
    )*};
}
extern_fns! {
    sin(x: F64): F64, cos(x: F64): F64, sqrt(x: F64): F64, floor(x: F64): F64,
    pow(a: F64, b: F64): F64, min(a: F64, b: F64): F64, max(a: F64, b: F64): F64, clamp(x: F64, lo: F64, hi: F64): F64,
}

#[macro_export]
macro_rules! vars {
    ($b:expr; $($name:ident : $t:ident),* $(,)?) => {
        $( let $name = $b.local(stringify!($name), VariableType::$t); )*
    };
}

#[macro_export]
macro_rules! build {
    // entry point: body!(builder, { ... })
    ($b:expr, { $($t:tt)* }) => {{
        let mut __v: Vec<Statement> = Vec::new();
        $crate::body!(@acc $b, __v; $($t)*);
        for stmt in __v {
            ($b).add_statement(stmt);
        }
    }};
}

#[macro_export]
macro_rules! body {
    ($b:expr, { $($t:tt)* }) => {{
        let mut __v: Vec<Statement> = Vec::new();
        $crate::body!(@acc $b, __v; $($t)*);
        __v
    }};

    (@acc $b:expr, $v:ident;) => {};

    (@acc $b:expr, $v:ident; return $e:expr; $($rest:tt)*) => {
        $v.push(Statement::Return(($e).into()));
        $crate::body!(@acc $b, $v; $($rest)*);
    };
    (@acc $b:expr, $v:ident; break; $($rest:tt)*) => {
        $v.push(Statement::Break);
        $crate::body!(@acc $b, $v; $($rest)*);
    };

    // let x: T = e;   (declares the variable on the builder, binds `x` in Rust scope)
    (@acc $b:expr, $v:ident; let $n:ident : $ty:ident = $e:expr; $($rest:tt)*) => {
        let $n = $b.local(stringify!($n), VariableType::$ty);
        $v.push(Statement::Assign { target: $n, value: ($e).into() });
        $crate::body!(@acc $b, $v; $($rest)*);
    };

    (@acc $b:expr, $v:ident; let $n:ident = $e:expr; $($rest:tt)*) => {
        let $n = $e;
        $crate::body!(@acc $b, $v; $($rest)*);
    };

    (@acc $b:expr, $v:ident; while ($c:expr) { $($body:tt)* } $($rest:tt)*) => {
        $v.push(Statement::While {
            condition: ($c).into(),
            body: $crate::body!($b, { $($body)* }),
        });
        $crate::body!(@acc $b, $v; $($rest)*);
    };
    (@acc $b:expr, $v:ident; repeat ($c:expr) { $($body:tt)* } $($rest:tt)*) => {
        $v.push(Statement::Repeat {
            count: ($c),
            body: $crate::body!($b, { $($body)* }),
        });
        $crate::body!(@acc $b, $v; $($rest)*);
    };
    // (@acc $b:expr, repeat ($n:expr) { $($a:tt)* } $($rest:tt)*) => {
    //     $v.push(Statement::Repeat { count: $n, body: body!($($a)*) });
    //     body!(@acc $v; $($rest)*);
    // };
    (@acc $b:expr, $v:ident; if ($c:expr) { $($t:tt)* } else { $($e:tt)* } $($rest:tt)*) => {
        $v.push(Statement::If {
            condition: ($c).into(),
            then_branch: $crate::body!($b, { $($t)* }),
            else_branch: $crate::body!($b, { $($e)* }),
        });
        $crate::body!(@acc $b, $v; $($rest)*);
    };
    (@acc $b:expr, $v:ident; if ($c:expr) { $($t:tt)* } $($rest:tt)*) => {
        $v.push(Statement::If {
            condition: ($c).into(),
            then_branch: $crate::body!($b, { $($t)* }),
            else_branch: Vec::new(),
        });
        $crate::body!(@acc $b, $v; $($rest)*);
    };

    // x -= e; x += e; ...
    (@acc $b:expr, $v:ident; $t:ident -= $e:expr; $($rest:tt)*) => {
        $v.push(Statement::Assign { target: $t, value: Expression::from($t) - ($e) });
        $crate::body!(@acc $b, $v; $($rest)*);
    };
    (@acc $b:expr, $v:ident; $t:ident += $e:expr; $($rest:tt)*) => {
        $v.push(Statement::Assign { target: $t, value: Expression::from($t) + ($e) });
        $crate::body!(@acc $b, $v; $($rest)*);
    };

    (@acc $b:expr, $v:ident; $t:ident = $e:expr; $($rest:tt)*) => {
        $v.push(Statement::Assign { target: $t, value: ($e).into() });
        $crate::body!(@acc $b, $v; $($rest)*);
    };
}

#[macro_export]
macro_rules! vec3 {
    ($x:expr, $y:expr, $z:expr) => {
        Expression::Construct {
            t: VariableType::Vec3,
            args: vec![($x).into(), ($y).into(), ($z).into()],
        }
    };
}

#[macro_export]
macro_rules! pos3 {
    ($x:expr, $y:expr, $z:expr) => {
        Expression::Construct {
            t: VariableType::Pos3,
            args: vec![($x).into(), ($y).into(), ($z).into()],
        }
    };
}

#[macro_export]
macro_rules! array {
    ($($e:expr),* $(,)?) => { Expression::ArrayLiteral(vec![$(($e).into()),*]) };
}
