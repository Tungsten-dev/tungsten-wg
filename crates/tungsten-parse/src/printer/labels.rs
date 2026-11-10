//! Label assignment (SPEC §5.2). One table serves the printer, error messages and inspect.

use std::collections::HashMap;

use tungsten_ir::*;

use super::float::quote_name;
use crate::lexer::{is_ident, is_keyword};

#[derive(Debug, Clone, Default)]
pub struct Labels {
    functions: HashMap<Ref<Function>, String>,
    units: HashMap<Ref<ComputeUnit>, String>,
    constants: HashMap<Ref<Constant>, String>,
    hosts: HashMap<Ref<HostInput>, String>,
    tables: HashMap<Ref<PermutationTableInput>, String>,
    vars: HashMap<(Ref<Function>, Ref<Var>), String>,
    inputs: HashMap<Ref<DensityInput>, String>,
    input_units: HashMap<Ref<DensityInput>, Ref<ComputeUnit>>,
    entries: Vec<String>,
}

fn is_numbered(s: &str, prefix: char) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

fn plain(s: &str) -> bool {
    is_ident(s) && !is_keyword(s)
}

fn count<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut m = HashMap::new();
    for n in names {
        *m.entry(n).or_insert(0) += 1;
    }
    m
}

/// Labels for a `Name` arena: `x`, `x#N`, `x##N` (only for duplicated `Named`), `%N`.
fn name_labels<'a, T: 'a>(
    items: impl Iterator<Item = (Ref<T>, Option<&'a Name>)>,
) -> Vec<(Ref<T>, String)> {
    let items: Vec<_> = items.collect();
    let named = count(items.iter().filter_map(|(_, n)| match n {
        Some(Name::Named(s)) => Some(s.as_str()),
        _ => None,
    }));
    items
        .iter()
        .copied()
        .enumerate()
        .map(|(i, (r, n))| {
            let l = match n {
                Some(Name::Named(s)) if named[s.as_str()] > 1 => format!("{}##{i}", quote_name(s)),
                Some(Name::Named(s)) => quote_name(s),
                Some(Name::Prefixed(s)) => format!("{}#{i}", quote_name(s)),
                Some(Name::Anonymous) | None => format!("%{i}"),
            };
            (r, l)
        })
        .collect()
}

/// `fn name(&self, r: Ref<T>) -> &str` for each label map, panicking on a ref that isn't in it.
macro_rules! accessors {
    ($($name:ident($field:ident: $t:ty) $what:literal;)*) => {
        $(
            pub fn $name(&self, r: Ref<$t>) -> &str {
                self.$field
                    .get(&r)
                    .unwrap_or_else(|| panic!(concat!("dangling Ref: ", $what)))
            }
        )*
    };
}

impl Labels {
    /// Panics (naming the entity) if a `DensityInput.var` or an entry's unit doesn't resolve.
    pub fn new(p: &OptimizedProgram) -> Labels {
        let mut l = Labels::default();

        let fn_names = count(
            p.functions
                .values()
                .filter_map(|f| f.canonical_name.as_deref()),
        );
        for (i, (r, f)) in p.functions.iter().enumerate() {
            let s = match f.canonical_name.as_deref() {
                Some(n) if fn_names[n] > 1 => format!("{}#{i}", quote_name(n)),
                Some(n) => quote_name(n),
                None => format!("%{i}"),
            };
            l.functions.insert(r, s);
        }

        let const_names = count(p.constants.values().map(|c| c.name.as_str()));
        for (i, (r, c)) in p.constants.iter().enumerate() {
            let s = if const_names[c.name.as_str()] > 1 {
                format!("{}#{i}", quote_name(&c.name))
            } else {
                quote_name(&c.name)
            };
            l.constants.insert(r, s);
        }

        let hosts = p.host_inputs.iter().map(|(r, h)| {
            (
                r,
                match h {
                    HostInput::Static { name, .. } => Some(name),
                    HostInput::Dynamic => None,
                },
            )
        });
        for (r, s) in name_labels(hosts) {
            l.hosts.insert(r, s);
        }

        for (i, (r, _)) in p.permutation_tables.iter().enumerate() {
            l.tables.insert(r, format!("t{i}"));
        }

        let mut var_names: HashMap<Ref<Var>, Name> = HashMap::new();
        for (f, func) in p.functions.iter() {
            for (r, s) in name_labels(func.variables.iter().map(|(r, v)| (r, Some(&v.name)))) {
                l.vars.insert((f, r), s);
            }
            for (r, v) in func.variables.iter() {
                var_names.insert(r, v.name.clone());
            }
        }

        // units: the main fn's label when it is a plain identifier and unique
        let candidates: Vec<Option<String>> = p
            .compute_units
            .values()
            .map(|u| {
                let ComputeUnit::DensityFunction { function, .. } = u;
                let s = l.functions.get(function)?;
                (plain(s) && !is_numbered(s, 'u')).then(|| s.clone())
            })
            .collect();
        let cand_count = count(candidates.iter().flatten().map(|s| s.as_str()));
        for (i, (r, _)) in p.compute_units.iter().enumerate() {
            let s = match &candidates[i] {
                Some(c) if cand_count[c.as_str()] == 1 => c.clone(),
                _ => format!("u{i}"),
            };
            l.units.insert(r, s);
        }

        // inputs: per unit
        for (_, u) in p.compute_units.iter() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = u;
            let bases: Vec<Option<String>> = density_inputs
                .values()
                .map(|d| {
                    let name = var_names.get(&d.var).unwrap_or_else(|| {
                        panic!("DensityInput.var refers to a variable that is in no function")
                    });
                    match name {
                        Name::Named(s) | Name::Prefixed(s) => {
                            (plain(s) && !is_numbered(s, 'd')).then(|| s.clone())
                        }
                        Name::Anonymous => None,
                    }
                })
                .collect();
            let bc = count(bases.iter().flatten().map(|s| s.as_str()));
            for (i, (r, _)) in density_inputs.iter().enumerate() {
                let s = match &bases[i] {
                    Some(b) if bc[b.as_str()] == 1 => b.clone(),
                    _ => format!("d{i}"),
                };
                l.inputs.insert(r, s);
            }
        }
        for (u, unit) in p.compute_units.iter() {
            let ComputeUnit::DensityFunction { density_inputs, .. } = unit;
            for (d, _) in density_inputs.iter() {
                l.input_units.insert(d, u);
            }
        }

        // entries
        let mut per_unit: HashMap<Ref<ComputeUnit>, usize> = HashMap::new();
        for e in &p.entry_points {
            *per_unit.entry(e.density_function).or_insert(0) += 1;
        }
        for (i, e) in p.entry_points.iter().enumerate() {
            let unit = l.units.get(&e.density_function).unwrap_or_else(|| {
                panic!("entry point {i} refers to a unit that is not in the program")
            });
            let s = if per_unit[&e.density_function] == 1 && !is_numbered(unit, 'e') {
                unit.clone()
            } else {
                format!("e{i}")
            };
            l.entries.push(s);
        }
        l
    }

    accessors! {
        function(functions: Function) "a function that is not in the program";
        unit(units: ComputeUnit) "a compute unit that is not in the program";
        constant(constants: Constant) "a constant that is not in the program";
        host(hosts: HostInput) "a host input that is not in the program";
        table(tables: PermutationTableInput) "a permutation table that is not in the program";
        input(inputs: DensityInput) "a density input that is in no unit";
    }

    pub fn var(&self, f: Ref<Function>, v: Ref<Var>) -> &str {
        self.vars.get(&(f, v)).unwrap_or_else(|| {
            panic!(
                "dangling Ref: a variable that is not in function {}",
                self.function(f)
            )
        })
    }
    /// The unit that owns the input.
    pub fn input_unit(&self, d: Ref<DensityInput>) -> Ref<ComputeUnit> {
        *self
            .input_units
            .get(&d)
            .unwrap_or_else(|| panic!("dangling Ref: a density input that is in no unit"))
    }
    pub fn entry(&self, i: usize) -> &str {
        &self.entries[i]
    }
}
