//! Inspection backend: turns an `OptimizedProgram` into human-friendly outputs instead of
//! executable code. See SPEC.md for the full design.
//!
//! Everything goes through one analysis model, [`Inspection`] (`model.rs`). `analyze` builds it
//! from the program and the shared [`Interface`](tungsten_interface::Interface) (built with
//! [`NamingRules::inspect`](tungsten_interface::naming::NamingRules::inspect), so names are the raw
//! canonical names); renderers only read the model, so the files and `inspection.json` agree.
//!
//! Output (§7): `index.md`, `inspection.json`, `ir/program.txt`, DOT graphs (per-entry
//! dependencies and waves, call and resource graphs, per-function dataflow DAGs) and SVG buffer
//! charts. [`diff`] summarizes an optimization run as text (§9).
//!
//! Per-function IR text is printed during analysis (`FunctionInfo::ir`), so `diff` can compare
//! functions across programs and the renderers never need the program.
//!
//! The inspector assumes a valid program, with the same preconditions as the other backends
//! (no unit cycles, no `HostInput::Dynamic`, named host inputs, no stale refs) and may panic
//! otherwise.

mod analyze;
pub mod backend;
mod diff;
pub mod model;
mod render;

#[cfg(test)]
mod tests;

pub use backend::{ElementSizes, InspectBackend, InspectConfig, InspectOutput, Views, inspect};
pub use diff::diff;
pub use model::Inspection;
