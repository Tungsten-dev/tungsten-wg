//! Builds valid-by-construction [`OptimizedProgram`](tungsten_ir::OptimizedProgram)s from Rust.
//! See `README.md` and `specs/builder.md`.
//!
//! A [`ProgramBuilder`] owns the program. Each function is an owned [`FnBuilder`], finished as a
//! unit or a helper. Expressions are typed as they are built, a mistake is recorded where it was
//! written, and [`ProgramBuilder::build`] returns every mistake at once. Unit and function lists
//! are derived from the bodies.

mod decls;
mod derive;
mod error;
mod expr;
mod function;
mod macros;
mod program;

pub use decls::{Decls, ExternDecl, RecordDecl};
pub use error::{BuildError, BuildErrorKind, BuildErrors, FieldProblem};
pub use expr::{Expr, origin, origin_scale, pos3, position_scale};
pub use function::{Block, FnBuilder, Local};
pub use program::{
    Auto, AutoArg, Constant, Grid, Helper, Host, Input, InputSpec, Param, ProgramBuilder, Unit,
};

pub use tungsten_ir::{OptimizedProgram, VariableType};

#[doc(hidden)]
pub use tungsten_ir as __ir;

/// Everything a frontend usually needs: the builders, handles, ambients, macros and the scalar
/// type names (`F64`, `I32`, …).
pub mod prelude {
    pub use crate::{
        Auto, AutoArg, Block, BuildError, BuildErrors, Constant, Expr, FnBuilder, Grid, Helper, Host, Input,
        InputSpec, Local, Param, ProgramBuilder, Unit, origin, origin_scale, pos3,
        position_scale,
    };
    // `pos3` above brings both the ambient function and the `pos3!` macro.
    pub use crate::{array, body, build, extern_fns, record_decls, vec3, vt};
    pub use tungsten_ir::OptimizedProgram;
    pub use tungsten_ir::VariableType::{
        self, Bool, F32, F64, I8, I32, I64, Handle, Pos3, Vec3,
    };
}
