//! Assembles the output file tree from the model (SPEC.md §7).
//!
//! Renderers read only the [`Inspection`]. The one thing they add to it is the path map (§7.1),
//! which is computed first so that `inspection.json` and `index.md` both carry it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use crate::backend::{InspectConfig, Views};
use crate::model::{FunctionInfo, Inspection, InstanceInfo, UnitInfo};

mod dot;
pub(crate) mod format;
mod graphs;
mod index;
pub(crate) mod ir_text;
mod svg;

pub(crate) fn render(
    inspection: &mut Inspection,
    config: &InspectConfig,
) -> BTreeMap<PathBuf, String> {
    let views = &config.views;
    inspection.paths = paths(inspection, views);
    let ctx = Ctx::new(inspection, views);

    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut add = |path: String, contents: String| {
        files.insert(PathBuf::from(path), contents);
    };

    let mut ir_lines = BTreeMap::new();
    if views.ir_text {
        let (text, lines) = ir_text::program(inspection);
        add("ir/program.txt".into(), text);
        ir_lines = lines;
    }
    if views.program_graphs {
        add(
            "program/call_graph.dot".into(),
            graphs::call_graph(&ctx, "program/call_graph.dot"),
        );
        add(
            "program/resources.dot".into(),
            graphs::resources(&ctx, "program/resources.dot"),
        );
    }
    for entry in &inspection.entries {
        let Some(dir) = ctx.entry_dir(&entry.id) else {
            continue;
        };
        if views.entries {
            let path = format!("{dir}/dependencies.dot");
            add(path.clone(), graphs::dependencies(&ctx, entry, &path));
            let path = format!("{dir}/waves.dot");
            add(path.clone(), graphs::waves(&ctx, entry, &path));
        }
        if views.buffers {
            add(format!("{dir}/buffers.txt"), svg::table(&ctx, entry));
            add(
                format!("{dir}/buffers.cpu.svg"),
                svg::chart(&ctx, entry, &entry.buffers.cpu),
            );
            add(
                format!("{dir}/buffers.gpu.svg"),
                svg::chart(&ctx, entry, &entry.buffers.gpu),
            );
        }
    }
    for f in &inspection.functions {
        if let (Some(dag), Some(path)) = (&f.dag, ctx.function_dot(&f.id)) {
            add(path.to_string(), graphs::function(&ctx, f, dag, path));
        }
    }
    if views.index {
        add("index.md".into(), index::index(&ctx, &ir_lines));
    }

    let json = serde_json::to_string_pretty(inspection).expect("the model always serializes");
    add("inspection.json".into(), json);
    files
}

/// Lookups over the model for the renderers.
pub(crate) struct Ctx<'a> {
    pub i: &'a Inspection,
    pub views: &'a Views,
    pub units: HashMap<&'a str, &'a UnitInfo>,
    pub instances: HashMap<&'a str, &'a InstanceInfo>,
    displays: HashMap<&'a str, &'a str>,
}

impl<'a> Ctx<'a> {
    fn new(i: &'a Inspection, views: &'a Views) -> Self {
        let mut displays: HashMap<&str, &str> = HashMap::new();
        let mut add = |id: &'a String, display: &'a String| {
            displays.insert(id, display);
        };
        i.entries.iter().for_each(|x| add(&x.id, &x.display));
        i.units.iter().for_each(|x| add(&x.id, &x.display));
        i.instances.iter().for_each(|x| add(&x.id, &x.display));
        i.functions.iter().for_each(|x| add(&x.id, &x.display));
        i.constants.iter().for_each(|x| add(&x.id, &x.display));
        i.tables.iter().for_each(|x| add(&x.id, &x.display));
        i.host_inputs.iter().for_each(|x| add(&x.id, &x.display));
        Self {
            i,
            views,
            units: i.units.iter().map(|u| (u.id.as_str(), u)).collect(),
            instances: i.instances.iter().map(|x| (x.id.as_str(), x)).collect(),
            displays,
        }
    }

    /// Display name of any entity id; the id itself if unknown.
    pub fn display<'s>(&'s self, id: &'s str) -> &'s str {
        self.displays.get(id).copied().unwrap_or(id)
    }

    /// `entries/<entry-path>`, when an entry view family is on.
    pub fn entry_dir(&self, entry: &str) -> Option<&str> {
        self.i.paths.get(entry).map(String::as_str)
    }

    /// `functions/<fn-path>.dot`, when the dataflow view is on.
    pub fn function_dot(&self, function: &str) -> Option<&str> {
        self.i.paths.get(function).map(String::as_str)
    }

    /// The rendered `.svg` of a function's DAG, as a link relative to `from` (a file path).
    pub fn function_href(&self, from: &str, function: &str) -> Option<String> {
        let dot = self.function_dot(function)?;
        Some(relative(from, &svg_of(dot)))
    }

    /// The rendered `.svg` of a unit's main-function DAG, relative to `from`.
    pub fn unit_href(&self, from: &str, unit: &str) -> Option<String> {
        self.function_href(from, &self.units.get(unit)?.function)
    }
}

pub(crate) fn svg_of(dot: &str) -> String {
    format!("{}.svg", dot.strip_suffix(".dot").unwrap_or(dot))
}

/// `to` as a link relative to the directory of the file `from`. Both are root-relative with `/`.
/// `#` is percent-encoded so links don't turn into fragments.
pub(crate) fn relative(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = from.split('/').collect();
    let from_dirs = &from_dirs[..from_dirs.len() - 1];
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dirs
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<&str> = vec![".."; from_dirs.len() - common];
    parts.extend(&to_parts[common..]);
    parts.join("/").replace('#', "%23")
}

// ---------------------------------------------------------------------------------------------
// Path mapping (SPEC.md §7.1)
// ---------------------------------------------------------------------------------------------

/// Id → path for every emitted entity: entry ids → `entries/<p>` (a directory), function ids →
/// `functions/<p>.dot`, and unit ids → their main function's file.
fn paths(i: &Inspection, views: &Views) -> BTreeMap<String, String> {
    let functions: HashMap<&str, &FunctionInfo> =
        i.functions.iter().map(|f| (f.id.as_str(), f)).collect();
    let units: HashMap<&str, &UnitInfo> = i.units.iter().map(|u| (u.id.as_str(), u)).collect();
    let instances: HashMap<&str, &InstanceInfo> =
        i.instances.iter().map(|x| (x.id.as_str(), x)).collect();

    let unit_path = |u: &UnitInfo| match &functions[u.function.as_str()].canonical_name {
        Some(name) => name_components(name),
        None => vec!["anon".to_string(), format!("{:016x}", u.source_hash)],
    };

    let mut out = BTreeMap::new();

    if views.dataflow {
        let mut items: Vec<(String, Vec<String>)> = i
            .functions
            .iter()
            .map(|f| {
                let comps = match (&f.kind, &f.canonical_name) {
                    (crate::model::FunctionKind::Main, _) => unit_path(units[f.units[0].as_str()]),
                    (_, Some(name)) => name_components(name),
                    (_, None) => vec![
                        "anon".to_string(),
                        component(f.id.strip_prefix("fn:").unwrap_or(&f.id)),
                    ],
                };
                (f.id.clone(), comps)
            })
            .collect();
        let fn_paths = disambiguate(&mut items, "functions", ".dot");
        for u in &i.units {
            if let Some(p) = fn_paths.get(&u.function) {
                out.insert(u.id.clone(), p.clone());
            }
        }
        out.extend(fn_paths);
    }

    if views.entries || views.buffers {
        let mut items: Vec<(String, Vec<String>)> = i
            .entries
            .iter()
            .map(|e| {
                let inst = instances[e.instance.as_str()];
                let unit = units[inst.unit.as_str()];
                let mut comps = unit_path(unit);
                let g = &inst.grid;
                let mut suffix = format!("@{}", format::dims(g.dimensions));
                // Same disambiguation as the display name (SPEC.md §6.1).
                if inst.display.contains(" o(") {
                    let (o, p) = (g.scaled_origin, g.scaled_position);
                    suffix.push_str(&format!(
                        "@{}",
                        component(&format!("o{},{},{}p{},{},{}", o.0, o.1, o.2, p.0, p.1, p.2))
                    ));
                }
                if e.display != inst.display {
                    suffix.push_str(&format!("#{}", e.index));
                }
                comps.last_mut().unwrap().push_str(&suffix);
                (e.id.clone(), comps)
            })
            .collect();
        out.extend(disambiguate(&mut items, "entries", ""));
    }
    out
}

/// `ns:a/b` → `[ns, a, b]`, each component sanitized.
fn name_components(name: &str) -> Vec<String> {
    let (ns, rest) = match name.split_once(':') {
        Some((ns, rest)) => (Some(ns), rest),
        None => (None, name),
    };
    ns.into_iter()
        .chain(rest.split('/'))
        .map(component)
        .collect()
}

const DEVICE_NAMES: &[&str] = &[
    "con", "aux", "nul", "prn", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// One path component: characters outside `[A-Za-z0-9._-]` and leading dots become `_`;
/// Windows device names get a trailing `_` (before any extension).
fn component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dots = out.len() - out.trim_start_matches('.').len();
    out.replace_range(..dots, &"_".repeat(dots));
    if out.is_empty() {
        out.push('_');
    }
    let stem_len = out.find('.').unwrap_or(out.len());
    if DEVICE_NAMES.contains(&out[..stem_len].to_ascii_lowercase().as_str()) {
        out.insert(stem_len, '_');
    }
    out
}

/// Joins `root/<components><ext>`, resolving case-insensitive collisions: in (path, id) order,
/// later items get `_2`, `_3` … on the final component.
fn disambiguate(
    items: &mut [(String, Vec<String>)],
    root: &str,
    ext: &str,
) -> BTreeMap<String, String> {
    let join = |comps: &[String]| format!("{root}/{}{ext}", comps.join("/"));
    items.sort_by(|a, b| (join(&a.1), &a.0).cmp(&(join(&b.1), &b.0)));
    let mut taken = HashSet::new();
    let mut out = BTreeMap::new();
    for (id, comps) in items.iter() {
        let mut path = join(comps);
        let mut n = 1;
        while !taken.insert(path.to_lowercase()) {
            n += 1;
            let mut c = comps.clone();
            c.last_mut().unwrap().push_str(&format!("_{n}"));
            path = join(&c);
        }
        out.insert(id.clone(), path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_names_to_paths() {
        assert_eq!(
            name_components("minecraft:overworld/continents"),
            vec!["minecraft", "overworld", "continents"]
        );
        assert_eq!(component(".hidden"), "_hidden");
        assert_eq!(component("a b?"), "a_b_");
        assert_eq!(component("CON"), "CON_");
        assert_eq!(component("lpt1.x"), "lpt1_.x");
    }

    #[test]
    fn resolves_case_insensitive_collisions() {
        let mut items = vec![
            ("b".to_string(), vec!["Foo".to_string()]),
            ("a".to_string(), vec!["foo".to_string()]),
        ];
        let paths = disambiguate(&mut items, "functions", ".dot");
        assert_eq!(paths["b"], "functions/Foo.dot");
        assert_eq!(paths["a"], "functions/foo_2.dot");
    }

    #[test]
    fn makes_relative_links() {
        assert_eq!(
            relative("entries/t/a@4x4x4/dependencies.dot", "functions/t/a.svg"),
            "../../../functions/t/a.svg"
        );
        assert_eq!(
            relative("index.md", "entries/t/a#1/waves.dot"),
            "entries/t/a%231/waves.dot"
        );
        assert_eq!(relative("functions/t/a.dot", "functions/t/b.svg"), "b.svg");
    }
}
