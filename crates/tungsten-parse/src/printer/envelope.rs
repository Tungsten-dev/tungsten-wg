//! `TestFile` → text: directives around the canonical program.

use super::expr::type_text;
use super::float::{f64_lit, quote_name, quote_string};
use super::labels::Labels;
use super::program::print;
use crate::elaborate::{ExpectKind, TestFile, Value};
use crate::stack;
use crate::tables::Num;

fn value(v: &Value) -> String {
    stack::grow(|| value_inner(v))
}

fn value_inner(v: &Value) -> String {
    let list = |vs: &[Value]| vs.iter().map(value).collect::<Vec<_>>().join(", ");
    if let Some(n) = Num::of_value(v) {
        return n.spell();
    }
    match v {
        Value::I8(_) | Value::I32(_) | Value::I64(_) | Value::F32(_) | Value::F64(_) => {
            unreachable!()
        }
        Value::Bool(b) => b.to_string(),
        Value::Vec3([x, y, z]) => {
            format!("vec3({}, {}, {})", f64_lit(*x), f64_lit(*y), f64_lit(*z))
        }
        Value::Pos3([x, y, z]) => format!("pos3({x}, {y}, {z})"),
        Value::Array(vs) => format!("[{}]", list(vs)),
        Value::Record(name, fields) => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(n, v)| format!("{}: {}", quote_name(n), value(v)))
                .collect();
            let t = type_text(&tungsten_ir::VariableType::Extern(name));
            if fs.is_empty() {
                format!("new {t} {{}}")
            } else {
                format!("new {t} {{ {} }}", fs.join(", "))
            }
        }
    }
}

pub fn print_test_file(t: &TestFile) -> String {
    let labels = Labels::new(&t.program);
    let mut out = String::new();
    for k in &t.knownfails {
        out.push_str(&format!(
            "@knownfail({}, {}",
            quote_name(&k.backend),
            quote_string(&k.reason)
        ));
        if let Some(c) = &k.case {
            out.push_str(&format!(", case = {}", quote_string(c)));
        }
        if let Some(s) = &k.stage {
            out.push_str(&format!(", stage = {}", quote_name(s)));
        }
        out.push_str(")\n");
    }
    if let Some(n) = t.fuel {
        out.push_str(&format!("@fuel({n})\n"));
    }
    if let Some(tier) = &t.tier {
        out.push_str(&format!("@tier({})\n", quote_name(tier)));
    }
    if let Some(c) = &t.cells {
        out.push_str(&format!(
            "@cells(sample = {}, seed = {})\n",
            c.sample, c.seed
        ));
    }
    out.push_str(&print(&t.program));
    for c in &t.cases {
        out.push_str(&format!(
            "@case {} origin({}, {}, {}) seed({})",
            quote_string(&c.name),
            f64_lit(c.origin[0]),
            f64_lit(c.origin[1]),
            f64_lit(c.origin[2]),
            c.seed
        ));
        if !c.hosts.is_empty() {
            let hs: Vec<String> = c
                .hosts
                .iter()
                .map(|(h, v)| format!("{} = {}", labels.host(*h), value(v)))
                .collect();
            out.push_str(&format!(" host({})", hs.join(", ")));
        }
        out.push('\n');
    }
    if let Some(r) = &t.reject {
        match r {
            Some(k) => out.push_str(&format!("@expect reject({})\n", quote_name(k))),
            None => out.push_str("@expect reject\n"),
        }
    }
    for e in &t.expects {
        let case = quote_string(&t.cases[e.case].name);
        let entry = e.entry.map(|i| labels.entry(i));
        let line = match &e.kind {
            ExpectKind::Error(k) => format!("error({})", quote_name(k)),
            ExpectKind::Cell([x, y, z], v) => {
                format!("{}[{x}, {y}, {z}] = {}", entry.unwrap(), value(v))
            }
            ExpectKind::All(v) => format!("{}[*] = {}", entry.unwrap(), value(v)),
            ExpectKind::Grid(vs) => format!(
                "{} = [{}]",
                entry.unwrap(),
                vs.iter().map(value).collect::<Vec<_>>().join(", ")
            ),
        };
        out.push_str(&format!("@expect {case} {line}\n"));
    }
    out
}
