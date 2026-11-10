//! Build errors. Every mistake is recorded where it was written and collected by the
//! [`ProgramBuilder`](crate::ProgramBuilder), which returns them all from `build`.

use std::fmt;
use std::panic::Location;

/// One problem found while building, with the Rust source location that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    pub kind: BuildErrorKind,
    /// The function being built, if the problem is inside one.
    pub function: Option<String>,
    pub location: &'static Location<'static>,
}

impl BuildError {
    pub(crate) fn new(kind: BuildErrorKind, location: &'static Location<'static>) -> Self {
        Self {
            kind,
            function: None,
            location,
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.location)?;
        if let Some(name) = &self.function {
            write!(f, "fn `{name}`: ")?;
        }
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for BuildError {}

/// What went wrong with a field read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldProblem {
    /// `x`, `y` and `z` are the only fields of `vec3` and `pos3`.
    NoSuchField,
    /// The base is neither `vec3`, `pos3` nor a record.
    NotAStruct,
    /// The stated type of a `vec3`/`pos3` field isn't its real type.
    Annotation { annotated: String, actual: String },
}

/// The kinds of [`BuildError`]. Types appear as in the checker's messages (`VariableType`'s
/// `Debug`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildErrorKind {
    #[error("`{op}` can't be applied to `{left}` and `{right}`")]
    BadBinary {
        op: &'static str,
        left: String,
        right: String,
    },
    #[error("`{op}` can't be applied to `{operand}`")]
    BadUnary { op: &'static str, operand: String },
    #[error("can't cast `{from}` to `{to}`")]
    BadCast { from: String, to: String },
    #[error("{}", field_message(field, base, problem))]
    BadField {
        field: String,
        base: String,
        problem: FieldProblem,
    },
    #[error("indexing needs an array, found `{found}`")]
    NotArray { found: String },
    #[error("an index must be an `i32`, found `{found}`")]
    BadIndex { found: String },
    #[error("expected {expected} arguments, found {found}")]
    Arity { expected: usize, found: usize },
    #[error("argument {index}: expected `{expected}`, found `{found}`")]
    ArgumentType {
        index: usize,
        expected: String,
        found: String,
    },
    #[error("assigned value: expected `{expected}`, found `{found}`")]
    AssignType { expected: String, found: String },
    #[error("returned value: expected `{expected}`, found `{found}`")]
    ReturnType { expected: String, found: String },
    #[error("condition: expected `bool`, found `{found}`")]
    ConditionType { found: String },
    #[error("argument {index} of `{t}`: expected `{expected}`, found `{found}`")]
    BadConstruct {
        t: String,
        index: usize,
        expected: String,
        found: String,
    },
    #[error("empty array literal")]
    EmptyArray,
    #[error("array element {index}: expected `{expected}`, found `{found}`")]
    ArrayElement {
        index: usize,
        expected: String,
        found: String,
    },
    #[error("{what} comes from another {owner}")]
    ForeignHandle {
        what: &'static str,
        owner: &'static str,
    },
    #[error("`{name}` is already declared in this function")]
    DuplicateLocal { name: String },
    #[error("constant `{name}` can't contain {found}")]
    NotConstant { name: String, found: &'static str },
    #[error("host `{name}` was declared as `{existing}`, and now as `{requested}`")]
    HostTypeConflict {
        name: String,
        existing: String,
        requested: String,
    },
    #[error("param `{name}` was declared as `{existing}`, and now as `{requested}`")]
    AutoParamConflict {
        name: String,
        existing: String,
        requested: String,
    },
    #[error("a unit's main can't have parameters (it has {params})")]
    UnitHasParams { params: usize },
}

fn field_message(field: &str, base: &str, problem: &FieldProblem) -> String {
    match problem {
        FieldProblem::NoSuchField => format!("`{base}` has no field `{field}`"),
        FieldProblem::NotAStruct => format!("field `{field}` can't be read from `{base}`"),
        FieldProblem::Annotation { annotated, actual } => {
            format!("field `{field}` annotated as `{annotated}`, but it is `{actual}`")
        }
    }
}

/// Every error of a build, in the order they were recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildErrors(pub Vec<BuildError>);

impl BuildErrors {
    pub fn iter(&self) -> impl Iterator<Item = &BuildError> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for BuildErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BuildErrors {}
