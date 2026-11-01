//! `ir/program.txt` (SPEC.md §7.5), assembled from the model. Function bodies come pre-printed
//! in `FunctionInfo::ir` (see `analyze/ir_text.rs`).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use super::format::grid_exact;
use crate::model::{FunctionInfo, FunctionKind, Inspection, Summary};

/// The header line shared with `index.md`.
pub(crate) fn summary_line(s: &Summary) -> String {
    format!(
        "{} units ({} reachable), {} helpers, {} constants, {} tables, {} host inputs, {} entries",
        s.units, s.reachable_units, s.helpers, s.constants, s.tables, s.host_inputs, s.entries
    )
}

/// The text, and the 1-based line of each unit's `unit … {` line, by unit id.
pub(crate) fn program(i: &Inspection) -> (String, BTreeMap<String, usize>) {
    let displays: HashMap<&str, &str> = i
        .units
        .iter()
        .map(|u| (u.id.as_str(), u.display.as_str()))
        .chain(
            i.instances
                .iter()
                .map(|x| (x.id.as_str(), x.display.as_str())),
        )
        .chain(
            i.functions
                .iter()
                .map(|x| (x.id.as_str(), x.display.as_str())),
        )
        .chain(
            i.constants
                .iter()
                .map(|x| (x.id.as_str(), x.display.as_str())),
        )
        .chain(i.tables.iter().map(|x| (x.id.as_str(), x.display.as_str())))
        .chain(
            i.host_inputs
                .iter()
                .map(|x| (x.id.as_str(), x.display.as_str())),
        )
        .collect();
    let display = |id: &str| displays.get(id).copied().unwrap_or("?").to_string();
    let functions: HashMap<&str, &FunctionInfo> =
        i.functions.iter().map(|f| (f.id.as_str(), f)).collect();
    let marker = |reachable: bool| if reachable { "" } else { "  // unreachable" };

    let mut s = String::new();
    let _ = writeln!(s, "// program: {}\n", summary_line(&i.summary));

    for e in &i.entries {
        let inst = i.instances.iter().find(|x| x.id == e.instance).unwrap();
        let dup = if e.display != inst.display {
            format!(" #{}", e.index)
        } else {
            String::new()
        };
        let _ = writeln!(
            s,
            "entry {} @ {}{dup}",
            display(&inst.unit),
            grid_exact(&inst.grid)
        );
    }
    s.push('\n');

    for c in &i.constants {
        let _ = writeln!(
            s,
            "const {} = {}{}",
            c.display,
            c.value,
            marker(c.reachable)
        );
    }
    if !i.constants.is_empty() {
        s.push('\n');
    }
    for t in &i.tables {
        let _ = writeln!(s, "table {}{}", t.display, marker(t.reachable));
    }
    for h in &i.host_inputs {
        let _ = writeln!(s, "host {}: {}{}", h.display, h.ty, marker(h.reachable));
    }
    if !i.tables.is_empty() || !i.host_inputs.is_empty() {
        s.push('\n');
    }

    let mut lines = BTreeMap::new();
    let list = |ids: &[String]| {
        ids.iter()
            .map(|x| display(x))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for u in &i.units {
        if !u.reachable {
            s.push_str("// unreachable\n");
        }
        lines.insert(u.id.clone(), s.matches('\n').count() + 1);
        let _ = writeln!(s, "unit {} -> {} {{", u.display, u.return_type);
        if !u.inputs.is_empty() {
            s.push_str("    inputs {\n");
            for input in &u.inputs {
                let idx = if input.indexed { "  // indexed" } else { "" };
                let _ = writeln!(
                    s,
                    "        {} = {} @ {}{idx}",
                    input.name,
                    display(&input.producer),
                    grid_exact(&input.grid)
                );
            }
            s.push_str("    }\n");
        }
        for (label, ids) in [
            ("tables", &u.tables),
            ("host", &u.host_inputs),
            ("helpers", &u.helpers),
            ("consts", &u.constants),
        ] {
            if !ids.is_empty() {
                let _ = writeln!(s, "    {label} {{ {} }}", list(ids));
            }
        }
        let _ = writeln!(s, "    inline: {}\n", u.inline_verdict.explanation);
        for line in functions[u.function.as_str()].ir.lines() {
            let _ = writeln!(s, "    {line}");
        }
        s.push_str("}\n\n");
    }

    for f in i
        .functions
        .iter()
        .filter(|f| f.kind == FunctionKind::Helper)
    {
        if !f.reachable {
            s.push_str("// unreachable\n");
        }
        s.push_str(&f.ir);
        s.push('\n');
    }
    (s, lines)
}
