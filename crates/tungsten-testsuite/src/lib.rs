//! `tungsten-testsuite`: differential test harness, corpus and runtimes.
//!
//! Registry, `TestEnv`, shared externs, the interpreter-side trials (round-trip, lowering,
//! interpreter, inliner) and the Rust, Java and CUDA backend trials. See the crate README.

pub mod compare;
pub mod corpus;
pub mod env;
pub mod externs;
pub mod harness;
pub mod knownfail;
pub mod oracle;
pub mod registry;
pub mod selection;
pub mod values;
