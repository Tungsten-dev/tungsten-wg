//! `eval` outside the corpus (`specs/unit-eval.md` §7, "Other checks"): `Interface` names and
//! scheduling, the inliner's use counting, and the Java size estimate.

use tungsten_interface::naming::NamingRules;
use tungsten_interface::{BufferBounds, Instance, Interface};
use tungsten_ir::model::{ComputeUnit, Program, Ref};
use tungsten_optimizer::{InlineRejection, InlineVerdict, explain_inline, perform_inline};
use tungsten_parse::{FileId, NoEnv, load};

const S: &str = "@origin_scale(1, 1, 1) @position_scale(1, 1, 1)";
const G: &str = "@dims(2, 1, 1) @origin_scale(1, 1, 1) @position_scale(1, 1, 1)";

fn parse(src: &str) -> Program {
    load(src, FileId(0), &NoEnv).unwrap_or_else(|d| panic!("{d}\n{src}")).program
}

fn unit(p: &Program, main: &str) -> Ref<ComputeUnit> {
    p.compute_units
        .iter()
        .find(|(_, u)| {
            let ComputeUnit::DensityFunction { function, .. } = u;
            p.functions[*function].canonical_name.as_deref() == Some(main)
        })
        .map(|(r, _)| r)
        .unwrap_or_else(|| panic!("no unit `{main}`"))
}

/// `probe` reads `b` by index and host `h` through `helper`; `top` evaluates it at two scales.
fn program() -> Program {
    parse(&format!(
        "program {{
            host h: f64;
            fn base_main() -> f64 {{ return 1.0; }}
            unit base {{ main base_main; }}
            input b = base {G};
            fn helper() -> f64 @hosts(h) {{ return host(h); }}
            fn probe_main() -> f64 {{ return (density(b, 0) + helper()); }}
            unit probe {{ main probe_main; helpers [helper]; inputs [b]; }}
            fn top_main() -> f64 {{
                return (eval(probe, ambient(pos3)) {S}
                    + eval(probe, ambient(pos3)) @origin_scale(0.5, 1, 1) @position_scale(1, 1, 1));
            }}
            unit top {{ main top_main; }}
            entry top {G};
        }}"
    ))
}

#[test]
fn eval_names_are_stable_and_deterministic() {
    let p = program();
    let (a, b) = (Interface::build(&p, &NamingRules::rust()), Interface::build(&p, &NamingRules::rust()));
    let names = |i: &Interface| i.eval_fns().iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b));
    assert_eq!(names(&a), ["eval_probe_main_s1x1x1_1x1x1", "eval_probe_main_sh3fe0000000000000x1x1_1x1x1"]);
    let probe = unit(&p, "probe_main");
    assert_eq!(a.eval_fn(probe, (1.0, 1.0, 1.0), (1.0, 1.0, 1.0)), "eval_probe_main_s1x1x1_1x1x1");
    // Helpers are cloned per key.
    let helper = p.functions.iter().find(|(_, f)| f.canonical_name.as_deref() == Some("helper")).unwrap().0;
    let keys: Vec<_> = a.eval_fns().iter().map(|e| e.key).collect();
    assert_eq!(a.eval_helper(&keys[0], helper), "eval_probe_main_s1x1x1_1x1x1_helper");
    assert_ne!(a.eval_helper(&keys[0], helper), a.eval_helper(&keys[1], helper));
}

#[test]
fn a_name_collision_falls_back_to_the_discovery_index() {
    let mut p = program();
    let taken = p.functions.iter().find(|(_, f)| f.canonical_name.as_deref() == Some("helper")).unwrap().0;
    p.functions[taken].canonical_name = Some("eval_probe_main_s1x1x1_1x1x1".into());
    let i = Interface::build(&p, &NamingRules::rust());
    assert_eq!(i.eval_fns()[0].name, "eval_probe_main_0");
}

#[test]
fn buffers_read_inside_evaluated_bodies_are_scheduled_first() {
    let p = program();
    let i = Interface::build(&p, &NamingRules::rust());
    let base = Instance::new(unit(&p, "base_main"), (2, 1, 1), (1.0, 1.0, 1.0), (1.0, 1.0, 1.0));
    let top = Instance::new(unit(&p, "top_main"), (2, 1, 1), (1.0, 1.0, 1.0), (1.0, 1.0, 1.0));
    assert_eq!(i.worker_schedule(0), [vec![base], vec![top]]);
    // `probe` has no instance, buffer or kernel: it's only evaluated.
    assert!(i.instances_of(unit(&p, "probe_main")).is_empty());
    // The host read inside the evaluated body joins the entry's hosts.
    assert_eq!(i.entry_hosts(0).len(), 1);
    // The reader passes the offset of `b` on to the eval function.
    let top_fn = p.functions.iter().find(|(_, f)| f.canonical_name.as_deref() == Some("top_main")).unwrap().0;
    assert_eq!(i.input_params(top_fn).len(), 1);
}

#[test]
fn an_eval_site_is_not_a_use() {
    let mut p = program();
    let verdicts = explain_inline(&p);
    let probe = unit(&p, "probe_main");
    let v = &verdicts.iter().find(|(u, _)| *u == probe).unwrap().1;
    assert_eq!(*v, InlineVerdict::Rejected(InlineRejection::Unused));
    // `base` is read by index, and `probe` is unused: nothing moves.
    assert_eq!(perform_inline(&mut p).inlined, 0);
    assert!(p.compute_units.contains(probe));
}

#[test]
fn evaluated_units_are_never_removed() {
    // `probe` is also read once as an input, so it would be inlined into `reader`, but `top`
    // evaluates it.
    let p = parse(&format!(
        "program {{
            fn probe_main() -> f64 {{ return 1.0; }}
            unit probe {{ main probe_main; }}
            input x = probe {G};
            fn reader_main() -> f64 {{ return (density(x) + eval(probe, ambient(pos3)) {S}); }}
            unit reader {{ main reader_main; inputs [x]; }}
            entry reader {G};
        }}"
    ));
    let probe = unit(&p, "probe_main");
    let v = explain_inline(&p).into_iter().find(|(u, _)| *u == probe).unwrap().1;
    assert_eq!(v, InlineVerdict::Rejected(InlineRejection::Evaluated));
}

#[test]
fn inlining_into_an_evaluated_body_must_not_add_unindexed_reads() {
    // `probe` is evaluated by `other`, and its only input's source `callee` reads `d` un-indexed:
    // inlining `callee` into `probe` would put that read in an evaluated body. (The program isn't
    // valid; the inliner doesn't validate.)
    let p = parse(&format!(
        "program {{
            fn leaf_main() -> f64 {{ return 2.0; }}
            unit leaf {{ main leaf_main; }}
            input d = leaf {G};
            fn callee_main() -> f64 {{ return density(d); }}
            unit callee {{ main callee_main; inputs [d]; }}
            input c = callee {G};
            fn probe_main() -> f64 {{ return density(c); }}
            unit probe {{ main probe_main; inputs [c]; }}
            fn other_main() -> f64 {{ return eval(probe, ambient(pos3)) {S}; }}
            unit other {{ main other_main; }}
            entry probe {G};
            entry other {G};
        }}"
    ));
    let callee = unit(&p, "callee_main");
    let v = explain_inline(&p).into_iter().find(|(u, _)| *u == callee).unwrap().1;
    assert_eq!(v, InlineVerdict::Rejected(InlineRejection::EvaluatedCaller));
}

#[test]
fn java_estimates_eval_methods_on_their_own() {
    let p = program();
    let rules = NamingRules::java();
    let interface = Interface::build(&p, &rules);
    let pools = tungsten_java::pools::Pools::new(&p, &interface, BufferBounds::PerBuffer);
    let mut codegen = tungsten_java::JavaCodegen::new(&p, &interface, &pools, &rules, "Gen");
    use tungsten_interface::DeviceCodegen;
    codegen.generate_kernel_of(vec![unit(&p, "top_main")]);
    let methods = codegen.methods();
    let eval = methods.iter().find(|m| m.name == "eval_probe_main_s1x1x1_1x1x1").expect("its own method");
    let top = methods.iter().find(|m| m.name.starts_with("top_main_d")).unwrap();
    let (e, t) = (tungsten_java::size::estimate_method(eval), tungsten_java::size::estimate_method(top));
    assert!(e > 0 && t > 0);
    // Growing the evaluated body grows its method only.
    let mut big = p.clone();
    let probe_fn = big.functions.iter().find(|(_, f)| f.canonical_name.as_deref() == Some("probe_main")).unwrap().0;
    let ret = big.functions[probe_fn].body[0].clone();
    for _ in 0..20 {
        big.functions[probe_fn].body.insert(0, ret.clone());
    }
    let interface = Interface::build(&big, &rules);
    let pools = tungsten_java::pools::Pools::new(&big, &interface, BufferBounds::PerBuffer);
    let mut codegen = tungsten_java::JavaCodegen::new(&big, &interface, &pools, &rules, "Gen");
    let top_unit = unit(&big, "top_main");
    codegen.generate_kernel_of(vec![top_unit]);
    let methods = codegen.methods();
    let eval2 = methods.iter().find(|m| m.name == "eval_probe_main_s1x1x1_1x1x1").unwrap();
    let top2 = methods.iter().find(|m| m.name.starts_with("top_main_d")).unwrap();
    assert!(tungsten_java::size::estimate_method(eval2) > e);
    assert_eq!(tungsten_java::size::estimate_method(top2), t);
}

/// The low-level path doesn't validate: every backend refuses what it can't emit.
#[test]
fn backends_refuse_eval_cycles_and_unindexed_reads() {
    use tungsten_interface::{Backend, BackendError};
    let cycle = parse(&format!(
        "program {{
            fn a_main() -> f64 {{ return eval(a, ambient(pos3)) {S}; }}
            unit a {{ main a_main; }}
            fn top_main() -> f64 {{ return eval(a, ambient(pos3)) {S}; }}
            unit top {{ main top_main; }}
            entry top {G};
        }}"
    ));
    let unindexed = parse(&format!(
        "program {{
            fn base_main() -> f64 {{ return 1.0; }}
            unit base {{ main base_main; }}
            input b = base {G};
            fn probe_main() -> f64 {{ return density(b); }}
            unit probe {{ main probe_main; inputs [b]; }}
            fn top_main() -> f64 {{ return eval(probe, ambient(pos3)) {S}; }}
            unit top {{ main top_main; }}
            entry top {G};
        }}"
    ));
    fn refuses<B: Backend>(b: &B, p: &Program) {
        let i = Interface::build(p, &b.naming_rules());
        assert!(matches!(b.generate(p, &i), Err(BackendError::Codegen(_))), "{}", b.name());
    }
    for p in [&cycle, &unindexed] {
        refuses(&tungsten_rust::backend::RustConfig::default().to_backend(), p);
        refuses(&tungsten_java::backend::JavaConfig::default().to_backend(), p);
        refuses(&tungsten_cuda::backend::CudaConfig::default().to_backend(), p);
    }
}
