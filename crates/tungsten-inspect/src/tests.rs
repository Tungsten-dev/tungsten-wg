//! Smoke test: a small program, as `.tw` text, covering sharing, resampling, indexed reads, loops,
//! constants, host inputs and tables. Set `TUNGSTEN_INSPECT_OUT=<dir>` to write the tree there.

use tungsten_interface::{Backend, Compiler, Interface};
use tungsten_ir::model::OptimizedProgram;
use tungsten_optimizer::{explain_inline, perform_inline};
use tungsten_parse::{FileId, NoEnv, load};

use crate::backend::{InspectBackend, InspectCompile, InspectConfig, Views, inspect};
use crate::diff::diff;
use crate::model::{Inspection, Verdict};

fn program(src: &str) -> OptimizedProgram {
    load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}")).program
}

const SMALL: &str = r#"
program {
    host seed: i64;
    table temperature = perlin("t:temperature", "first", 0);
    const base = 0.25;
    const scale = (const(base) * 2.0);

    // Helper: a repeat loop with a break; `acc` is reassigned in the loop (loop-carried).
    fn `t:accumulate`(t: f64) -> f64 {
        vars { acc: f64 }
        acc = 0.0;
        repeat 3 {
            acc = (acc + t);
            if (acc > 10.0) {
                break;
            }
        }
        return acc;
    }

    // A 1x1x1 unit reading a host input.
    fn `t:seed`() -> f64 @hosts(seed) { return (host(seed) as f64); }

    // Noise: a constant depending on a constant, and a permutation table. Used at two grids.
    fn `t:noise`() -> f64 {
        return (ext sample::<(perm_table, f64) -> f64>(table(temperature), 1.0) * const(scale));
    }

    // Shared by both entries; reads the seed at 1x1x1 (resampled).
    fn `t:shared`() -> f64 { return (density(seed_in) + 1.0); }

    // Read once, by `t:a`, with an identity mapping: inlinable.
    fn `t:leaf`() -> f64 { return 2.0; }
    // Read once, by `t:a`, with an index: not inlinable.
    fn `t:table`() -> f64 { return 3.0; }

    fn `t:a`() -> f64 {
        vars { x: f64 }
        x = (density(shared_in) + density(noise_a));
        if (x > 0.0) {
            x = `t:accumulate`(x);
        } else {
            x = (density(leaf_in) + density(table_in, 0));
        }
        // `x` here is a phi of both branches.
        return (x * (density(shared_in) + density(noise_a)));
    }

    fn `t:b`() -> f64 { return (density(shared_in) - density(noise_b, 0)); }

    // Read by nothing: unreachable.
    fn `t:orphan`() -> f64 { return 4.0; }

    unit seed_u { main `t:seed`; hosts [seed]; }
    unit noise_u { main `t:noise`; consts [base, scale]; tables [temperature]; }
    unit shared_u { main `t:shared`; inputs [seed_in]; }
    unit leaf_u { main `t:leaf`; }
    unit table_u { main `t:table`; }
    unit orphan_u { main `t:orphan`; }
    input seed_in = seed_u @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    input noise_a = noise_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    input noise_b = noise_u @dims(2, 2, 2) @origin_scale(1, 1, 1) @position_scale(0.5, 0.5, 0.5);
    input shared_in = shared_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    input leaf_in = leaf_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    input table_in = table_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    unit a_u {
        main `t:a`;
        helpers [`t:accumulate`];
        inputs [shared_in, noise_a, leaf_in, table_in];
    }
    unit b_u { main `t:b`; inputs [shared_in, noise_b]; }
    entry a_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry b_u @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

fn unit_info<'a>(i: &'a Inspection, display: &str) -> &'a crate::model::UnitInfo {
    i.units
        .iter()
        .find(|u| u.display == display)
        .unwrap_or_else(|| panic!("no unit {display}"))
}

#[test]
fn inspects_small_program() {
    let mut program = program(SMALL);
    let config = InspectConfig::default();

    let out = inspect(&program, &config);
    let backend = InspectBackend::new(config.clone());
    let interface = Interface::build(&program, &backend.naming_rules());
    let generated = backend.generate(&program, &interface).unwrap();
    assert_eq!(
        generated.files,
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
const BYTES: &str = r#"
program {
    const `table` = [1b, -2b, 127b];
    fn `minecraft:bytes`() -> i8 {
        vars { a: i8, wide: i32, d: f64 }
        // 127 + 127 wraps
        a = (const(`table`)[2] + 127b);
        a = (-a & (a << 1));
        wide = ((a as i32) & 255);
        d = 300.5;
        a = (d as i8);
        return (a + (wide as i8));
    }
    unit bytes { main `minecraft:bytes`; consts [`table`]; }
    entry bytes @dims(4, 4, 4) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

#[test]
fn byte_buffers_and_literals() {
    let program = program(BYTES);
    let i = inspect(&program, &InspectConfig::default()).inspection;
    let entry = &i.entries[0];
    for axis in [&entry.buffers.cpu, &entry.buffers.gpu] {
        let out = axis
            .buffers
            .iter()
            .find(|b| b.pinned)
            .expect("entry output is pinned");
        assert_eq!(out.bytes, 64, "4x4x4 bytes");
    }
    let ir = &i
        .functions
        .iter()
        .find(|f| f.ir.contains("table"))
        .expect("unit function")
        .ir;
    assert!(ir.contains("127b"), "missing byte literal in:\n{ir}");
}

#[test]
fn compiler_matches_inspect_and_views_setter_works() {
    let registry = tungsten_check::RegistryBuilder::new().build().unwrap();
    let src = "program { fn `minecraft:one`() -> f64 { return 1.0; } unit one { main `minecraft:one`; } entry one @dims(1, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1); }";
    let p = program(src);
    let mut views = Views::default();
    views.ir_text = false;
    let c = Compiler::new(p.clone(), registry, InspectBackend::new(InspectConfig::default()))
        .views(views.clone());
    assert_eq!(c.backend().config().views, views);
    let out = c.compile().unwrap();
    let expected = inspect(&p, &InspectConfig { views, ..InspectConfig::default() });
    assert_eq!(out.files, expected.files);
}
