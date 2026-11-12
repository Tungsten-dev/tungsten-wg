/*
Program-wide facts the device and orchestration sides both need (`specs/workers.md` §5.3): the
fields of `InternalBuffers`, one array per pool element type, and how reads of them are checked.
*/

use std::collections::HashSet;

use tungsten_interface::naming::{NamingRules, unique};
use tungsten_interface::{BufferBounds, Interface};
use tungsten_ir::model::{Program, VariableType};
use tungsten_rust::pools::{type_stem, unit_return_type};

/// The pools of `InternalBuffers`, and how reads of them are checked.
#[derive(Debug, Clone)]
pub struct Pools {
    /// (element type, field name), in first-use order over the entries' workers.
    pub fields: Vec<(VariableType, String)>,
    pub bounds: BufferBounds,
}

impl Pools {
    /// One field per element type used by any worker or read by any density input, so dead code
    /// still compiles.
    pub fn new(program: &Program, interface: &Interface, bounds: BufferBounds) -> Self {
        let mut types: Vec<VariableType> = vec![];
        let mut add = |t: &VariableType| {
            if !types.contains(t) {
                types.push(t.clone());
            }
        };
        for e in 0..program.entry_points.len() {
            for pool in &interface.worker(e).pools {
                add(&pool.element);
            }
        }
        for (_, d) in program.density_inputs.iter() {
            add(unit_return_type(program, d.density_function));
        }
        let mut used: HashSet<String> = NamingRules::java().used_set();
        let fields = types
            .into_iter()
            .map(|t| {
                let name = unique(&mut used, format!("{}s", type_stem(&t)));
                (t, name)
            })
            .collect();
        Self { fields, bounds }
    }

    /// The `InternalBuffers` field holding elements of type `t`.
    pub fn field(&self, t: &VariableType) -> &str {
        self.fields
            .iter()
            .find(|(e, _)| e == t)
            .map(|(_, n)| n.as_str())
            .unwrap_or_else(|| panic!("no pool for element type {t:?}"))
    }
}
