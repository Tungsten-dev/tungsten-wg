//! `tungsten-testsuite`: differential test harness, corpus and runtimes.
//!
//! Milestone 1: registry, `TestEnv`, shared externs, and the interpreter-side trials
//! (round-trip, lowering, interpreter, inliner). See `SPEC.md` at the repo root.

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
