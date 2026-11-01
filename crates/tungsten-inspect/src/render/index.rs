//! `index.md`, the landing page (SPEC.md §7.6). Links to disabled view families are left out.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::format::{bytes, dims, md};
use super::ir_text::summary_line;
use super::{Ctx, relative};
use crate::model::Verdict;

const FROM: &str = "index.md";

fn link(text: &str, to: &str) -> String {
    format!("[{text}]({})", relative(FROM, to))
}

pub(crate) fn index(ctx: &Ctx, ir_lines: &BTreeMap<String, usize>) -> String {
    let i = ctx.i;
    let v = ctx.views;
    let mut s = String::new();

    let _ = writeln!(s, "# Inspection\n\n{}\n", summary_line(&i.summary));

    s.push_str("## Rendering\n\n");
    s.push_str("Render every `.dot` file next to itself so the links in the SVGs resolve:\n\n");
    s.push_str(
        "```sh\nfind . -name '*.dot' -exec sh -c 'dot -Tsvg \"$1\" -o \"${1%.dot}.svg\"' _ {} \\;\n```\n\n",
    );
    s.push_str(
        "Links inside rendered SVGs assume this was run. For large programs, \
         `program/call_graph.dot` and `program/resources.dot` may lay out better with `sfdp`.\n\n",
    );

    // Entry points.
    s.push_str("## Entry points\n\n");
    s.push_str("| entry | grid | instances | waves | max width | total | peak live (cpu / gpu) | slots (cpu / gpu) | views |\n");
    s.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for e in &i.entries {
        let inst = ctx.instances[e.instance.as_str()];
        let (cpu, gpu) = (&e.buffers.cpu, &e.buffers.gpu);
        let mut views = Vec::new();
        if let Some(dir) = ctx.entry_dir(&e.id) {
            if v.entries {
                views.push(link("dependencies", &format!("{dir}/dependencies.dot")));
                views.push(link("waves", &format!("{dir}/waves.dot")));
            }
            if v.buffers {
                views.push(link("buffers", &format!("{dir}/buffers.txt")));
                views.push(link("cpu", &format!("{dir}/buffers.cpu.svg")));
                views.push(link("gpu", &format!("{dir}/buffers.gpu.svg")));
            }
        }
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} / {} | {} / {} | {} |",
            md(&e.display),
            dims(inst.grid.dimensions),
            e.instances.len(),
            e.wave_count,
            e.max_wave_width,
            bytes(cpu.total_bytes),
            bytes(cpu.peak_live_bytes),
            bytes(gpu.peak_live_bytes),
            bytes(cpu.slot_bytes),
            bytes(gpu.slot_bytes),
            views.join(" ")
        );
    }

    // Compute units.
    s.push_str("\n## Compute units\n\n");
    s.push_str("| unit | grids | entries | fan-in | fan-out | depth | inline | links |\n");
    s.push_str("| --- | --- | ---: | ---: | ---: | ---: | --- | --- |\n");
    for u in &i.units {
        let grids: Vec<String> = u
            .instances
            .iter()
            .map(|x| dims(ctx.instances[x.as_str()].grid.dimensions))
            .collect();
        let mut links = Vec::new();
        if let Some(dot) = ctx.function_dot(&u.function) {
            links.push(link("dag", dot));
        }
        if let Some(line) = ir_lines.get(&u.id) {
            links.push(format!(
                "[ir:{line}]({}#L{line})",
                relative(FROM, "ir/program.txt")
            ));
        }
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            md(&u.display),
            if grids.is_empty() {
                "-".to_string()
            } else {
                grids.join(", ")
            },
            u.entries.len(),
            u.fan_in,
            u.fan_out,
            u.depth,
            md(&u.inline_verdict.explanation),
            links.join(" ")
        );
    }

    // Sharing.
    s.push_str("\n## Sharing\n\n");
    let shared: Vec<_> = i.instances.iter().filter(|x| x.entries.len() > 1).collect();
    if shared.is_empty() {
        s.push_str("No instance is needed by more than one entry.\n");
    }
    for x in shared {
        let entries: Vec<&str> = x.entries.iter().map(|e| ctx.display(e)).collect();
        let _ = writeln!(
            s,
            "- `{}` ({}): {}",
            x.display,
            bytes(x.bytes),
            entries.join(", ")
        );
    }

    // Inlining.
    s.push_str("\n## Inlining\n\n");
    s.push_str(
        "Verdicts are a snapshot of this program: inlining one unit can change the verdicts of \
         others.\n",
    );
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for u in &i.units {
        groups
            .entry(verdict_kind(&u.inline_verdict.verdict))
            .or_default()
            .push(format!(
                "- `{}`: {}",
                u.display, u.inline_verdict.explanation
            ));
    }
    for (kind, lines) in groups {
        let _ = writeln!(s, "\n### {kind} ({})\n\n{}", lines.len(), lines.join("\n"));
    }

    // Unreachable.
    s.push_str("\n## Unreachable\n\n");
    let un = &i.summary.unreachable;
    let mut any = false;
    for (label, ids) in [
        ("units", &un.units),
        ("functions", &un.functions),
        ("constants", &un.constants),
        ("tables", &un.tables),
        ("host inputs", &un.host_inputs),
    ] {
        if !ids.is_empty() {
            any = true;
            let names: Vec<String> = ids
                .iter()
                .map(|x| format!("`{}`", ctx.display(x)))
                .collect();
            let _ = writeln!(s, "- {label}: {}", names.join(", "));
        }
    }
    if !any {
        s.push_str("Everything is reachable from an entry point.\n");
    }

    // Resources.
    s.push_str("\n## Resources\n\n");
    if v.program_graphs {
        let _ = writeln!(
            s,
            "Graphs: {}, {}\n",
            link("call graph", "program/call_graph.dot"),
            link("resources", "program/resources.dot")
        );
    }
    s.push_str("| kind | resource | users |\n| --- | --- | ---: |\n");
    for t in &i.tables {
        let _ = writeln!(s, "| table | {} | {} |", md(&t.display), t.users.len());
    }
    for h in &i.host_inputs {
        let _ = writeln!(
            s,
            "| host input | {}: {} | {} |",
            md(&h.display),
            h.ty,
            h.users.len()
        );
    }
    for c in &i.constants {
        let _ = writeln!(
            s,
            "| constant | {} = {} | {} |",
            md(&c.display),
            md(&c.value),
            c.users.len()
        );
    }

    // Notes.
    s.push_str("\n## Notes\n\n");
    if i.notes.is_empty() {
        s.push_str("None.\n");
    }
    for n in &i.notes {
        let _ = writeln!(s, "- {n}");
    }

    // Path map.
    s.push_str("\n## Path map\n\n");
    if v.ir_text {
        let _ = writeln!(s, "IR text: {}\n", link("ir/program.txt", "ir/program.txt"));
    }
    s.push_str("| id | path |\n| --- | --- |\n");
    for (id, path) in &i.paths {
        let _ = writeln!(s, "| `{}` | {} |", md(id), link(&md(path), path));
    }
    s
}

fn verdict_kind(v: &Verdict) -> &'static str {
    match v {
        Verdict::Inlinable { .. } => "inlinable",
        Verdict::EntryPoint => "entry point",
        Verdict::Unused => "unused",
        Verdict::MultipleUses { .. } => "multiple uses",
        Verdict::SelfUse => "self use",
        Verdict::CallerTooComplex => "caller too complex",
        Verdict::CallerGridAmbiguous => "caller grid ambiguous",
        Verdict::GridMismatch { .. } => "grid mismatch",
        Verdict::IndexedRead { .. } => "indexed read",
        Verdict::ReadInSeveralFunctions => "read in several functions",
        Verdict::NotRead => "not read",
    }
}
