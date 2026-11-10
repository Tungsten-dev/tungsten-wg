//! Derived lists (spec §5): transitive helpers, a shared helper with different callers,
//! constants reaching constants, and first-use order.

mod common;

use common::*;
use tungsten_builder::prelude::*;
use tungsten_ir::{ComputeUnit, OptimizedProgram, isomorphic};

fn program() -> ProgramBuilder {
    let mut pb = ProgramBuilder::new();
    let h1 = pb.host("h1", F64);
    let h2 = pb.host("h2", I32);
    let seed = pb.param("seed", I64);
    let perm = pb.auto_const("perm", Handle("PerlinNoiseSampler"), "make_perm", [seed.into(), 1i64.into()]);
    let sampler = pb.auto_const("sampler", Handle("InterpolatedNoiseSampler"), "make_sampler", [seed.into()]);
    let k = pb.constant("k", 1.0);
    let k2 = pb.constant("k2", k * 2.0);
    let k3 = pb.constant("k3", k2 + k);

    let mut f = FnBuilder::new("base", F64);
    f.ret(pos3().x().cast(F64));
    let base = pb.finish_unit(f);
    let d = pb.input_with(InputSpec::new(base, Grid::new((2, 1, 1))).named("d"));

    // leaf(x) reads h1, k3 and the sampler.
    let mut f = FnBuilder::new("leaf", F64);
    let x = f.param("x", F64);
    let s = f.local("s", Handle("InterpolatedNoiseSampler"));
    f.assign(s, sampler);
    f.ret(x + h1 + k3);
    let leaf = pb.finish_helper(f);

    // mid() reads d (an argument, so before leaf's items), then calls leaf, then reads h2 and
    // the permutation table.
    let mut f = FnBuilder::new("mid", F64);
    let t = f.local("t", Handle("PerlinNoiseSampler"));
    let r = f.local("r", F64);
    f.assign(r, leaf.call([d.read()]) + h2.cast(F64));
    f.assign(t, perm);
    f.ret(r);
    let mid = pb.finish_helper(f);

    // main1 reaches leaf twice (directly and through mid).
    let mut f = FnBuilder::new("main1", F64);
    f.ret(mid.call([]) + leaf.call([2.0.into()]));
    let main1 = pb.finish_unit(f);

    // main2 shares leaf, and reads k2 directly first.
    let mut f = FnBuilder::new("main2", F64);
    f.ret(k2 + leaf.call([k.into()]));
    let main2 = pb.finish_unit(f);

    pb.entry(main1, Grid::new((2, 1, 1)));
    pb.entry(main2, Grid::new((2, 1, 1)));
    pb
}

const TWIN: &str = r#"
program {
    host h1: f64;
    host h2: i32;
    param seed: i64;
    auto perm: handle PerlinNoiseSampler = make_perm(param(seed), 1L);
    auto sampler: handle InterpolatedNoiseSampler = make_sampler(param(seed));
    const k = 1.0;
    const k2 = (const(k) * 2.0);
    const k3 = (const(k2) + const(k));

    fn base() -> f64 {
        return (ambient(pos3).x as f64);
    }
    fn leaf(x: f64) -> f64 @hosts(h1) {
        vars { s: handle InterpolatedNoiseSampler }
        s = auto(sampler);
        return ((x + host(h1)) + const(k3));
    }
    fn mid() -> f64 @hosts(h2) {
        vars { t: handle PerlinNoiseSampler, r: f64 }
        r = (leaf(density(d)) + (host(h2) as f64));
        t = auto(perm);
        return r;
    }
    fn main1() -> f64 {
        return (mid() + leaf(2.0));
    }
    fn main2() -> f64 {
        return (const(k2) + leaf(const(k)));
    }

    input d = base @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);

    unit base { main base; }
    unit main1 {
        main main1;
        helpers [leaf, mid];
        autos [sampler, perm];
        hosts [h1, h2];
        consts [k, k2, k3];
        inputs [d];
    }
    unit main2 {
        main main2;
        helpers [leaf];
        autos [sampler];
        hosts [h1];
        consts [k, k2, k3];
    }
    entry main1 @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    entry main2 @dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#;

#[test]
fn lists_match_the_hand_written_twin() {
    let built = checked(program());
    let twin = parse(TWIN);
    assert!(
        isomorphic(&built, &twin),
        "built:\n{}\ntwin:\n{}",
        tungsten_parse::print(&built),
        tungsten_parse::print(&twin)
    );
}

/// Every way of removing one entry from one derived list.
fn removals(p: &OptimizedProgram) -> Vec<(String, OptimizedProgram)> {
    let mut out = Vec::new();
    for (u, cu) in p.compute_units.iter() {
        let ComputeUnit::DensityFunction {
            density_inputs,
            auto_consts,
            host_inputs,
            helper_functions,
            constants,
            ..
        } = cu;
        let lens = [
            density_inputs.len(),
            auto_consts.len(),
            host_inputs.len(),
            helper_functions.len(),
            constants.len(),
        ];
        for (list, n) in lens.into_iter().enumerate() {
            for i in 0..n {
                let mut q = p.clone();
                let u2 = q.compute_units.translate(u, &p.compute_units);
                let ComputeUnit::DensityFunction {
                    density_inputs,
                    auto_consts,
                    host_inputs,
                    helper_functions,
                    constants,
                    ..
                } = &mut q.compute_units[u2];
                match list {
                    0 => _ = density_inputs.remove(i),
                    1 => _ = auto_consts.remove(i),
                    2 => _ = host_inputs.remove(i),
                    3 => _ = helper_functions.remove(i),
                    _ => _ = constants.remove(i),
                }
                out.push((format!("unit {u:?} list {list} entry {i}"), q));
            }
        }
    }
    for (f, func) in p.functions.iter() {
        for i in 0..func.host_inputs.len() {
            let mut q = p.clone();
            let f2 = q.functions.translate(f, &p.functions);
            q.functions[f2].host_inputs.remove(i);
            out.push((format!("fn {f:?} host {i}"), q));
        }
    }
    out
}

#[test]
fn removing_any_list_entry_fails_the_checker() {
    let p = checked(program());
    let cases = removals(&p);
    // main1: 10 entries, main2: 6, and the hosts of leaf and mid.
    assert_eq!(cases.len(), 18);
    for (what, q) in cases {
        assert!(
            tungsten_check::prepare(&q, &empty_registry()).is_err(),
            "removing {what} still checks:\n{}",
            tungsten_parse::print(&q)
        );
    }
}

#[test]
fn summary_example_lists() {
    let built = checked(summary_example());
    let twin = parse(
        r#"
program {
    fn base() -> f64 { return ((ambient(pos3).x as f64) * 10.0); }
    fn shared() -> f64 { return density(%0); }
    fn `main`() -> f64 {
        vars { i: i32, total#1: f64 }
        i = 0;
        total#1 = shared();
        while (i < 3) {
            total#1 = (total#1 + density(%0, i));
            i = (i + 1);
        }
        return total#1;
    }
    input %0 = base @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
    unit base { main base; }
    unit m { main `main`; helpers [shared]; inputs [%0]; }
    entry m @dims(4, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1);
}
"#,
    );
    assert!(isomorphic(&built, &twin), "{}", tungsten_parse::print(&built));
}
