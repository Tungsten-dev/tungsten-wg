mod fuse;
mod inline;

use tungsten_ir::model::OptimizedProgram;

pub use inline::{GridInfo, InlineRecord, InlineRejection, InlineVerdict, explain_inline};

/// What an inlining run changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineStats {
    /// Number of compute units that were inlined into their caller (`== records.len()`).
    pub inlined: usize,
    /// One record per merge, in the order they happened.
    pub records: Vec<InlineRecord>,
}

pub fn perform_inline(program: &mut OptimizedProgram) -> InlineStats {
    let records = inline::Inliner::new(program).traverse();
    InlineStats {
        inlined: records.len(),
        records,
    }
}
