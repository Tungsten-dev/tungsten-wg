use std::collections::HashSet;

use crate::spmt::{
    model::{Addr, DensityFunctionRef},
    pretty::{PrettyPrint, Printer},
};

pub struct DensityDAG<'m> {
    pub root: DensityFunctionRef<'m>,
}

impl<'m> PrettyPrint for DensityDAG<'m> {
    fn pretty(&self, p: &mut Printer) {
        let mut visited = HashSet::new();

        p.line("digraph DensityDAG {");
        p.indent();
        p.line("node [shape=box];");

        self.visit(&self.root, p, &mut visited);

        p.dedent();
        p.line("}");
    }
}

impl<'m> DensityDAG<'m> {
    fn visit(
        &self,
        func: &DensityFunctionRef<'m>,
        p: &mut Printer,
        visited: &mut HashSet<*const ()>,
    ) {
        let addr = func.addr();
        if !visited.insert(addr) {
            return;
        }

        // Stable node ID via printer cache
        let node_id = p.anon_name(*func, "df");

        let label = func.canonical_name.as_deref().unwrap_or(&node_id);

        p.line(&format!(
            r#"{id} [label="{label}"];"#,
            id = node_id,
            label = label
        ));

        for input in &func.density_inputs {
            let child = &input.density_function;

            let child_id = p.anon_name(*child, "df");
            // Emit edge (optionally labeled with variable name)
            p.line(&format!(r#"{from} -> {to}"#, from = node_id, to = child_id));

            self.visit(child, p, visited);
        }
    }
}
