//! Smoke test: a small hand-built program covering sharing, resampling, indexed reads, loops,
//! constants, host inputs and tables. Set `TUNGSTEN_INSPECT_OUT=<dir>` to write the tree there.

use bumpalo::Bump;

use tungsten_interface::{Backend, Interface};
use tungsten_legacy::builder::{Builder, SPMTBuilder};
use tungsten_legacy::conversion::convert;
use tungsten_legacy::model::*;
use tungsten_optimizer::{explain_inline, perform_inline};

use crate::backend::{InspectBackend, InspectConfig, inspect};
use crate::diff::diff;
use crate::model::{FunctionKind, Inspection, NodeKind, Verdict};

fn var<'m>(v: Var<'m>) -> Expression<'m> {
    Expression::Variable(v)
}

fn unit<'m>(
    arena: &'m Bump,
    name: &str,
    body: impl FnOnce(&mut Builder<'m>) -> Vec<Statement<'m>>,
) -> DensityFunctionRef<'m> {
    let mut b = Builder::new(arena).named(name).returns(VariableType::F64);
    let stmts = body(&mut b);
    b.set_body(stmts);
    b.finish_compute_unit()
}

fn build_program<'m>(arena: &'m Bump) -> SPMT<'m> {
    // Helper: a repeat loop with a break; `acc` is reassigned in the loop (loop-carried).
    let mut h = Builder::new(arena)
        .named("t:accumulate")
        .returns(VariableType::F64);
    let t = h.param("t", VariableType::F64);
    let acc = h.local("acc", VariableType::F64);
    h.set_body(vec![
        Statement::Assign {
            target: acc,
            value: 0.0f64.into(),
        },
        Statement::Repeat {
            count: 3,
            body: vec![
                Statement::Assign {
                    target: acc,
                    value: acc + t,
                },
                Statement::If {
                    condition: acc.gt(10.0f64),
                    then_branch: vec![Statement::Break],
                    else_branch: vec![],
                },
            ],
        },
        Statement::Return(var(acc)),
    ]);
    let helper = h.finish_function();

    // A 1x1x1 unit reading a host input.
    let mut s = Builder::new(arena)
        .named("t:seed")
        .returns(VariableType::F64);
    let seed = s.alloc_var(Name::Named("seed".into()), VariableType::I64);
    s.set_body(vec![Statement::Return(Expression::ExplicitCast {
        to: VariableType::F64,
        expr: Box::new(var(seed)),
    })]);
    let mut seed_unit = s.build_compute_unit();
    seed_unit.host_inputs = vec![HostInput::Static(seed)];
    let seed_unit: DensityFunctionRef = Interned::new(arena.alloc(seed_unit));

    // Noise: a constant depending on a constant, and a permutation table. Used at two grids.
    let noise = unit(arena, "t:noise", |n| {
        let base = n.constant("base", VariableType::F64, 0.25f64);
        let scale = n.constant(
            "scale",
            VariableType::F64,
            Expression::Variable(base) * 2.0f64,
        );
        let perm = Expression::PermutationTable(PermutationTableInput::PerlinNoise {
            ident: "t:temperature".into(),
            subident: Some("first".into()),
            subident_index: 0,
        });
        vec![Statement::Return(
            Expression::ExternCall {
                function_name: "sample".into(),
                parameters: vec![perm, 1.0f64.into()],
                parameter_types: vec![VariableType::PermutationTable, VariableType::F64],
                return_type: VariableType::F64,
            } * var(scale),
        )]
    });

    // Shared by both entries; reads the seed at 1x1x1 (resampled).
    let shared = unit(arena, "t:shared", |b| {
        let seed_in = b.density(seed_unit, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (1, 1, 1));
        vec![Statement::Return(seed_in.get() + 1.0f64)]
    });

    // Read once, by `t:a`, with an identity mapping: inlinable.
    let leaf = unit(arena, "t:leaf", |_| vec![Statement::Return(2.0f64.into())]);
    // Read once, by `t:a`, with an index: not inlinable.
    let table = unit(arena, "t:table", |_| vec![Statement::Return(3.0f64.into())]);

    let a = unit(arena, "t:a", |b| {
        let s = b.density(shared, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let n = b.density(noise, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let l = b.density(leaf, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let t = b.density(table, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let x = b.local("x", VariableType::F64);
        vec![
            Statement::Assign {
                target: x,
                value: s.get() + n.get(),
            },
            Statement::If {
                condition: var(x).gt(0.0f64),
                then_branch: vec![Statement::Assign {
                    target: x,
                    value: helper.call([var(x)]),
                }],
                else_branch: vec![Statement::Assign {
                    target: x,
                    value: l.get() + t.at(0i32),
                }],
            },
            // `x` here is a φ of both branches.
            Statement::Return(var(x) * (s.get() + n.get())),
        ]
    });

    let b_entry = unit(arena, "t:b", |b| {
        let s = b.density(shared, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), (4, 4, 4));
        let n = b.density(noise, (1.0, 1.0, 1.0), (0.5, 0.5, 0.5), (2, 2, 2));
        vec![Statement::Return(s.get() - n.at(0i32))]
    });

    // Read by nothing: unreachable.
    let orphan = unit(arena, "t:orphan", |_| {
        vec![Statement::Return(4.0f64.into())]
    });

    SPMT {
        density_functions: vec![seed_unit, noise, shared, leaf, table, a, b_entry, orphan],
        functions: vec![helper],
        main_density_functions: [a, b_entry]
            .into_iter()
            .map(|density_function| MainDensityFunction {
                density_function,
                dimensions: (4, 4, 4),
                scaled_origin: (1.0, 1.0, 1.0),
                scaled_position: (1.0, 1.0, 1.0),
            })
            .collect(),
    }
}

fn unit_info<'a>(i: &'a Inspection, display: &str) -> &'a crate::model::UnitInfo {
    i.units
        .iter()
        .find(|u| u.display == display)
        .unwrap_or_else(|| panic!("no unit {display}"))
}

#[test]
fn inspects_small_program() {
    let arena = Bump::new();
    let mut program = convert(&build_program(&arena)).expect("conversion");
    let config = InspectConfig::default();

    let out = inspect(&program, &config);
    let mut backend = InspectBackend::new(config.clone());
    let interface = Interface::build(&program, backend.naming_rules());
    backend.generate(&program, &interface).unwrap();
    assert_eq!(
        backend.output().unwrap().files,
        out.files,
        "backend and inspect() differ"
    );
    assert_eq!(
        inspect(&program, &config).files,
        out.files,
        "inspecting twice differs"
    );

    if let Ok(dir) = std::env::var("TUNGSTEN_INSPECT_OUT") {
        out.write_to(dir.as_ref()).unwrap();
    }

    for path in [
        "index.md",
        "inspection.json",
        "ir/program.txt",
        "program/call_graph.dot",
        "program/resources.dot",
        "entries/t/a@4x4x4/dependencies.dot",
        "entries/t/a@4x4x4/waves.dot",
        "entries/t/a@4x4x4/buffers.txt",
        "entries/t/a@4x4x4/buffers.cpu.svg",
        "entries/t/b@4x4x4/buffers.gpu.svg",
    ] {
        assert!(
            out.files.contains_key(std::path::Path::new(path)),
            "missing {path}; have {:?}",
            out.files.keys().collect::<Vec<_>>()
        );
    }
    let ir = &out.files[std::path::Path::new("ir/program.txt")];
    assert!(ir.contains("unit t:a -> f64 {"));
    assert!(ir.contains("fn t:accumulate(t: f64) -> f64 {"));
    assert!(ir.contains("const scale = (base * 2.0)"));

    let json = &out.files[std::path::Path::new("inspection.json")];
    let parsed: serde_json::Value = serde_json::from_str(json).expect("inspection.json parses");
    assert_eq!(parsed["version"], 1);

    let i = &out.inspection;
    assert_eq!(i.summary.entries, 2);
    assert_eq!(i.summary.units, 8);
    assert_eq!(i.summary.reachable_units, 7);
    assert_eq!(i.summary.unreachable.units, vec!["cu:t:orphan".to_string()]);
    assert_eq!(
        i.summary.unreachable.functions,
        vec!["fn:t:orphan".to_string()]
    );
    assert!(matches!(
        unit_info(i, "t:orphan").inline_verdict.verdict,
        Verdict::Unused
    ));

    // Raw canonical names, unsanitized.
    assert!(i.units.iter().any(|u| u.id == "cu:t:shared"));
    assert!(
        i.instances
            .iter()
            .any(|x| x.id == "inst:t:shared_d4x4x4_output")
    );

    // Noise at two grids; the seed is read on a different grid than its consumer's.
    assert_eq!(unit_info(i, "t:noise").instances.len(), 2);
    assert!(
        i.instances
            .iter()
            .flat_map(|x| &x.inputs)
            .any(|e| e.resampled)
    );
    assert!(
        i.instances
            .iter()
            .flat_map(|x| &x.inputs)
            .any(|e| e.indexed)
    );

    // Both entries need `t:shared` (and the seed).
    for e in &i.entries {
        assert!(
            e.shared
                .iter()
                .any(|s| s.instance == "inst:t:shared_d4x4x4_output")
        );
        assert_eq!(e.schedule.steps.len(), e.instances.len());
        assert_eq!(e.wave_count, e.schedule.waves.len());
        for axis in [&e.buffers.cpu, &e.buffers.gpu] {
            let out = axis
                .buffers
                .iter()
                .find(|b| b.pinned)
                .expect("entry output is pinned");
            assert_eq!(out.instance, e.instance);
            assert_eq!(out.end, None);
            assert!(axis.peak_live_bytes <= axis.total_bytes);
            assert!(axis.slot_bytes <= axis.total_bytes);
            assert!(axis.slot_count <= axis.buffers.len());
        }
    }
    // `t:a` runs 6 instances; with reuse, it needs less than one buffer each.
    let a = i
        .entries
        .iter()
        .find(|e| e.display == "t:a @ 4x4x4")
        .unwrap();
    assert!(a.buffers.cpu.slot_bytes < a.buffers.cpu.total_bytes);

    // Inline verdicts.
    assert!(matches!(
        unit_info(i, "t:a").inline_verdict.verdict,
        Verdict::EntryPoint
    ));
    assert!(matches!(
        unit_info(i, "t:shared").inline_verdict.verdict,
        Verdict::MultipleUses { uses: 2 }
    ));
    assert!(matches!(
        unit_info(i, "t:table").inline_verdict.verdict,
        Verdict::IndexedRead { .. }
    ));
    assert!(matches!(
        unit_info(i, "t:seed").inline_verdict.verdict,
        Verdict::GridMismatch { .. }
    ));
    let Verdict::Inlinable { into, .. } = &unit_info(i, "t:leaf").inline_verdict.verdict else {
        panic!("t:leaf should be inlinable");
    };
    assert_eq!(into, "cu:t:a");

    // Resources: the constant chain, the table, the host input.
    assert!(
        i.resources
            .constant_dependencies
            .iter()
            .any(|d| d.constant == "const:scale" && d.depends_on == "const:base")
    );
    assert_eq!(i.tables.len(), 1);
    assert_eq!(i.tables[0].display, "perlin t:temperature[0] first");
    assert_eq!(i.host_inputs[0].users, vec!["cu:t:seed".to_string()]);
    assert!(i.call_graph.edges.iter().any(|e| e.to == "fn:t:accumulate"));

    // Snapshot + inline: provenance names the absorbed unit, and the snapshot is independent.
    let before = program.clone();
    let stats = perform_inline(&mut program);
    assert_eq!(stats.inlined, stats.records.len());
    let record = stats
        .records
        .iter()
        .find(|r| r.callee_name.as_deref() == Some("t:leaf"))
        .expect("t:leaf absorbed");
    assert!(program.compute_units.contains(record.caller));
    assert!(!program.compute_units.contains(record.callee));
    let callee_before = before
        .compute_units
        .translate(record.callee, &program.compute_units);
    assert!(before.compute_units.contains(callee_before));
    assert_eq!(explain_inline(&before).len(), 8);
    let after = inspect(&program, &config).inspection;
    assert_eq!(after.summary.units, 8 - stats.inlined);

    let summary = diff(&before, &program, &stats);
    if let Ok(dir) = std::env::var("TUNGSTEN_INSPECT_OUT") {
        std::fs::write(std::path::Path::new(&dir).join("diff.txt"), &summary).unwrap();
    }
    assert!(summary.contains("t:leaf → t:a"), "{summary}");
    assert!(
        summary.contains("removed without provenance: (none)"),
        "{summary}"
    );
    assert!(summary.contains("added: (none)"), "{summary}");
}

/// A 4x4x4 unit returning `I8`: byte constants, wrapping arithmetic, bit operations and casts.
fn build_byte_program<'m>(arena: &'m Bump) -> SPMT<'m> {
    let b = Builder::new(arena).named("minecraft:bytes").returns(VariableType::I8);
    let table = b.constant(
        "table",
        VariableType::Array(Box::new(VariableType::I8), 3),
        Expression::ArrayLiteral(vec![1i8.into(), (-2i8).into(), 127i8.into()]),
    );
    let a = b.local("a", VariableType::I8);
    let wide = b.local("wide", VariableType::I32);
    let d = b.local("d", VariableType::F64);
    let mut b = b;
    b.set_body(vec![
        // 127 + 127 wraps
        Statement::Assign { target: a, value: var(table).index(2i32) + 127i8 },
        Statement::Assign { target: a, value: (-var(a)).bitand(var(a).shl(1i32)) },
        Statement::Assign { target: wide, value: var(a).cast(VariableType::I32).bitand(255i32) },
        Statement::Assign { target: d, value: 300.5f64.into() },
        Statement::Assign { target: a, value: var(d).cast(VariableType::I8) },
        Statement::Return(var(a) + var(wide).cast(VariableType::I8)),
    ]);
    let unit = b.finish_compute_unit();
    SPMT {
        density_functions: vec![unit],
        functions: vec![],
        main_density_functions: vec![MainDensityFunction {
            density_function: unit,
            dimensions: (4, 4, 4),
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
        }],
    }
}

#[test]
fn byte_buffers_and_literals() {
    let arena = Bump::new();
    let program = convert(&build_byte_program(&arena)).expect("conversion");
    let i = inspect(&program, &InspectConfig::default()).inspection;
    let entry = &i.entries[0];
    for axis in [&entry.buffers.cpu, &entry.buffers.gpu] {
        let out = axis.buffers.iter().find(|b| b.pinned).expect("entry output is pinned");
        assert_eq!(out.bytes, 64, "4x4x4 bytes");
    }
    let ir = &i.functions.iter().find(|f| f.ir.contains("table")).expect("unit function").ir;
    assert!(ir.contains("127b"), "missing byte literal in:\n{ir}");
}
