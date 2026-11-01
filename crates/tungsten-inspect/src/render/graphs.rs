//! The DOT views (SPEC.md §7.3): per-entry dependency and wave graphs, the program call and
//! resource graphs, and per-function dataflow DAGs.

use std::collections::{BTreeMap, HashMap};

use super::Ctx;
use super::dot::{Attrs, Dot};
use super::format::{dims, grid_exact, grid_label, round};
use crate::model::{
    DagNode, DensityReadInfo, EntryInfo, FunctionDag, FunctionInfo, FunctionKind, Literal,
    NodeKind, RegionKind,
};

fn grey(attrs: &mut Attrs) {
    attrs.push(("color", "gray".into()));
    attrs.push(("fontcolor", "gray".into()));
}

fn href(attrs: &mut Attrs, link: Option<String>) {
    if let Some(link) = link {
        attrs.push(("href", link));
    }
}

/// `dependencies.dot`: producers at the bottom, one node per instance of the entry's closure.
pub(crate) fn dependencies(ctx: &Ctx, entry: &EntryInfo, path: &str) -> String {
    let mut g = Dot::digraph(&format!("dependencies {}", entry.display));
    g.attr("rankdir", "BT");
    g.attr("labelloc", "t");
    g.attr(
        "label",
        &format!(
            "dependencies: {}\n{} instances, {} waves, max width {}",
            entry.display,
            entry.instances.len(),
            entry.wave_count,
            entry.max_wave_width
        ),
    );
    g.defaults("node", vec![("shape", "box".into())]);

    for id in &entry.instances {
        let inst = ctx.instances[id.as_str()];
        let mut attrs: Attrs = vec![
            ("label", format!("{}\n{}", inst.display, inst.buffer)),
            ("tooltip", grid_exact(&inst.grid)),
        ];
        if *id == entry.instance {
            attrs.push(("style", "bold".into()));
            attrs.push(("penwidth", "2".into()));
        }
        href(&mut attrs, ctx.unit_href(path, &inst.unit));
        g.node(id, attrs);
    }
    for id in &entry.instances {
        for e in &ctx.instances[id.as_str()].inputs {
            let mut label = e.input.clone();
            if e.resampled {
                label.push_str(&format!("\nin {}", grid_label(&e.input_grid)));
            }
            let mut styles = Vec::new();
            let mut attrs: Attrs = vec![("tooltip", grid_exact(&e.input_grid))];
            if e.indexed {
                label.push_str(" [idx]");
                styles.push("dashed");
            }
            if e.resampled {
                styles.push("bold");
                attrs.push(("color", "blue".into()));
                attrs.push(("fontcolor", "blue".into()));
            }
            if !styles.is_empty() {
                attrs.push(("style", styles.join(",")));
            }
            attrs.insert(0, ("label", label));
            g.edge(&e.producer, id, attrs);
        }
    }
    g.finish()
}

/// `waves.dot`: one cluster per wave of the entry's pruned schedule, in execution order.
pub(crate) fn waves(ctx: &Ctx, entry: &EntryInfo, path: &str) -> String {
    let mut g = Dot::digraph(&format!("waves {}", entry.display));
    g.attr("rankdir", "LR");
    g.attr("labelloc", "t");
    g.attr(
        "label",
        &format!(
            "waves: {}\n{} waves, max width {}",
            entry.display, entry.wave_count, entry.max_wave_width
        ),
    );
    g.defaults("node", vec![("shape", "box".into())]);

    let mut wave_of: HashMap<&str, usize> = HashMap::new();
    for (w, wave) in entry.schedule.waves.iter().enumerate() {
        g.open(&format!("cluster_wave_{w}"));
        g.attr("label", &format!("wave {w} ({} instances)", wave.len()));
        g.attr("rank", "same");
        for id in wave {
            wave_of.insert(id, w);
            let inst = ctx.instances[id.as_str()];
            let mut attrs: Attrs = vec![("label", inst.display.clone())];
            if *id == entry.instance {
                attrs.push(("style", "bold".into()));
            }
            href(&mut attrs, ctx.unit_href(path, &inst.unit));
            g.node(id, attrs);
        }
        g.close();
    }
    for id in &entry.schedule.steps {
        for p in &ctx.instances[id.as_str()].depends_on {
            let mut attrs: Attrs = vec![];
            if wave_of[p.as_str()] + 1 < wave_of[id.as_str()] {
                attrs.push(("style", "dashed".into()));
                attrs.push(("color", "gray".into()));
            }
            g.edge(p, id, attrs);
        }
    }
    g.finish()
}

/// `program/call_graph.dot`: every function; main functions are boxes, helpers ellipses.
pub(crate) fn call_graph(ctx: &Ctx, path: &str) -> String {
    let mut g = Dot::digraph("call graph");
    g.attr("labelloc", "t");
    g.attr(
        "label",
        &format!(
            "call graph: {} functions, {} call edges",
            ctx.i.functions.len(),
            ctx.i.call_graph.edges.len()
        ),
    );
    for f in &ctx.i.functions {
        let mut label = f.display.clone();
        let shape = match f.kind {
            FunctionKind::Main => "box",
            FunctionKind::Helper => {
                if f.units.len() > 1 {
                    let users: Vec<&str> = f.units.iter().map(|u| ctx.display(u)).collect();
                    label.push_str(&format!("\nused by: {}", users.join(", ")));
                }
                "ellipse"
            }
        };
        let mut attrs: Attrs = vec![("label", label), ("shape", shape.into())];
        if !f.reachable {
            grey(&mut attrs);
        }
        href(&mut attrs, ctx.function_href(path, &f.id));
        g.node(&f.id, attrs);
    }
    for e in &ctx.i.call_graph.edges {
        let attrs: Attrs = if e.count > 1 {
            vec![("label", format!("×{}", e.count))]
        } else {
            vec![]
        };
        g.edge(&e.from, &e.to, attrs);
    }
    g.finish()
}

/// `program/resources.dot`: units ↔ tables, host inputs and constants; constant → constant.
pub(crate) fn resources(ctx: &Ctx, path: &str) -> String {
    let mut g = Dot::digraph("resources");
    g.attr("rankdir", "LR");
    g.attr("labelloc", "t");
    g.attr(
        "label",
        &format!(
            "resources: {} tables, {} host inputs, {} constants",
            ctx.i.tables.len(),
            ctx.i.host_inputs.len(),
            ctx.i.constants.len()
        ),
    );

    let mut users: Vec<&str> = ctx
        .i
        .resources
        .uses
        .iter()
        .map(|u| u.unit.as_str())
        .collect();
    users.sort_by_key(|u| (ctx.display(u), *u));
    users.dedup();
    for u in users {
        let unit = ctx.units[u];
        let mut attrs: Attrs = vec![("label", unit.display.clone()), ("shape", "box".into())];
        if !unit.reachable {
            grey(&mut attrs);
        }
        href(&mut attrs, ctx.unit_href(path, u));
        g.node(u, attrs);
    }

    let mut resource = |id: &str, label: String, shape: &str, reachable: bool, users: usize| {
        let mut attrs: Attrs = vec![("shape", shape.into())];
        if users > 1 {
            attrs.push(("label", format!("{label}\n×{users} users")));
            attrs.push(("penwidth", "2".into()));
            attrs.push(("color", "darkorange".into()));
        } else {
            attrs.push(("label", label));
        }
        if !reachable {
            grey(&mut attrs);
        }
        g.node(id, attrs);
    };
    for t in &ctx.i.tables {
        resource(
            &t.id,
            t.display.clone(),
            "cylinder",
            t.reachable,
            t.users.len(),
        );
    }
    for h in &ctx.i.host_inputs {
        let label = format!("{}: {}", h.display, h.ty);
        resource(&h.id, label, "invhouse", h.reachable, h.users.len());
    }
    for c in &ctx.i.constants {
        let label = format!("{} = {}", c.display, c.value);
        resource(&c.id, label, "note", c.reachable, c.users.len());
    }

    for u in &ctx.i.resources.uses {
        g.edge(&u.unit, &u.resource, vec![]);
    }
    for d in &ctx.i.resources.constant_dependencies {
        g.edge(&d.constant, &d.depends_on, vec![("style", "dashed".into())]);
    }
    g.finish()
}

/// `functions/<fn>.dot`: the dataflow DAG, with control regions as nested clusters.
pub(crate) fn function(ctx: &Ctx, f: &FunctionInfo, dag: &FunctionDag, path: &str) -> String {
    let mut g = Dot::digraph(&format!("dataflow {}", f.display));
    g.attr("compound", "true");
    g.attr("labelloc", "t");
    let s = &dag.summary;
    let mut label = format!(
        "dataflow: {}\n{} nodes, {} merged by CSE",
        f.display,
        s.nodes_after_cse,
        s.nodes_before_cse - s.nodes_after_cse
    );
    if !f.reachable {
        label.push_str("\n(unreachable)");
    }
    g.attr("label", &label);

    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for r in &dag.regions {
        if let Some(p) = r.parent {
            children.entry(p).or_default().push(r.id);
        }
    }
    let mut nodes_in: BTreeMap<usize, Vec<&DagNode>> = BTreeMap::new();
    for n in &dag.nodes {
        nodes_in.entry(n.region).or_default().push(n);
    }
    let dead = |mut r: usize| loop {
        let region = &dag.regions[r];
        if region.kind == RegionKind::Unreachable {
            break true;
        }
        match region.parent {
            Some(p) => r = p,
            None => break false,
        }
    };

    let renderer = DagRenderer { ctx, f, path };
    fn emit(
        g: &mut Dot,
        r: &DagRenderer,
        dag: &FunctionDag,
        region: usize,
        children: &BTreeMap<usize, Vec<usize>>,
        nodes_in: &BTreeMap<usize, Vec<&DagNode>>,
        dead: &dyn Fn(usize) -> bool,
    ) {
        if region != 0 {
            g.open(&format!("cluster_r{region}"));
            g.attr("label", &region_label(&dag.regions[region].kind));
            if dead(region) {
                g.attr("color", "gray");
                g.attr("fontcolor", "gray");
            }
            // An anchor keeps empty regions visible and gives `lhead` edges a target.
            g.node(
                &format!("r{region}"),
                vec![
                    ("shape", "point".into()),
                    ("style", "invis".into()),
                    ("width", "0".into()),
                ],
            );
        }
        for n in nodes_in.get(&region).into_iter().flatten() {
            let mut attrs = r.node_attrs(n);
            if dead(region) {
                grey(&mut attrs);
            }
            g.node(&format!("n{}", n.id), attrs);
        }
        for c in children.get(&region).into_iter().flatten() {
            emit(g, r, dag, *c, children, nodes_in, dead);
        }
        if region != 0 {
            g.close();
        }
    }
    emit(&mut g, &renderer, dag, 0, &children, &nodes_in, &dead);

    // Each `if` condition points at its branches.
    for n in &dag.nodes {
        if n.kind != NodeKind::Cond {
            continue;
        }
        for c in children.get(&n.region).into_iter().flatten() {
            if matches!(dag.regions[*c].kind, RegionKind::Then | RegionKind::Else) {
                g.edge(
                    &format!("n{}", n.id),
                    &format!("r{c}"),
                    vec![
                        ("lhead", format!("cluster_r{c}")),
                        ("style", "dotted".into()),
                        ("arrowhead", "empty".into()),
                    ],
                );
            }
        }
    }

    for e in &dag.edges {
        let mut attrs: Attrs = vec![];
        let target = &dag.nodes[e.to].kind;
        let unary = matches!(target, NodeKind::Unary { .. } | NodeKind::Cast { .. });
        if !unary && !matches!(e.port.as_str(), "value" | "cond") {
            attrs.push(("label", e.port.clone()));
        }
        if e.loop_carried {
            attrs.push(("color", "red".into()));
            attrs.push(("style", "dashed".into()));
            attrs.push(("constraint", "false".into()));
        }
        g.edge(&format!("n{}", e.from), &format!("n{}", e.to), attrs);
    }
    g.finish()
}

fn region_label(kind: &RegionKind) -> String {
    match kind {
        RegionKind::Body => "body".into(),
        RegionKind::If => "if".into(),
        RegionKind::Then => "then".into(),
        RegionKind::Else => "else".into(),
        RegionKind::While => "while".into(),
        RegionKind::Repeat { count } => format!("repeat ×{count}"),
        RegionKind::Unreachable => "unreachable".into(),
    }
}

struct DagRenderer<'c, 'a> {
    ctx: &'c Ctx<'a>,
    f: &'c FunctionInfo,
    path: &'c str,
}

impl DagRenderer<'_, '_> {
    fn node_attrs(&self, n: &DagNode) -> Attrs {
        let ctx = self.ctx;
        let mut attrs: Attrs = Vec::new();
        let (label, shape) = match &n.kind {
            NodeKind::Param { var } => (var.clone(), "invhouse"),
            NodeKind::Free { var } => {
                attrs.push(("style", "dashed".into()));
                attrs.push((
                    "tooltip",
                    "free variable (no parameter or def reaches it)".into(),
                ));
                (var.clone(), "invhouse")
            }
            NodeKind::Literal(l) => {
                let (label, exact) = literal(l);
                attrs.push(("tooltip", exact));
                (label, "plaintext")
            }
            NodeKind::Constant { constant } => (ctx.display(constant).to_string(), "plaintext"),
            NodeKind::HostInput { host_input } => {
                (format!("host {}", ctx.display(host_input)), "invhouse")
            }
            NodeKind::PermTable { table } => (format!("table {}", ctx.display(table)), "invhouse"),
            NodeKind::DensityRead(info) => (self.density_read(info, &mut attrs), "box3d"),
            NodeKind::IndexedDensityRead(info) => {
                let label = self.density_read(info, &mut attrs);
                (format!("{label}\n[idx]"), "box3d")
            }
            NodeKind::Binary { op } | NodeKind::Unary { op } => (op.clone(), "ellipse"),
            NodeKind::Field { name } => (format!(".{name}"), "ellipse"),
            NodeKind::Index => ("[ ]".into(), "ellipse"),
            NodeKind::Construct { ty } => (ty.clone(), "ellipse"),
            NodeKind::ConstructExtern { ty, fields } => {
                (format!("{ty}{{ {} }}", fields.join(", ")), "ellipse")
            }
            NodeKind::ArrayLiteral => ("[..]".into(), "ellipse"),
            NodeKind::Cast { ty } => (format!("as {ty}"), "ellipse"),
            NodeKind::Extern {
                name,
                parameter_types,
                return_type,
            } => {
                attrs.push((
                    "tooltip",
                    format!("({}) -> {return_type}", parameter_types.join(", ")),
                ));
                (format!("extern {name}"), "ellipse")
            }
            NodeKind::Call { function } => {
                href(&mut attrs, ctx.function_href(self.path, function));
                (ctx.display(function).to_string(), "component")
            }
            NodeKind::Def { var, version } => (format!("{var}#{version}"), "note"),
            NodeKind::Return => ("return".into(), "doubleoctagon"),
            NodeKind::Cond => ("cond".into(), "diamond"),
        };
        attrs.insert(0, ("shape", shape.into()));
        attrs.insert(0, ("label", label));
        attrs
    }

    /// Label for a density read; adds the producer link and the exact grid as tooltip.
    fn density_read(&self, info: &DensityReadInfo, attrs: &mut Attrs) -> String {
        let ctx = self.ctx;
        let mut label = if self.f.units.contains(&info.unit) {
            info.input.clone()
        } else {
            format!("{}::{}", ctx.display(&info.unit), info.input)
        };
        label.push_str(&format!(
            "\n← {} @ {}",
            ctx.display(&info.producer_unit),
            dims(info.grid.dimensions)
        ));
        attrs.push(("tooltip", grid_exact(&info.grid)));
        href(attrs, ctx.unit_href(self.path, &info.producer_unit));
        label
    }
}

/// (rounded label, exact value).
fn literal(l: &Literal) -> (String, String) {
    match l {
        Literal::Float(x) => (format!("{}f", round(*x as f64)), format!("{x:?}f")),
        Literal::Double(x) => (round(*x), format!("{x:?}")),
        Literal::Byte(x) => (format!("{x}b"), format!("{x}b")),
        Literal::Int(x) => (x.to_string(), x.to_string()),
        Literal::Long(x) => (format!("{x}L"), format!("{x}L")),
        Literal::Bool(x) => (x.to_string(), x.to_string()),
    }
}
