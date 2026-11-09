//! `body!`, `build!`, `extern_fns!`, `auto_initializers!`, `record_decls!`, `vec3!`, `pos3!`, `array!` and `vt!`.
//!
//! `body!` and `build!` expand onto `FnBuilder`'s methods, so both styles build the same IR.
//! The builder argument must be a variable name: nested blocks become closures whose parameter
//! shadows it, so code inside a block can use it (and other `&mut` values such as the program
//! builder) freely.

/// A `VariableType` from a short spelling: `F64`, `I32`, `"Record"`, `handle "Name"`, `[F64; 3]`, `(…)`.
#[macro_export]
macro_rules! vt {
    ([$t:tt; $n:expr]) => {
        $crate::__ir::VariableType::Array(::std::boxed::Box::new($crate::vt!($t)), $n)
    };
    ($s:literal) => {
        $crate::__ir::VariableType::Extern($s)
    };
    (handle $s:literal) => {
        $crate::__ir::VariableType::Handle($s)
    };
    (($($inner:tt)*)) => {
        $crate::vt!($($inner)*)
    };
    ($id:ident) => {
        $crate::__ir::VariableType::$id
    };
}

/// Declares extern functions. Each signature gives a typed call function and, with
/// `group name;`, an entry in `pub fn name() -> Decls`. A handle type is written
/// `handle "Name"`.
///
/// ```ignore
/// extern_fns! {
///     group aquifer_externs;
///     random_next_int(r: "Random", bound: I32): I32,
///     checked_div(a: F64, b: F64): F64 @domain_error,
///     table_get(t: handle "Table", i: I32): I32,
/// }
/// ```
#[macro_export]
macro_rules! extern_fns {
    (group $g:ident; $($body:tt)*) => {
        $crate::extern_fns!(@items [$g] [] $($body)*);
    };

    // Signatures, one at a time. `done` holds `{ name [arg: (type)..] (ret) [flag] }`.
    (@items $g:tt $done:tt) => {
        $crate::extern_fns!(@emit $g $done);
    };
    (@items $g:tt [$($done:tt)*] $name:ident ( $($args:tt)* ) : $($rest:tt)*) => {
        $crate::extern_fns!(@args $g [$($done)*] $name [] [$($args)*] [$($rest)*]);
    };

    // Arguments, normalised to `arg: (type)`.
    (@args $g:tt $done:tt $name:ident [$($acc:tt)*] [$a:ident : handle $s:literal , $($more:tt)*] $rest:tt) => {
        $crate::extern_fns!(@args $g $done $name [$($acc)* $a : (handle $s),] [$($more)*] $rest);
    };
    (@args $g:tt $done:tt $name:ident [$($acc:tt)*] [$a:ident : handle $s:literal] $rest:tt) => {
        $crate::extern_fns!(@args $g $done $name [$($acc)* $a : (handle $s),] [] $rest);
    };
    (@args $g:tt $done:tt $name:ident [$($acc:tt)*] [$a:ident : $t:tt , $($more:tt)*] $rest:tt) => {
        $crate::extern_fns!(@args $g $done $name [$($acc)* $a : ($t),] [$($more)*] $rest);
    };
    (@args $g:tt $done:tt $name:ident [$($acc:tt)*] [$a:ident : $t:tt] $rest:tt) => {
        $crate::extern_fns!(@args $g $done $name [$($acc)* $a : ($t),] [] $rest);
    };
    (@args $g:tt $done:tt $name:ident $acc:tt [] [handle $s:literal $($rest:tt)*]) => {
        $crate::extern_fns!(@tail $g $done $name $acc (handle $s) [$($rest)*]);
    };
    (@args $g:tt $done:tt $name:ident $acc:tt [] [$t:tt $($rest:tt)*]) => {
        $crate::extern_fns!(@tail $g $done $name $acc ($t) [$($rest)*]);
    };

    // After the return type: an optional `@flag` and an optional comma.
    (@tail $g:tt [$($done:tt)*] $name:ident $acc:tt $ret:tt [@ $flag:ident , $($more:tt)*]) => {
        $crate::extern_fns!(@items $g [$($done)* { $name $acc $ret [$flag] }] $($more)*);
    };
    (@tail $g:tt [$($done:tt)*] $name:ident $acc:tt $ret:tt [@ $flag:ident]) => {
        $crate::extern_fns!(@items $g [$($done)* { $name $acc $ret [$flag] }]);
    };
    (@tail $g:tt [$($done:tt)*] $name:ident $acc:tt $ret:tt [, $($more:tt)*]) => {
        $crate::extern_fns!(@items $g [$($done)* { $name $acc $ret [] }] $($more)*);
    };
    (@tail $g:tt [$($done:tt)*] $name:ident $acc:tt $ret:tt []) => {
        $crate::extern_fns!(@items $g [$($done)* { $name $acc $ret [] }]);
    };

    (@emit [$g:ident] [$( { $name:ident [$($arg:ident : $aty:tt,)*] $ret:tt [$($flag:ident)?] } )*]) => {
        $( $crate::extern_fns!(@fn $name($($arg : $aty),*) : $ret); )*

        /// The declarations of this group's externs.
        pub fn $g() -> $crate::Decls {
            let mut d = $crate::Decls::new();
            $(
                d.externs.push($crate::ExternDecl {
                    name: ::std::string::String::from(stringify!($name)),
                    params: ::std::vec![$($crate::vt!($aty)),*],
                    ret: $crate::vt!($ret),
                    domain_error: $crate::extern_fns!(@flag $($flag)?),
                });
            )*
            d
        }
    };
    (@emit [] [$( { $name:ident [$($arg:ident : $aty:tt,)*] $ret:tt [$($flag:ident)?] } )*]) => {
        $( $crate::extern_fns!(@fn $name($($arg : $aty),*) : $ret); )*
    };

    (@fn $name:ident($($arg:ident : $aty:tt),*) : $ret:tt) => {
        #[track_caller]
        #[allow(clippy::too_many_arguments)]
        pub fn $name($($arg: impl ::std::convert::Into<$crate::Expr>),*) -> $crate::Expr {
            $crate::Expr::__extern_call(
                stringify!($name),
                ::std::vec![$($crate::vt!($aty)),*],
                $crate::vt!($ret),
                ::std::vec![$(::std::convert::Into::<$crate::Expr>::into($arg)),*],
            )
        }
    };
    (@flag) => { false };
    (@flag domain_error) => { true };
    ($($body:tt)*) => {
        $crate::extern_fns!(@items [] [] $($body)*);
    };
}

/// Declares autoconst initializers for the registry, as `pub fn name() -> Decls`. Arguments are
/// `I64`, `F64`, `Bool` or `Str`; the return type is a scalar or `handle "Name"`.
///
/// ```ignore
/// auto_initializers! {
///     group table_initializers;
///     make_table(I64, I64): handle "Table",
///     derive_salt(I64, Str): I64,
/// }
/// ```
#[macro_export]
macro_rules! auto_initializers {
    (group $g:ident; $($body:tt)*) => {
        /// The declarations of this group's autoconst initializers.
        pub fn $g() -> $crate::Decls {
            let mut d = $crate::Decls::new();
            $crate::auto_initializers!(@items d $($body)*);
            d
        }
    };
    (@items $d:ident) => {};
    (@items $d:ident $name:ident ( $($a:ident),* $(,)? ) : handle $s:literal $(, $($more:tt)*)?) => {
        $d.initializers.push($crate::AutoInitializerDecl {
            name: ::std::string::String::from(stringify!($name)),
            params: ::std::vec![$($crate::AutoArgType::$a),*],
            ret: $crate::vt!(handle $s),
        });
        $crate::auto_initializers!(@items $d $($($more)*)?);
    };
    (@items $d:ident $name:ident ( $($a:ident),* $(,)? ) : $ret:tt $(, $($more:tt)*)?) => {
        $d.initializers.push($crate::AutoInitializerDecl {
            name: ::std::string::String::from(stringify!($name)),
            params: ::std::vec![$($crate::AutoArgType::$a),*],
            ret: $crate::vt!($ret),
        });
        $crate::auto_initializers!(@items $d $($($more)*)?);
    };
}

/// Declares records for the registry, as `pub fn name() -> Decls`.
///
/// ```ignore
/// record_decls! {
///     group structure_records;
///     Piece { terrain_adjustment: I32, block_box: "BlockBox" },
/// }
/// ```
#[macro_export]
macro_rules! record_decls {
    (group $g:ident; $($rec:ident { $($field:ident : $fty:tt),* $(,)? }),* $(,)?) => {
        /// The declarations of this group's records.
        pub fn $g() -> $crate::Decls {
            let mut d = $crate::Decls::new();
            $(
                d.records.push($crate::RecordDecl {
                    name: ::std::string::String::from(stringify!($rec)),
                    fields: ::std::vec![$(
                        (::std::string::String::from(stringify!($field)), $crate::vt!($fty))
                    ),*],
                });
            )*
            d
        }
    };
}

/// `vec3(x, y, z)` from three `f64`s.
#[macro_export]
macro_rules! vec3 {
    ($x:expr, $y:expr, $z:expr $(,)?) => {
        $crate::Expr::construct(
            $crate::__ir::VariableType::Vec3,
            [
                ::std::convert::Into::<$crate::Expr>::into($x),
                ::std::convert::Into::<$crate::Expr>::into($y),
                ::std::convert::Into::<$crate::Expr>::into($z),
            ],
        )
    };
}

/// `pos3(x, y, z)` from three `i32`s.
#[macro_export]
macro_rules! pos3 {
    ($x:expr, $y:expr, $z:expr $(,)?) => {
        $crate::Expr::construct(
            $crate::__ir::VariableType::Pos3,
            [
                ::std::convert::Into::<$crate::Expr>::into($x),
                ::std::convert::Into::<$crate::Expr>::into($y),
                ::std::convert::Into::<$crate::Expr>::into($z),
            ],
        )
    };
}

/// An array literal. All elements must have the type of the first.
#[macro_export]
macro_rules! array {
    ($($e:expr),* $(,)?) => {
        $crate::Expr::array(::std::vec![$(::std::convert::Into::<$crate::Expr>::into($e)),*])
    };
}

/// Appends statements to the function builder `f`:
///
/// ```ignore
/// build!(f, {
///     i = 0;
///     let total: F64 = shared.call([]);
///     while (i.lt(3)) { total += d.at(i); i += 1; }
///     return total;
/// });
/// ```
#[macro_export]
macro_rules! build {
    ($b:ident, { $($t:tt)* }) => {{
        $crate::build!(@acc $b; $($t)*);
    }};

    (@acc $b:ident;) => {};

    (@acc $b:ident; return $e:expr; $($rest:tt)*) => {
        { let __e = $e; $b.ret(__e); }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; break; $($rest:tt)*) => {
        $b.break_();
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; let $n:ident : $ty:tt = $e:expr; $($rest:tt)*) => {
        let __e = $e;
        let $n = $b.temp(stringify!($n), $crate::vt!($ty));
        $b.assign($n, __e);
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; let $n:ident = $e:expr; $($rest:tt)*) => {
        let $n = $e;
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; if ($c:expr) { $($t:tt)* } else { $($e:tt)* } $($rest:tt)*) => {
        {
            let __c = $c;
            let __c = $b.__condition(__c);
            let __then = $b.block(|$b| { $crate::build!(@acc $b; $($t)*); });
            let __else = $b.block(|$b| { $crate::build!(@acc $b; $($e)*); });
            $b.__if(__c, __then, __else);
        }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; if ($c:expr) { $($t:tt)* } $($rest:tt)*) => {
        {
            let __c = $c;
            $b.if_then(__c, |$b| { $crate::build!(@acc $b; $($t)*); });
        }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; while ($c:expr) { $($t:tt)* } $($rest:tt)*) => {
        {
            let __c = $c;
            $b.while_(__c, |$b| { $crate::build!(@acc $b; $($t)*); });
        }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; repeat ($n:expr) { $($t:tt)* } $($rest:tt)*) => {
        {
            let __n = $n;
            $b.repeat(__n, |$b| { $crate::build!(@acc $b; $($t)*); });
        }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; $t:ident += $e:expr; $($rest:tt)*) => {
        { let __e = $e; $b.assign($t, $t + __e); }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; $t:ident -= $e:expr; $($rest:tt)*) => {
        { let __e = $e; $b.assign($t, $t - __e); }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; $t:ident *= $e:expr; $($rest:tt)*) => {
        { let __e = $e; $b.assign($t, $t * __e); }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; $t:ident /= $e:expr; $($rest:tt)*) => {
        { let __e = $e; $b.assign($t, $t / __e); }
        $crate::build!(@acc $b; $($rest)*);
    };
    (@acc $b:ident; $t:ident = $e:expr; $($rest:tt)*) => {
        { let __e = $e; $b.assign($t, __e); }
        $crate::build!(@acc $b; $($rest)*);
    };
}

/// Like `build!`, but returns the statements as a `Block` for `f.append`.
#[macro_export]
macro_rules! body {
    ($b:ident, { $($t:tt)* }) => {
        $b.block(|$b| { $crate::build!(@acc $b; $($t)*); })
    };
}
