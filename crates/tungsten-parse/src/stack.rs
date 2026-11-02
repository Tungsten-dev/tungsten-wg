//! Stack growth for the recursive stages. Their depth is bounded (by `ast::MAX_DEPTH` and
//! `ast::MAX_RECURSION` for parse and elaborate, by the program for print), but debug-build frames
//! are large, so every recursive entry point moves to a fresh heap-allocated stack segment when
//! the current one runs low. The depth limits are then the only limits, in every build profile.

/// Grow when less than this is left.
const RED_ZONE: usize = 256 * 1024;
/// The size of each new segment.
const SEGMENT: usize = 4 * 1024 * 1024;

pub(crate) fn grow<T>(f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(RED_ZONE, SEGMENT, f)
}
