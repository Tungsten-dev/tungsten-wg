//! `RegistryBuilder`.

use tungsten_core::{ExternDecl, ExternId, RecordDecl, RecordId, Type};
use tungsten_ir::VariableType as T;

use crate::{RegistryBuilder, RegistryError};

#[test]
fn declaration_order_ids() {
    let r = RegistryBuilder::new()
        .extern_fn("b", &[T::F64], T::F64)
        .extern_fn_with_domain_error("a", &[T::F64], T::F64)
        // Forward reference to a record declared later.
        .record("Z", &[("inner", T::Extern("Y")), ("n", T::I32)])
        .record("Y", &[("v", T::Array(Box::new(T::F64), 2))])
        .build()
        .unwrap();
    assert_eq!(r.extern_id("b"), Some(ExternId(0)));
    assert_eq!(r.extern_id("a"), Some(ExternId(1)));
    assert_eq!(r.extern_id("c"), None);
    assert_eq!(r.extern_name(ExternId(1)), "a");
    assert_eq!(r.record_id("Z"), Some(RecordId(0)));
    assert_eq!(r.record_id("Y"), Some(RecordId(1)));
    assert_eq!(r.record_name(RecordId(1)), "Y");
    assert_eq!(r.field_index(RecordId(0), "n"), Some(1));
    assert_eq!(r.field_index(RecordId(0), "v"), None);
    assert_eq!(r.field_name(RecordId(0), 0), "inner");
    let (externs, records) = r.decls();
    assert_eq!(
        externs,
        vec![
            ExternDecl {
                params: vec![Type::F64],
                ret: Type::F64,
                domain_error: false
            },
            ExternDecl {
                params: vec![Type::F64],
                ret: Type::F64,
                domain_error: true
            },
        ]
    );
    assert_eq!(
        records,
        vec![
            RecordDecl {
                fields: vec![Type::Record(RecordId(1)), Type::I32]
            },
            RecordDecl {
                fields: vec![Type::Array(Box::new(Type::F64), 2)]
            },
        ]
    );
}

#[test]
fn duplicates() {
    assert_eq!(
        RegistryBuilder::new()
            .extern_fn("f", &[], T::F64)
            .extern_fn("f", &[T::I32], T::F64)
            .build(),
        Err(RegistryError::DuplicateExtern("f".into()))
    );
    assert_eq!(
        RegistryBuilder::new()
            .record("R", &[])
            .record("R", &[])
            .build(),
        Err(RegistryError::DuplicateRecord("R".into()))
    );
    assert_eq!(
        RegistryBuilder::new()
            .record("R", &[("a", T::F64), ("a", T::I32)])
            .build(),
        Err(RegistryError::DuplicateField {
            record: "R".into(),
            field: "a".into()
        })
    );
}

#[test]
fn unknown_record_and_density() {
    assert_eq!(
        RegistryBuilder::new()
            .record("R", &[("a", T::Array(Box::new(T::Extern("Nope")), 1))])
            .build(),
        Err(RegistryError::UnknownRecord {
            context: "R.a".into(),
            name: "Nope".into()
        })
    );
    assert_eq!(
        RegistryBuilder::new()
            .extern_fn("f", &[T::Extern("Nope")], T::F64)
            .build(),
        Err(RegistryError::UnknownRecord {
            context: "f".into(),
            name: "Nope".into()
        })
    );
    assert_eq!(
        RegistryBuilder::new()
            .extern_fn("f", &[], T::DensityInput)
            .build(),
        Err(RegistryError::DensityInputType {
            context: "f".into()
        })
    );
}

#[test]
fn record_cycles() {
    assert_eq!(
        RegistryBuilder::new()
            .record("R", &[("me", T::Extern("R"))])
            .build(),
        Err(RegistryError::RecordCycle(vec!["R".into(), "R".into()]))
    );
    assert_eq!(
        RegistryBuilder::new()
            .record("Ok", &[("v", T::F64)])
            .record("A", &[("b", T::Array(Box::new(T::Extern("B")), 3))])
            .record("B", &[("ok", T::Extern("Ok")), ("a", T::Extern("A"))])
            .build(),
        Err(RegistryError::RecordCycle(vec![
            "A".into(),
            "B".into(),
            "A".into()
        ]))
    );
    // A diamond is not a cycle.
    assert!(
        RegistryBuilder::new()
            .record("L", &[("v", T::F64)])
            .record("M", &[("l", T::Extern("L"))])
            .record("N", &[("l", T::Extern("L")), ("m", T::Extern("M"))])
            .build()
            .is_ok()
    );
}
