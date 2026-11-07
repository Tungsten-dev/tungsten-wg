//! Determinism (spec §10) and duplicate names (spec §6).

mod common;

use std::collections::HashSet;

use common::*;
use tungsten_builder::prelude::*;
use tungsten_interface::naming::{NamingRules, entry_method_names};
use tungsten_interface::{Backend, Interface};
use tungsten_ir::{ComputeUnit, isomorphic};
use tungsten_rust::backend::{RustBackend, RustConfig};

fn rust(p: &OptimizedProgram) -> String {
    let mut b = RustBackend::new(RustConfig::default());
    let i = Interface::build(p, b.naming_rules());
    b.generate(p, &i).unwrap();
    b.output().unwrap().full_source.clone()
}

#[test]
fn building_twice_gives_the_same_program_and_code() {
    let a = checked(summary_example());
    let b = checked(summary_example());
    assert!(isomorphic(&a, &b));
    assert_eq!(rust(&a), rust(&b));
    assert_eq!(tungsten_parse::print(&a), tungsten_parse::print(&b));
}

/// Two units, helpers and constants with one name each get distinct generated names.
#[test]
fn duplicate_names_are_made_unique() {
    let mut pb = ProgramBuilder::new();
    let mut units = vec![];
    for k in [1.0, 2.0] {
        let c = pb.constant("c", k);
        let mut h = FnBuilder::new("h", F64);
        h.ret(c);
        let h = pb.finish_helper(h);
        let mut f = FnBuilder::new("u", F64);
        f.ret(h.call([]));
        let u = pb.finish_unit(f);
        pb.entry(u, Grid::new((1, 1, 1)));
        units.push(u);
    }
    let p = checked(pb);
    for rules in [NamingRules::rust(), NamingRules::java(), NamingRules::inspect()] {
        let i = Interface::build(&p, &rules);
        let fns: HashSet<_> = p
            .compute_units
            .iter()
            .map(|(r, _)| i.density_fn(r, (1, 1, 1)).to_string())
            .collect();
        assert_eq!(fns.len(), 2, "{fns:?}");
        let helpers: HashSet<_> = p
            .functions
            .iter()
            .filter(|(r, _)| {
                !p.compute_units.values().any(|u| {
                    let ComputeUnit::DensityFunction { function, .. } = u;
                    function == r
                })
            })
            .map(|(r, _)| i.helper(r).to_string())
            .collect();
        assert_eq!(helpers.len(), 2, "{helpers:?}");
        let consts: HashSet<_> = p.constants.iter().map(|(r, _)| i.constant(r).to_string()).collect();
        assert_eq!(consts.len(), 2, "{consts:?}");
        let entries: HashSet<_> = entry_method_names(&p, &rules, HashSet::new()).into_iter().collect();
        assert_eq!(entries.len(), 2, "{entries:?}");
    }
}
