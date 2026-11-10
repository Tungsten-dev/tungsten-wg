//! L2: lowering invariants and an `insta` snapshot of the lowered core program. Regression files
//! are excluded (their contents are generated and would only add churn).

use libtest_mimic::{Arguments, Failed, Trial};
use tungsten_check::lower;
use tungsten_testsuite::corpus::{CorpusFile, discover, load_file};
use tungsten_testsuite::env::TestEnv;

fn check(file: CorpusFile) -> Result<(), Failed> {
    let env = TestEnv::new();
    let name = file.name.clone();
    let is_reject = file_is_reject(&file, &env);
    let tf = load_file(file, &env).result.map_err(Failed::from)?;
    let lowered = match lower(&tf.program, &env.registry) {
        Ok(l) => l,
        Err(e) if is_reject => {
            let _ = e;
            return Ok(());
        }
        Err(e) => return Err(format!("lowering failed:\n{e}").into()),
    };
    // Deterministic.
    let again = lower(&tf.program, &env.registry).map_err(|e| Failed::from(e.to_string()))?;
    if again != lowered {
        return Err("lower is not deterministic".to_string().into());
    }
    // Arity preserved, and every id resolves back to a ref.
    let p = &lowered.program;
    let ir = &tf.program;
    let n = |what: &str, a: usize, b: usize| -> Result<(), Failed> {
        if a == b { Ok(()) } else { Err(format!("{what}: {a} in the IR, {b} after lowering").into()) }
    };
    n("units", ir.compute_units.iter().count(), p.units.len())?;
    n("functions", ir.functions.iter().count(), p.functions.len())?;
    n("consts", ir.constants.iter().count(), p.consts.len())?;
    n("hosts", ir.host_inputs.iter().count(), p.hosts.len())?;
    n("tables", ir.permutation_tables.iter().count(), p.tables.len())?;
    n("entries", ir.entry_points.len(), p.entries.len())?;
    for (i, f) in p.functions.iter().enumerate() {
        let id = tungsten_core::FnId(i);
        let r = lowered
            .map
            .functions
            .get(id)
            .ok_or_else(|| Failed::from(format!("fn {i} has no ref")))?;
        n(&format!("fn {i} parameters"), ir.functions[r].parameters.len(), f.params)?;
        // Density-input vars are not locals in the core IR.
        let vars = ir.functions[r]
            .variables
            .values()
            .filter(|v| v.t != tungsten_ir::VariableType::DensityInput)
            .count();
        n(&format!("fn {i} locals"), vars, f.locals.len())?;
        n(&format!("fn {i} hosts"), ir.functions[r].host_inputs.len(), f.hosts.len())?;
    }
    if is_reject {
        return Ok(()); // no snapshot for programs the checker rejects
    }
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path("snapshots");
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_debug_snapshot!(name.replace('/', "__"), lowered.program);
    });
    Ok(())
}

fn file_is_reject(file: &CorpusFile, env: &TestEnv) -> bool {
    load_file(file.clone(), env).result.map(|t| t.reject.is_some()).unwrap_or(false)
}

fn main() {
    let args = Arguments::from_args();
    let trials = discover()
        .into_iter()
        .filter(|f| !f.is_regression())
        .map(|f| {
            let name = format!("{}::lowering", f.name);
            Trial::test(name, move || check(f))
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}
