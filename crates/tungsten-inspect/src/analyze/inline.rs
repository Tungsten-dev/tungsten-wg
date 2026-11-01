//! Inline verdicts (SPEC.md §6.8): `tungsten_optimizer::explain_inline` with refs mapped to ids
//! and a one-line explanation.

use std::collections::HashMap;

use tungsten_ir::model::{ComputeUnit, Ref};
use tungsten_optimizer::{GridInfo, InlineRejection, InlineVerdict, explain_inline};

use super::{Ctx, fmt_dims, fmt_triple};
use crate::model::{Grid, InlineVerdictInfo, Verdict};

pub(crate) fn verdicts(ctx: &Ctx) -> HashMap<Ref<ComputeUnit>, InlineVerdictInfo> {
    explain_inline(ctx.program)
        .into_iter()
        .map(|(cu, v)| (cu, translate(ctx, v)))
        .collect()
}

fn translate(ctx: &Ctx, v: InlineVerdict) -> InlineVerdictInfo {
    let fn_display = |f| ctx.functions[&f].display.clone();
    let (verdict, explanation) = match v {
        InlineVerdict::Inlinable { into, site, reads } => (
            Verdict::Inlinable {
                into: ctx.unit_id(into),
                site: ctx.function_id(site),
                reads,
            },
            format!(
                "inlinable into {} (fn {}, {} read{})",
                ctx.units[&into].display,
                fn_display(site),
                reads,
                if reads == 1 { "" } else { "s" }
            ),
        ),
        InlineVerdict::Rejected(r) => match r {
            InlineRejection::EntryPoint => (
                Verdict::EntryPoint,
                "blocked: entry point (must stay addressable)".to_string(),
            ),
            InlineRejection::Unused => (Verdict::Unused, "blocked: no unit reads it".to_string()),
            InlineRejection::MultipleUses(uses) => (
                Verdict::MultipleUses { uses },
                format!("blocked: multiple uses ({uses} density inputs read it)"),
            ),
            InlineRejection::SelfUse => (Verdict::SelfUse, "blocked: it reads itself".to_string()),
            InlineRejection::CallerTooComplex => (
                Verdict::CallerTooComplex,
                "blocked: caller is too complex".to_string(),
            ),
            InlineRejection::CallerGridAmbiguous => (
                Verdict::CallerGridAmbiguous,
                "blocked: caller is consumed on several grids (or none)".to_string(),
            ),
            InlineRejection::GridMismatch { caller, input } => {
                let explanation = format!(
                    "blocked: read on grid {} but the caller runs on {} (same grid required)",
                    fmt_grid(&input),
                    fmt_grid(&caller)
                );
                (
                    Verdict::GridMismatch {
                        caller: grid(caller),
                        input: grid(input),
                    },
                    explanation,
                )
            }
            InlineRejection::IndexedRead { function } => (
                Verdict::IndexedRead {
                    function: ctx.function_id(function),
                },
                format!(
                    "blocked: read with an index in fn {} (identity mapping required)",
                    fn_display(function)
                ),
            ),
            InlineRejection::ReadInSeveralFunctions => (
                Verdict::ReadInSeveralFunctions,
                "blocked: read in more than one function of the caller".to_string(),
            ),
            InlineRejection::NotRead => (
                Verdict::NotRead,
                "blocked: the caller declares the input but never reads it".to_string(),
            ),
        },
    };
    InlineVerdictInfo {
        verdict,
        explanation,
    }
}

fn grid(g: GridInfo) -> Grid {
    Grid {
        dimensions: g.dimensions,
        scaled_origin: g.scaled_origin,
        scaled_position: g.scaled_position,
    }
}

fn fmt_grid(g: &GridInfo) -> String {
    format!(
        "{} o{} p{}",
        fmt_dims(g.dimensions),
        fmt_triple(g.scaled_origin),
        fmt_triple(g.scaled_position)
    )
}
