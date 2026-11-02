//! Whole programs and single functions.

use tungsten_ir::*;

use super::derive::Deriver;
use super::expr::{ExprPrinter, const_to_expression, type_text};
use super::float::{f64_lit, quote_string};
use super::labels::Labels;
use crate::stack;

/// `OptimizedProgram` → canonical text. Total over programs whose refs resolve; a dangling ref
/// panics with a message naming the entity. Any depth prints, but text nested deeper than
/// `ast::MAX_DEPTH` doesn't load again.
pub fn print(p: &OptimizedProgram) -> String {
    let labels = Labels::new(p);
    let mut out = String::from("program {\n");
    let mut sections: Vec<String> = vec![];

    let mut s = String::new();
    for (h, input) in p.host_inputs.iter() {
        match input {
            HostInput::Static { t, .. } => {
                s.push_str(&format!("    host {}: {};\n", labels.host(h), type_text(t)))
            }
            HostInput::Dynamic => s.push_str(&format!("    host {} dynamic;\n", labels.host(h))),
        }
    }
    sections.push(s);

    let mut s = String::new();
    for (t, table) in p.permutation_tables.iter() {
        let def = match table {
            PermutationTableInput::Base3DNoise => "base3d".to_string(),
            PermutationTableInput::PerlinNoise {
                ident,
                subident,
                subident_index,
            } => format!(
                "perlin({}, {}, {subident_index})",
                quote_string(ident),
                subident.as_deref().map_or("none".to_string(), quote_string)
            ),
        };
        s.push_str(&format!("    table {} = {def};\n", labels.table(t)));
    }
    sections.push(s);

    let mut s = String::new();
    let derive = Deriver::new(p);
    let consts = ExprPrinter {
        labels: &labels,
        derive: &derive,
        f: None,
    };
    for (c, constant) in p.constants.iter() {
        s.push_str(&format!(
            "    const {} = {};\n",
            labels.constant(c),
            consts.to_string(&const_to_expression(&constant.value))
        ));
    }
    sections.push(s);

    for (f, _) in p.functions.iter() {
        sections.push(function_text(p, &labels, f, 1));
    }
    for (u, _) in p.compute_units.iter() {
        sections.push(unit_text(p, &labels, u));
    }

    let mut s = String::new();
    for (i, e) in p.entry_points.iter().enumerate() {
        s.push_str(&format!(
            "    entry {} = {}{};\n",
            labels.entry(i),
            labels.unit(e.density_function),
            grid_text(e.dimensions, e.scaled_origin, e.scaled_position)
        ));
    }
    sections.push(s);

    out.push_str(
        &sections
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('\n'))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    if out.ends_with('{') || out.ends_with("{\n") {
        // empty program
    } else {
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

fn grid_text(d: (i32, i32, i32), o: (f64, f64, f64), s: (f64, f64, f64)) -> String {
    format!(
        " @dims({}, {}, {}) @origin_scale({}, {}, {}) @position_scale({}, {}, {})",
        d.0,
        d.1,
        d.2,
        f64_lit(o.0),
        f64_lit(o.1),
        f64_lit(o.2),
        f64_lit(s.0),
        f64_lit(s.1),
        f64_lit(s.2)
    )
}

fn unit_text(p: &OptimizedProgram, labels: &Labels, u: Ref<ComputeUnit>) -> String {
    let ComputeUnit::DensityFunction {
        function,
        density_inputs,
        permutation_table_inputs,
        host_inputs,
        helper_functions,
        constants,
        source_hash,
    } = &p.compute_units[u];
    let mut s = format!("    unit {} @hash({source_hash:#x}) {{\n", labels.unit(u));
    s.push_str(&format!("        main {};\n", labels.function(*function)));
    let list = |name: &str, items: Vec<&str>| {
        if items.is_empty() {
            String::new()
        } else {
            format!("        {name} [{}];\n", items.join(", "))
        }
    };
    s.push_str(&list(
        "helpers",
        helper_functions
            .iter()
            .map(|f| labels.function(*f))
            .collect(),
    ));
    s.push_str(&list(
        "tables",
        permutation_table_inputs
            .iter()
            .map(|t| labels.table(*t))
            .collect(),
    ));
    s.push_str(&list(
        "hosts",
        host_inputs.iter().map(|h| labels.host(*h)).collect(),
    ));
    s.push_str(&list(
        "consts",
        constants.iter().map(|c| labels.constant(*c)).collect(),
    ));
    for (d, input) in density_inputs.iter() {
        let owner = p
            .functions
            .iter()
            .find(|(_, f)| f.variables.contains(input.var))
            .map(|(f, _)| f)
            .unwrap_or_else(|| {
                panic!(
                    "dangling Ref: the variable of density input `{}` is in no function",
                    labels.input(d)
                )
            });
        s.push_str(&format!(
            "        input {} = {}{} @var({}.{});\n",
            labels.input(d),
            labels.unit(input.density_function),
            grid_text(input.dimensions, input.scaled_origin, input.scaled_position),
            labels.function(owner),
            labels.var(owner, input.var),
        ));
    }
    s.push_str("    }");
    s
}

/// One function, as in `print`, but at indent 0 and with the labels of `labels`.
pub fn print_function(p: &OptimizedProgram, labels: &Labels, f: Ref<Function>) -> String {
    function_text(p, labels, f, 0)
}

fn function_text(p: &OptimizedProgram, labels: &Labels, f: Ref<Function>, indent: usize) -> String {
    let func = &p.functions[f];
    let pad = "    ".repeat(indent);
    let slots: Vec<(Ref<Var>, &Var)> = func.variables.iter().collect();
    let typed = func.parameters.len() <= slots.len()
        && func.parameters.iter().zip(&slots).all(|(p, (r, _))| p == r);
    let params: Vec<String> = func
        .parameters
        .iter()
        .map(|v| {
            if typed {
                format!(
                    "{}: {}",
                    labels.var(f, *v),
                    type_text(&func.variables[*v].t)
                )
            } else {
                labels.var(f, *v).to_string()
            }
        })
        .collect();
    let mut s = format!(
        "{pad}fn {}({}) -> {} @hash({:#x})",
        labels.function(f),
        params.join(", "),
        type_text(&func.return_type),
        func.source_hash
    );
    if !func.host_inputs.is_empty() {
        let hs: Vec<&str> = func.host_inputs.iter().map(|h| labels.host(*h)).collect();
        s.push_str(&format!(" @hosts({})", hs.join(", ")));
    }
    s.push_str(" {\n");
    let rest = if typed {
        &slots[func.parameters.len()..]
    } else {
        &slots[..]
    };
    if !rest.is_empty() {
        s.push_str(&format!("{pad}    vars {{\n"));
        for (r, v) in rest {
            s.push_str(&format!(
                "{pad}        {}: {},\n",
                labels.var(f, *r),
                type_text(&v.t)
            ));
        }
        s.push_str(&format!("{pad}    }}\n"));
    }
    let derive = Deriver::new(p);
    let ep = ExprPrinter {
        labels,
        derive: &derive,
        f: Some(f),
    };
    stmts(&func.body, f, labels, &ep, indent + 1, &mut s);
    s.push_str(&format!("{pad}}}"));
    if indent == 0 {
        s.push('\n');
    }
    s
}

fn stmts(
    body: &[Statement],
    f: Ref<Function>,
    labels: &Labels,
    ep: &ExprPrinter,
    indent: usize,
    out: &mut String,
) {
    stack::grow(|| stmts_inner(body, f, labels, ep, indent, out))
}

fn stmts_inner(
    body: &[Statement],
    f: Ref<Function>,
    labels: &Labels,
    ep: &ExprPrinter,
    indent: usize,
    out: &mut String,
) {
    let pad = "    ".repeat(indent);
    for st in body {
        match st {
            Statement::Assign { target, value } => out.push_str(&format!(
                "{pad}{} = {};\n",
                labels.var(f, *target),
                ep.to_string(value)
            )),
            Statement::Return(e) => out.push_str(&format!("{pad}return {};\n", ep.to_string(e))),
            Statement::Break => out.push_str(&format!("{pad}break;\n")),
            Statement::If { .. } => {
                out.push_str(&pad);
                if_chain(st, f, labels, ep, indent, out);
                out.push('\n');
            }
            Statement::While { condition, body } => {
                out.push_str(&format!("{pad}while {} {{\n", ep.to_string(condition)));
                stmts(body, f, labels, ep, indent + 1, out);
                out.push_str(&format!("{pad}}}\n"));
            }
            Statement::Repeat { count, body } => {
                out.push_str(&format!("{pad}repeat {count} {{\n"));
                stmts(body, f, labels, ep, indent + 1, out);
                out.push_str(&format!("{pad}}}\n"));
            }
        }
    }
}

/// `if c { … } else if c2 { … } else { … }`, without the leading indent or trailing newline.
fn if_chain(
    st: &Statement,
    f: Ref<Function>,
    labels: &Labels,
    ep: &ExprPrinter,
    indent: usize,
    out: &mut String,
) {
    let pad = "    ".repeat(indent);
    let Statement::If {
        condition,
        then_branch,
        else_branch,
    } = st
    else {
        unreachable!()
    };
    out.push_str(&format!("if {} {{\n", ep.to_string(condition)));
    stmts(then_branch, f, labels, ep, indent + 1, out);
    out.push_str(&format!("{pad}}}"));
    match else_branch.as_slice() {
        [] => {}
        [inner @ Statement::If { .. }] => {
            out.push_str(" else ");
            if_chain(inner, f, labels, ep, indent, out);
        }
        _ => {
            out.push_str(" else {\n");
            stmts(else_branch, f, labels, ep, indent + 1, out);
            out.push_str(&format!("{pad}}}"));
        }
    }
}
