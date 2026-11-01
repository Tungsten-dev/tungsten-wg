//! Before/after summary of an optimization run (SPEC.md §9).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use tungsten_interface::Interface;
use tungsten_interface::naming::NamingRules;
use tungsten_ir::model::{ComputeUnit, OptimizedProgram, Ref};
use tungsten_optimizer::{InlineRecord, InlineStats};

use crate::analyze::{self, Ctx, Options, ir_text};
use crate::backend::ElementSizes;
use crate::model::{EntryInfo, Inspection};
use crate::render::format::bytes;

/// Text summary of what an optimization pass changed (SPEC.md §9).
///
/// `before` must be a clone of the program that `after` was mutated from, and `stats` the result
/// of that run; refs are matched with `Arena::translate`, which panics on unrelated programs.
pub fn diff(before: &OptimizedProgram, after: &OptimizedProgram, stats: &InlineStats) -> String {
    let options = Options {
        dataflow: false,
        element_sizes: ElementSizes::default(),
    };
    let (bi, ai) = (
        Interface::build(before, &NamingRules::inspect()),
        Interface::build(after, &NamingRules::inspect()),
    );
    let b = Ctx::new(before, &bi, &options);
    let a = Ctx::new(after, &ai, &options);
    let (bm, am) = (analyze::inspection(&b), analyze::inspection(&a));

    let mut s = String::new();
    inline_section(&mut s, &b, &a, stats);
    changed_functions(&mut s, &b, &a);
    program_table(&mut s, &bm, &am);
    entry_tables(&mut s, &bm, &am);
    s
}

/// Display name of a unit given as a ref into `after` (live or removed).
fn unit_display(b: &Ctx, a: &Ctx, r: Ref<ComputeUnit>) -> String {
    if let Some(n) = a.units.get(&r) {
        return n.display.clone();
    }
    let rb = b
        .program
        .compute_units
        .translate(r, &a.program.compute_units);
    b.units
        .get(&rb)
        .map_or_else(|| "<unknown unit>".to_string(), |n| n.display.clone())
}

fn inline_section(s: &mut String, b: &Ctx, a: &Ctx, stats: &InlineStats) {
    let (before, after) = (&b.program.compute_units, &a.program.compute_units);
    let rounds: BTreeSet<usize> = stats.records.iter().map(|r| r.round).collect();
    let _ = writeln!(
        s,
        "inline: {} unit{} absorbed in {} round{}\n",
        stats.inlined,
        plural(stats.inlined),
        rounds.len(),
        plural(rounds.len())
    );

    // Absorbed, in merge order; chains are followed to the live unit that ended up with it.
    let by_callee: HashMap<Ref<ComputeUnit>, &InlineRecord> =
        stats.records.iter().map(|r| (r.callee, r)).collect();
    let mut rows: Vec<(String, String)> = Vec::new();
    for rec in &stats.records {
        let mut chain = vec![rec.callee, rec.caller];
        let mut cur = rec.caller;
        while !after.contains(cur) && chain.len() <= stats.records.len() + 1 {
            let Some(next) = by_callee.get(&cur) else {
                break;
            };
            cur = next.caller;
            chain.push(cur);
        }
        let names: Vec<String> = chain.iter().map(|r| unit_display(b, a, *r)).collect();
        let site = a
            .functions
            .get(&rec.site)
            .map_or("<unknown fn>", |n| n.display.as_str());
        let mut detail = format!(
            "round {}, site fn {site}, {} read{}",
            rec.round,
            rec.reads,
            plural(rec.reads)
        );
        if chain.len() > 2 {
            let last = names.last().unwrap();
            let state = if after.contains(cur) {
                "is live"
            } else {
                "is NOT live"
            };
            detail.push_str(&format!("; chain, {last} {state}"));
        }
        rows.push((names.join(" → "), detail));
    }
    s.push_str("absorbed:");
    if rows.is_empty() {
        s.push_str(" (none)\n");
    } else {
        s.push('\n');
        let width = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
        for (left, detail) in &rows {
            let pad = width - left.chars().count();
            let _ = writeln!(s, "  {left}{}  ({detail})", " ".repeat(pad));
        }
    }

    // Removed units without a record, and units that only exist after.
    let mut without: Vec<String> = before
        .iter()
        .map(|(r, _)| r)
        .filter(|r| {
            let ra = after.translate(*r, before);
            !after.contains(ra) && !by_callee.contains_key(&ra)
        })
        .map(|r| b.units[&r].display.clone())
        .collect();
    without.sort();
    let mut added: Vec<String> = after
        .iter()
        .map(|(r, _)| r)
        .filter(|r| !before.contains(before.translate(*r, after)))
        .map(|r| a.units[&r].display.clone())
        .collect();
    added.sort();
    let _ = writeln!(s, "removed without provenance: {}", list_or_none(&without));
    let _ = writeln!(s, "added: {}", list_or_none(&added));
}

/// Functions present in both programs whose IR text differs.
fn changed_functions(s: &mut String, b: &Ctx, a: &Ctx) {
    let (before, after) = (&b.program.functions, &a.program.functions);
    let mut changed: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for (r, _) in before.iter() {
        let ra = after.translate(r, before);
        if !after.contains(ra) {
            removed.push(b.functions[&r].display.clone());
        } else if ir_text::function(b, r) != ir_text::function(a, ra) {
            changed.push(a.functions[&ra].display.clone());
        }
    }
    let mut added: Vec<String> = after
        .iter()
        .map(|(r, _)| r)
        .filter(|r| !before.contains(before.translate(*r, after)))
        .map(|r| a.functions[&r].display.clone())
        .collect();
    changed.sort();
    removed.sort();
    added.sort();
    let _ = writeln!(s, "changed functions: {}", changed.len());
    for name in &changed {
        let _ = writeln!(s, "  {name}");
    }
    if !removed.is_empty() {
        let _ = writeln!(s, "removed functions: {}", removed.join(", "));
    }
    if !added.is_empty() {
        let _ = writeln!(s, "added functions: {}", added.join(", "));
    }
    s.push('\n');
}

fn program_table(s: &mut String, bm: &Inspection, am: &Inspection) {
    let rows = [
        ("units", bm.summary.units, am.summary.units),
        (
            "reachable units",
            bm.summary.reachable_units,
            am.summary.reachable_units,
        ),
        ("instances", bm.summary.instances, am.summary.instances),
        ("helpers", bm.summary.helpers, am.summary.helpers),
    ];
    table(
        s,
        "program:",
        rows.iter().map(|(l, x, y)| count_row(l, *x, *y)),
    );
}

fn entry_tables(s: &mut String, bm: &Inspection, am: &Inspection) {
    let by_index = |m: &Inspection| -> HashMap<usize, EntryInfo> {
        m.entries.iter().map(|e| (e.index, e.clone())).collect()
    };
    let (be, ae) = (by_index(bm), by_index(am));
    let mut only = Vec::new();
    for n in 0..bm.entries.len().max(am.entries.len()) {
        let (x, y) = match (be.get(&n), ae.get(&n)) {
            (Some(x), Some(y)) => (x, y),
            (Some(x), None) => {
                only.push(format!("before: {}", x.display));
                continue;
            }
            (None, Some(y)) => {
                only.push(format!("after: {}", y.display));
                continue;
            }
            (None, None) => continue,
        };
        let (bx, by) = (&x.buffers, &y.buffers);
        let rows = vec![
            count_row("instances", x.instances.len(), y.instances.len()),
            count_row("waves", x.wave_count, y.wave_count),
            count_row("max wave width", x.max_wave_width, y.max_wave_width),
            bytes_row("total bytes", bx.cpu.total_bytes, by.cpu.total_bytes),
            bytes_row(
                "peak live (cpu)",
                bx.cpu.peak_live_bytes,
                by.cpu.peak_live_bytes,
            ),
            bytes_row(
                "peak live (gpu)",
                bx.gpu.peak_live_bytes,
                by.gpu.peak_live_bytes,
            ),
            bytes_row("slot bytes (cpu)", bx.cpu.slot_bytes, by.cpu.slot_bytes),
            bytes_row("slot bytes (gpu)", bx.gpu.slot_bytes, by.gpu.slot_bytes),
        ];
        table(s, &format!("per entry: {}", y.display), rows.into_iter());
    }
    let _ = writeln!(s, "entries only in before/after: {}", list_or_none(&only));
}

type Row = [String; 4];

fn count_row(label: &str, x: usize, y: usize) -> Row {
    let delta = y as i64 - x as i64;
    let delta = if delta > 0 {
        format!("+{delta}")
    } else {
        delta.to_string()
    };
    [label.to_string(), x.to_string(), y.to_string(), delta]
}

fn bytes_row(label: &str, x: usize, y: usize) -> Row {
    let delta = match (x, y) {
        (0, 0) => "0%".to_string(),
        (0, _) => "new".to_string(),
        _ => {
            let pct = (y as f64 - x as f64) / x as f64 * 100.0;
            format!("{}{:.0}%", if pct > 0.0 { "+" } else { "" }, pct)
        }
    };
    [label.to_string(), bytes(x), bytes(y), delta]
}

fn table(s: &mut String, title: &str, rows: impl Iterator<Item = Row>) {
    let rows: Vec<Row> = rows.collect();
    let w0 = rows.iter().map(|r| r[0].len()).max().unwrap_or(0).max(16);
    let w = |c: usize| rows.iter().map(|r| r[c].len()).chain([6]).max().unwrap();
    let (w1, w2, w3) = (w(1), w(2), w(3));
    let _ = writeln!(
        s,
        "{title:<width$}  {:>w1$}  {:>w2$}  {:>w3$}",
        "before",
        "after",
        "Δ",
        width = w0 + 2
    );
    for [label, x, y, d] in &rows {
        let _ = writeln!(s, "  {label:<w0$}  {x:>w1$}  {y:>w2$}  {d:>w3$}");
    }
    s.push('\n');
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}
