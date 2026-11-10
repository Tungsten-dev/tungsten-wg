//! Text format, parser, elaborator and canonical printer for tungsten programs.
//! See `SPEC.md`.

pub mod ast;
mod elaborate;
mod intern;
mod lexer;
mod numlit;
mod parser;
mod printer;
mod source_map;
mod span;
mod stack;
mod tables;

pub use elaborate::{
    Case, CellSample, Env, Expect, ExpectKind, ExternSig, KnownFail, NoEnv, TestFile, Value,
    elaborate, load,
};
pub use intern::intern;
pub use parser::parse;
pub use printer::{Labels, print, print_function, print_test_file};
pub use source_map::{ExprOwner, ExprPath, ExprStep, SourceMap, StmtPath, StmtStep};
pub use span::{Diagnostic, FileId, Snippet, SourceFiles, Span, Stage};

#[cfg(test)]
mod tests;
