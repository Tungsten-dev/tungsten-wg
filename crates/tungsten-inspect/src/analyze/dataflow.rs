//! Dataflow DAG of one function (SPEC.md §6.6).
//!
//! Two passes over the structured body:
//! 1. Reaching definitions with implicit φ, iterated to a fixpoint per loop. Results are keyed
//!    by syntactic occurrence (the address of the `Variable` expression), so re-walking a loop
//!    only grows the def sets. A def that reaches a read only after the read's first visit came
//!    in over a back edge: that edge is loop-carried.
//! 2. One walk in program order that builds nodes, hash-consed per function ("share
//!    everything"), using the final def sets from pass 1.
//!
//! Occurrence addresses never reach the output; node, region and def ids are counters.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use tungsten_interface::Instance;
use tungsten_ir::model::{
    BinaryOperator, ComputeUnit, Expression, Function, Ref, Statement, UnaryOperator, Var,
};

use super::{Ctx, grid_of_input};
use crate::model::{
    DagEdge, DagNode, DagSummary, DensityReadInfo, FunctionDag, Literal, NodeKind, Region,
    RegionKind,
};

/// Structured control flow converges in a few iterations; more means a bug here.
const MAX_LOOP_ITERATIONS: usize = 16;

pub(crate) fn build(ctx: &Ctx, f: Ref<Function>) -> FunctionDag {
    let func = &ctx.program.functions[f];

    let mut defs = Defs::default();
    defs.number(&func.body);

    let mut rd = ReachingDefs {
        defs: &defs,
        reads: HashMap::new(),
        breaks: Vec::new(),
    };
    let mut env = Env::new();
    for (i, p) in func.parameters.iter().enumerate() {
        env.insert(*p, BTreeSet::from([Src::Param(i)]));
    }
    rd.block(&func.body, State { env, live: true });
    let reads = rd.reads;

    let mut b = DagBuilder {
        ctx,
        f,
        defs: &defs,
        reads: &reads,
        regions: vec![Region {
            id: 0,
            kind: RegionKind::Body,
            parent: None,
        }],
        nodes: Vec::new(),
        edges: Vec::new(),
        edge_index: HashMap::new(),
        cse: HashMap::new(),
        def_nodes: HashMap::new(),
        param_nodes: Vec::new(),
        before_cse: 0,
        density_reads: BTreeMap::new(),
        helper_calls: BTreeMap::new(),
        extern_calls: BTreeMap::new(),
    };
    for p in &func.parameters {
        let var = b.var_name(*p);
        let n = b.node(NodeKind::Param { var }, vec![], 0, true);
        b.param_nodes.push(n);
    }
    b.block(&func.body, 0);

    let summary = DagSummary {
        nodes_before_cse: b.before_cse,
        nodes_after_cse: b.nodes.len(),
        density_reads: b.density_reads,
        helper_calls: b.helper_calls,
        extern_calls: b.extern_calls,
    };
    FunctionDag {
        regions: b.regions,
        nodes: b.nodes,
        edges: b.edges,
        summary,
    }
}

// ---------------------------------------------------------------------------------------------
// Pass 1: reaching definitions
// ---------------------------------------------------------------------------------------------

/// Where a variable's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Src {
    /// Index into the function's parameters.
    Param(usize),
    /// Index of the `Assign` in statement (pre-)order.
    Def(usize),
}

type Env = HashMap<Ref<Var>, BTreeSet<Src>>;

fn union_into(dst: &mut Env, src: &Env) {
    for (v, s) in src {
        dst.entry(*v).or_default().extend(s.iter().copied());
    }
}

#[derive(Clone)]
struct State {
    env: Env,
    /// False after a `return` or `break`: this path doesn't continue, so it doesn't take part
    /// in merges.
    live: bool,
}

/// Every `Assign`, numbered in statement order, with its per-variable version (from 1).
#[derive(Default)]
struct Defs {
    index: HashMap<*const Statement, usize>,
    /// By def index: (variable, version).
    info: Vec<(Ref<Var>, usize)>,
    versions: HashMap<Ref<Var>, usize>,
}

impl Defs {
    fn number(&mut self, stmts: &[Statement]) {
        for s in stmts {
            match s {
                Statement::Assign { target, .. } => {
                    let v = self.versions.entry(*target).or_default();
                    *v += 1;
                    self.index.insert(s as *const _, self.info.len());
                    self.info.push((*target, *v));
                }
                Statement::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.number(then_branch);
                    self.number(else_branch);
                }
                Statement::While { body, .. } | Statement::Repeat { body, .. } => self.number(body),
                Statement::Return(_) | Statement::Break => {}
            }
        }
    }
}

/// The def sets of one `Variable` occurrence.
#[derive(Default)]
struct Read {
    /// At its first visit: every forward path has been walked by then.
    first: BTreeSet<Src>,
    /// After the fixpoint.
    all: BTreeSet<Src>,
}

struct ReachingDefs<'d> {
    defs: &'d Defs,
    reads: HashMap<*const Expression, Read>,
    /// Envs at each `break`, per enclosing loop.
    breaks: Vec<Vec<Env>>,
}

impl ReachingDefs<'_> {
    fn block(&mut self, stmts: &[Statement], mut st: State) -> State {
        for s in stmts {
            st = self.stmt(s, st);
        }
        st
    }

    fn stmt(&mut self, s: &Statement, mut st: State) -> State {
        match s {
            Statement::Assign { target, value } => {
                self.expr(value, &st.env);
                let d = self.defs.index[&(s as *const _)];
                st.env.insert(*target, BTreeSet::from([Src::Def(d)]));
                st
            }
            Statement::Return(e) => {
                self.expr(e, &st.env);
                st.live = false;
                st
            }
            Statement::Break => {
                if let Some(b) = self.breaks.last_mut() {
                    b.push(st.env.clone());
                }
                st.live = false;
                st
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(condition, &st.env);
                let t = self.block(then_branch, st.clone());
                let e = self.block(else_branch, st.clone());
                let mut env = Env::new();
                let any_live = t.live || e.live;
                for branch in [&t, &e] {
                    // Dead branches only count when nothing is live (unreachable code still
                    // needs an env to be drawn with).
                    if branch.live || !any_live {
                        union_into(&mut env, &branch.env);
                    }
                }
                State {
                    env,
                    live: st.live && any_live,
                }
            }
            Statement::While { condition, body } => self.looped(Some(condition), body, st),
            Statement::Repeat { body, .. } => self.looped(None, body, st),
        }
    }

    /// `head = before ∪ end of body`, iterated until it stops changing. After the loop:
    /// `head ∪ every break`.
    fn looped(&mut self, condition: Option<&Expression>, body: &[Statement], st: State) -> State {
        let mut head = st.env.clone();
        for _ in 0..MAX_LOOP_ITERATIONS {
            if let Some(c) = condition {
                self.expr(c, &head);
            }
            self.breaks.push(Vec::new());
            let end = self.block(
                body,
                State {
                    env: head.clone(),
                    live: true,
                },
            );
            let breaks = self.breaks.pop().unwrap();

            let mut next = head.clone();
            if end.live {
                union_into(&mut next, &end.env);
            }
            if next == head {
                for b in &breaks {
                    union_into(&mut head, b);
                }
                return State {
                    env: head,
                    live: st.live,
                };
            }
            head = next;
        }
        panic!("reaching definitions did not converge in {MAX_LOOP_ITERATIONS} iterations");
    }

    fn expr(&mut self, e: &Expression, env: &Env) {
        if let Expression::Variable(v) = e {
            let srcs = env.get(v).cloned().unwrap_or_default();
            match self.reads.entry(e as *const _) {
                std::collections::hash_map::Entry::Occupied(mut o) => o.get_mut().all.extend(srcs),
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(Read {
                        first: srcs.clone(),
                        all: srcs,
                    });
                }
            }
        }
        for c in super::expr_children(e) {
            self.expr(c, env);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pass 2: nodes
// ---------------------------------------------------------------------------------------------

/// The value of an expression: one node, or (for a variable read) every reaching def, each
/// with its loop-carried flag.
type Value = Vec<(usize, bool)>;

/// Hash-consing key: kind, then per port the sorted set of input nodes.
type Key = (String, Vec<(String, Vec<usize>)>);

struct DagBuilder<'a> {
    ctx: &'a Ctx<'a>,
    f: Ref<Function>,
    defs: &'a Defs,
    reads: &'a HashMap<*const Expression, Read>,
    regions: Vec<Region>,
    nodes: Vec<DagNode>,
    edges: Vec<DagEdge>,
    edge_index: HashMap<(usize, usize, String), usize>,
    cse: HashMap<Key, usize>,
    /// Def index → node. Created on first use, which can be before the `Assign` (a back edge).
    def_nodes: HashMap<usize, usize>,
    param_nodes: Vec<usize>,
    before_cse: usize,
    density_reads: BTreeMap<String, usize>,
    helper_calls: BTreeMap<String, usize>,
    extern_calls: BTreeMap<String, usize>,
}

impl DagBuilder<'_> {
    fn region(&mut self, kind: RegionKind, parent: usize) -> usize {
        let id = self.regions.len();
        self.regions.push(Region {
            id,
            kind,
            parent: Some(parent),
        });
        id
    }

    /// Lowest common ancestor in the region tree.
    fn lca(&self, a: usize, b: usize) -> usize {
        let mut ancestors = vec![a];
        while let Some(p) = self.regions[*ancestors.last().unwrap()].parent {
            ancestors.push(p);
        }
        let mut r = b;
        loop {
            if ancestors.contains(&r) {
                return r;
            }
            r = self.regions[r].parent.expect("regions share the root");
        }
    }

    fn edge(&mut self, from: usize, to: usize, port: &str, loop_carried: bool) {
        let key = (from, to, port.to_string());
        if let Some(i) = self.edge_index.get(&key) {
            self.edges[*i].loop_carried |= loop_carried;
            return;
        }
        self.edge_index.insert(key, self.edges.len());
        self.edges.push(DagEdge {
            from,
            to,
            port: port.to_string(),
            loop_carried,
        });
    }

    /// Creates (or with `cse`, finds) a node with these inputs, occurring in `region`.
    fn node(
        &mut self,
        kind: NodeKind,
        inputs: Vec<(String, Value)>,
        region: usize,
        cse: bool,
    ) -> usize {
        self.before_cse += 1;
        let key: Key = (
            kind_key(&kind),
            inputs
                .iter()
                .map(|(port, v)| {
                    let set: BTreeSet<usize> = v.iter().map(|(n, _)| *n).collect();
                    (port.clone(), set.into_iter().collect())
                })
                .collect(),
        );
        let existing = if cse {
            self.cse.get(&key).copied()
        } else {
            None
        };
        let id = match existing {
            Some(id) => {
                self.nodes[id].region = self.lca(self.nodes[id].region, region);
                id
            }
            None => {
                let id = self.nodes.len();
                self.nodes.push(DagNode { id, kind, region });
                if cse {
                    self.cse.insert(key, id);
                }
                id
            }
        };
        for (port, v) in inputs {
            for (from, carried) in v {
                self.edge(from, id, &port, carried);
            }
        }
        id
    }

    fn def_node(&mut self, d: usize) -> usize {
        if let Some(n) = self.def_nodes.get(&d) {
            return *n;
        }
        let (var, version) = self.defs.info[d];
        let id = self.nodes.len();
        self.nodes.push(DagNode {
            id,
            kind: NodeKind::Def {
                var: self.var_name(var),
                version,
            },
            // Set when the `Assign` is walked.
            region: 0,
        });
        self.def_nodes.insert(d, id);
        id
    }

    fn var_name(&self, v: Ref<Var>) -> String {
        self.ctx.var_names[&self.f][&v].clone()
    }

    fn block(&mut self, stmts: &[Statement], region: usize) {
        let mut region = region;
        let mut dead = false;
        for s in stmts {
            if dead {
                region = self.region(RegionKind::Unreachable, region);
                dead = false;
            }
            self.stmt(s, region);
            if matches!(s, Statement::Return(_) | Statement::Break)
                && self.regions[region].kind != RegionKind::Unreachable
            {
                dead = true;
            }
        }
    }

    fn stmt(&mut self, s: &Statement, region: usize) {
        match s {
            Statement::Assign { value, .. } => {
                let v = self.expr(value, region);
                self.before_cse += 1;
                let d = self.def_node(self.defs.index[&(s as *const _)]);
                self.nodes[d].region = region;
                for (from, carried) in v {
                    self.edge(from, d, "value", carried);
                }
            }
            Statement::Return(e) => {
                let v = self.expr(e, region);
                self.node(NodeKind::Return, vec![("value".into(), v)], region, false);
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let r = self.region(RegionKind::If, region);
                let c = self.expr(condition, r);
                self.node(NodeKind::Cond, vec![("cond".into(), c)], r, false);
                let t = self.region(RegionKind::Then, r);
                self.block(then_branch, t);
                let e = self.region(RegionKind::Else, r);
                self.block(else_branch, e);
            }
            Statement::While { condition, body } => {
                let r = self.region(RegionKind::While, region);
                let c = self.expr(condition, r);
                self.node(NodeKind::Cond, vec![("cond".into(), c)], r, false);
                self.block(body, r);
            }
            Statement::Repeat { count, body } => {
                let r = self.region(RegionKind::Repeat { count: *count }, region);
                self.block(body, r);
            }
            Statement::Break => {}
        }
    }

    fn args(&mut self, args: &[&Expression], region: usize) -> Vec<(String, Value)> {
        args.iter()
            .enumerate()
            .map(|(i, a)| (i.to_string(), self.expr(a, region)))
            .collect()
    }

    fn leaf(&mut self, kind: NodeKind, region: usize) -> Value {
        vec![(self.node(kind, vec![], region, true), false)]
    }

    fn op(&mut self, kind: NodeKind, inputs: Vec<(String, Value)>, region: usize) -> Value {
        vec![(self.node(kind, inputs, region, true), false)]
    }

    fn expr(&mut self, e: &Expression, region: usize) -> Value {
        let ty = |t| format!("{:?}", t);
        match e {
            Expression::Variable(v) => {
                let read = &self.reads[&(e as *const _)];
                if read.all.is_empty() {
                    let var = self.var_name(*v);
                    return self.leaf(NodeKind::Free { var }, region);
                }
                read.all
                    .iter()
                    .map(|src| {
                        let n = match src {
                            Src::Param(i) => self.param_nodes[*i],
                            Src::Def(d) => self.def_node(*d),
                        };
                        (n, !read.first.contains(src))
                    })
                    .collect()
            }
            Expression::Float(x) => self.leaf(NodeKind::Literal(Literal::Float(*x)), region),
            Expression::Double(x) => self.leaf(NodeKind::Literal(Literal::Double(*x)), region),
            Expression::Byte(x) => self.leaf(NodeKind::Literal(Literal::Byte(*x)), region),
            Expression::Int(x) => self.leaf(NodeKind::Literal(Literal::Int(*x)), region),
            Expression::Long(x) => self.leaf(NodeKind::Literal(Literal::Long(*x)), region),
            Expression::Bool(x) => self.leaf(NodeKind::Literal(Literal::Bool(*x)), region),
            Expression::Constant(c) => {
                let constant = self.ctx.constants[c].id.clone();
                self.leaf(NodeKind::Constant { constant }, region)
            }
            Expression::HostInput(h) => {
                let host_input = self.ctx.host_inputs[h].id.clone();
                self.leaf(NodeKind::HostInput { host_input }, region)
            }
            Expression::PermutationTable(t) => {
                let table = self.ctx.tables[t].id.clone();
                self.leaf(NodeKind::PermTable { table }, region)
            }
            Expression::DensityVariable(r, idx) => {
                let info = self.density_read(*r);
                *self
                    .density_reads
                    .entry(format!("{}/{}", info.unit, info.input))
                    .or_default() += 1;
                match idx {
                    None => self.leaf(NodeKind::DensityRead(info), region),
                    Some(i) => {
                        let i = self.expr(i, region);
                        self.op(
                            NodeKind::IndexedDensityRead(info),
                            vec![("idx".into(), i)],
                            region,
                        )
                    }
                }
            }
            Expression::FunctionCall {
                function,
                parameters,
            } => {
                let function = self.ctx.function_id(*function);
                *self.helper_calls.entry(function.clone()).or_default() += 1;
                let args = self.args(&parameters.iter().collect::<Vec<_>>(), region);
                self.op(NodeKind::Call { function }, args, region)
            }
            Expression::ExternCall {
                function_name,
                parameters,
                parameter_types,
                return_type,
            } => {
                *self.extern_calls.entry(function_name.clone()).or_default() += 1;
                let args = self.args(&parameters.iter().collect::<Vec<_>>(), region);
                let kind = NodeKind::Extern {
                    name: function_name.clone(),
                    parameter_types: parameter_types.iter().map(ty).collect(),
                    return_type: ty(return_type),
                };
                self.op(kind, args, region)
            }
            Expression::BinaryOp { op, left, right } => {
                let l = self.expr(left, region);
                let r = self.expr(right, region);
                let kind = NodeKind::Binary {
                    op: binary_symbol(*op).to_string(),
                };
                self.op(kind, vec![("l".into(), l), ("r".into(), r)], region)
            }
            Expression::UnaryOp { op, operand } => {
                let v = self.expr(operand, region);
                let kind = NodeKind::Unary {
                    op: unary_symbol(*op).to_string(),
                };
                self.op(kind, vec![("0".into(), v)], region)
            }
            Expression::Field { base, field, .. } => {
                let b = self.expr(base, region);
                self.op(
                    NodeKind::Field {
                        name: field.clone(),
                    },
                    vec![("base".into(), b)],
                    region,
                )
            }
            Expression::ArrayAccess { array, index } => {
                let a = self.expr(array, region);
                let i = self.expr(index, region);
                self.op(
                    NodeKind::Index,
                    vec![("base".into(), a), ("idx".into(), i)],
                    region,
                )
            }
            Expression::Construct { t, args } => {
                let args = self.args(&args.iter().collect::<Vec<_>>(), region);
                self.op(NodeKind::Construct { ty: ty(t) }, args, region)
            }
            Expression::ConstructExtern { t, args } => {
                let fields = args.iter().map(|(n, _)| n.to_string()).collect();
                let args = self.args(&args.iter().map(|(_, a)| a).collect::<Vec<_>>(), region);
                self.op(
                    NodeKind::ConstructExtern { ty: ty(t), fields },
                    args,
                    region,
                )
            }
            Expression::ArrayLiteral(items) => {
                let args = self.args(&items.iter().collect::<Vec<_>>(), region);
                self.op(NodeKind::ArrayLiteral, args, region)
            }
            Expression::ExplicitCast { to, expr } => {
                let v = self.expr(expr, region);
                self.op(NodeKind::Cast { ty: ty(to) }, vec![("0".into(), v)], region)
            }
        }
    }

    fn density_read(&self, r: Ref<tungsten_ir::model::DensityInput>) -> DensityReadInfo {
        let input = &self.ctx.inputs[&r];
        let ComputeUnit::DensityFunction { density_inputs, .. } =
            &self.ctx.program.compute_units[input.unit];
        let di = &density_inputs[r];
        DensityReadInfo {
            input: input.name.clone(),
            unit: self.ctx.unit_id(input.unit),
            producer_unit: self.ctx.unit_id(di.density_function),
            producer_instance: self
                .ctx
                .instances
                .get(&Instance::from_input(di))
                .map(|n| n.id.clone()),
            grid: grid_of_input(di),
        }
    }
}

/// Literals compare by type and bits (`0.0` and `-0.0` differ, NaNs by payload); everything
/// else by its payload.
fn kind_key(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Literal(l) => match l {
            Literal::Float(x) => format!("lit:f32:{:x}", x.to_bits()),
            Literal::Double(x) => format!("lit:f64:{:x}", x.to_bits()),
            Literal::Byte(x) => format!("lit:i8:{x}"),
            Literal::Int(x) => format!("lit:i32:{x}"),
            Literal::Long(x) => format!("lit:i64:{x}"),
            Literal::Bool(x) => format!("lit:bool:{x}"),
        },
        other => format!("{other:?}"),
    }
}

pub(crate) fn binary_symbol(op: BinaryOperator) -> &'static str {
    match op {
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::And => "&&",
        BinaryOperator::Or => "||",
        BinaryOperator::ShiftLeft => "<<",
        BinaryOperator::ShiftRight => ">>",
        BinaryOperator::LogicalShiftRight => ">>>",
        BinaryOperator::BitOr => "|",
        BinaryOperator::BitAnd => "&",
        BinaryOperator::BitXor => "^",
    }
}

pub(crate) fn unary_symbol(op: UnaryOperator) -> &'static str {
    match op {
        UnaryOperator::Negate => "-",
        UnaryOperator::Not => "!",
        UnaryOperator::Ref(false) => "&",
        UnaryOperator::Ref(true) => "&mut",
    }
}
