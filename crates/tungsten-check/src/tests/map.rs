//! For every id, the forward then reverse lookup returns the same id.

use tungsten_core::{ExternId, FnId, HostId, RecordId, TableId};

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
        assert_eq!(m.auto_consts.len(), pr.tables.len());
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
    assert_eq!(m.auto_consts[0].initializer, "make_perm");
    assert_eq!(m.auto_consts[0].name, "n");
    assert_eq!(m.auto_params.name(crate::AutoParamId(0)), Some("seed"));
    assert_eq!(m.tables.name(TableId(0)), Some("n"));
    assert_eq!(m.extern_name(ExternId(0)), Some("sin"));
    assert_eq!(m.record_name(RecordId(0)), Some("Pair"));
    assert_eq!(m.field_name(RecordId(0), 1), Some("b"));
    assert_eq!(m.inputs.name(tungsten_core::InputId(0)), Some("d"));
    let main = fn_id(&l, "top");
    let pr = m.functions.get(main).unwrap();
    let param = p.functions[pr].parameters[0];
    assert_eq!(m.local(main, param), Some(tungsten_core::LocalId(0)));
}

#[test]
fn resolved_auto_consts_bind_params_by_name() {
    use crate::{AutoArgValue as V, AutoCoreId, ParamBindError as E};
    let l = ok(&load(&snippet("", "return 1.0;")), &registry());
    let m = &l.map;
    let seed = |v: V| vec![("seed".to_string(), v)];
    let r = m.resolved_auto_consts(&seed(V::I64(-7))).unwrap();
    // Every autoconst, read or not, in arena order.
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].name, r[0].initializer), ("n", "make_perm"));
    assert_eq!(r[0].core, AutoCoreId::Table(TableId(0)));
    assert_eq!(r[0].args, vec![V::I64(-7), V::I64(1)]);
    assert_eq!(r[1].args, vec![V::I64(-7)]);
    assert_eq!(r[1].core, AutoCoreId::Table(TableId(1)));

    assert_eq!(m.resolved_auto_consts(&[]).unwrap_err(), E::Missing("seed".into()));
    assert_eq!(
        m.resolved_auto_consts(&[("x".into(), V::I64(1))]).unwrap_err(),
        E::Unknown("x".into())
    );
    let twice = vec![("seed".to_string(), V::I64(1)), ("seed".to_string(), V::I64(2))];
    assert_eq!(m.resolved_auto_consts(&twice).unwrap_err(), E::Duplicate("seed".into()));
    assert!(matches!(
        m.resolved_auto_consts(&seed(V::F64(1.0))).unwrap_err(),
        E::WrongType { .. }
    ));
}

#[test]
fn handle_types_are_nominal() {
    // `sample` takes a PerlinNoiseSampler; b3 is an InterpolatedNoiseSampler.
    let src = snippet("", "return ext sample(auto(b3), p);");
    let p = load(&src);
    let l = crate::lower(&p, &registry()).unwrap();
    let e = crate::check_lowered(&l).unwrap_err();
    assert!(e.to_string().contains("handle InterpolatedNoiseSampler"), "{e}");
}
