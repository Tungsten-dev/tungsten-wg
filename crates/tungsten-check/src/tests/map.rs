//! For every id, the forward then reverse lookup returns the same id.

use tungsten_core::{ExternId, FnId, HostId, RecordId, TableId};
use tungsten_ir::PermutationTableInput;

use super::determinism::corpus;
use super::*;
use crate::{CoreId, IdMap};

fn round_trip<T, I: CoreId + PartialEq + std::fmt::Debug>(m: &IdMap<T, I>, n: usize) {
    assert_eq!(m.len(), n);
    for i in 0..n {
        let id = I::from_index(i);
        let r = m.get(id).unwrap();
        assert_eq!(m.id(r), Some(id));
        assert!(m.name(id).is_some());
    }
}

#[test]
fn forward_then_reverse() {
    for p in corpus() {
        let l = ok(&p, &registry());
        let (pr, m) = (&l.program, &l.map);
        round_trip(&m.units, pr.units.len());
        round_trip(&m.functions, pr.functions.len());
        round_trip(&m.consts, pr.consts.len());
        round_trip(&m.hosts, pr.hosts.len());
        round_trip(&m.tables, pr.tables.len());
        round_trip(&m.inputs, pr.inputs.len());
        assert_eq!(m.locals.len(), pr.functions.len());
        for (f, locals) in pr.functions.iter().zip(&m.locals) {
            round_trip(locals, f.locals.len());
        }
        assert_eq!(m.host_names.len(), pr.hosts.len());
        assert_eq!(m.table_idents.len(), pr.tables.len());
        assert_eq!(m.input_vars.len(), pr.inputs.len());
        assert_eq!(m.externs.len(), pr.externs.len());
        assert_eq!(m.records.len(), pr.records.len());
        for (r, d) in m.records.iter().zip(&pr.records) {
            assert_eq!(r.fields.len(), d.fields.len());
        }
        // Refs resolve in the program they came from.
        for (_, r, _) in m.functions.iter() {
            assert!(p.functions.contains(r));
        }
        let _ = FnId::from_index(0);
    }
}

#[test]
fn names_and_side_data() {
    let r = registry();
    let src = snippet(
        "",
        "return (ext sin(p) + new extern Pair(1.0, 2).a + density(d));",
    );
    let p = load(&src);
    let l = ok(&p, &r);
    let m = &l.map;
    assert_eq!(m.host_names, vec![Some("h".to_string())]);
    assert_eq!(m.hosts.name(HostId(0)), Some("h"));
    assert_eq!(
        m.table_idents[0],
        PermutationTableInput::PerlinNoise {
            ident: "n".into(),
            subident: None,
            subident_index: 0
        }
    );
    assert_eq!(m.tables.name(TableId(0)), Some("n"));
    assert_eq!(m.extern_name(ExternId(0)), Some("sin"));
    assert_eq!(m.record_name(RecordId(0)), Some("Pair"));
    assert_eq!(m.field_name(RecordId(0), 1), Some("b"));
    assert!(m.input_vars[0].is_some());
    let main = fn_id(&l, "top");
    let pr = m.functions.get(main).unwrap();
    let param = p.functions[pr].parameters[0];
    assert_eq!(m.local(main, param), Some(tungsten_core::LocalId(0)));
}
