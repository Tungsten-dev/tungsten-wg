//! L3 and L5 on the interpreter: `::load`, `::interp`, `::inline` for every corpus file, plus the
//! selection and runtime self-checks. Backend trials arrive in milestone 2.

use libtest_mimic::{Arguments, Failed, Trial};
use tungsten_testsuite::corpus::{CorpusFile, Loaded, discover, load_file};
use tungsten_testsuite::env::TestEnv;
use tungsten_testsuite::oracle;
use tungsten_testsuite::selection::Selection;

fn is_large(tf: &tungsten_parse::TestFile) -> bool {
    tf.tier.as_deref() == Some("large")
}

fn main() {
    let args = Arguments::from_args();
    let sel = match Selection::from_env() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let env = TestEnv::new();
    let mut trials = vec![];

    for file in discover() {
        let Loaded { file, result } = load_file(file, &env);
        match result {
            Err(msg) => {
                trials.push(Trial::test(format!("{}::load", file.name), move || Err(Failed::from(msg))));
            }
            Ok(tf) => {
                let ignore = is_large(&tf) && !sel.large;
                let reject = tf.reject.is_some();
                let mut add = |suffix: &str, run: fn(CorpusFile) -> Result<(), Failed>| {
                    let f = file.clone();
                    let t = Trial::test(format!("{}::{suffix}", file.name), move || run(f));
                    trials.push(if ignore {
                        t.with_ignored_flag(true).with_kind("large (set TW_LARGE=1)")
                    } else {
                        t
                    });
                };
                add("interp", interp);
                if !reject {
                    add("inline", inline);
                }
            }
        }
    }

    trials.push(Trial::test("runtime_externs_registered", runtime_externs_registered));
    libtest_mimic::run(&args, trials).exit();
}

fn load_ok(file: CorpusFile) -> Result<(TestEnv, tungsten_parse::TestFile), Failed> {
    let env = TestEnv::new();
    let tf = load_file(file, &env).result.map_err(Failed::from)?;
    Ok((env, tf))
}

fn interp(file: CorpusFile) -> Result<(), Failed> {
    let (env, tf) = load_ok(file)?;
    if tf.reject.is_some() {
        return oracle::check_reject(&tf, &env).map_err(Failed::from);
    }
    let run = oracle::run_all(&tf, &env)?;
    oracle::check_expects(&tf, &env, &run)?;
    Ok(())
}

fn inline(file: CorpusFile) -> Result<(), Failed> {
    let (env, tf) = load_ok(file)?;
    let run = oracle::run_all(&tf, &env)?;
    oracle::check_inline(&tf, &env, &run)?;
    Ok(())
}

/// Every registered extern has an implementation in `runtime/rust` (a missing one panics in
/// `CaseExterns::call`, so call each with well-typed arguments).
fn runtime_externs_registered() -> Result<(), Failed> {
    use tungsten_check::{Externs, Value};
    use tungsten_core::{F64x3, TableId};
    let env = TestEnv::new();
    let map = tungsten_check::LowerMap {
        externs: env.spec().externs.iter().map(|e| e.name.clone()).collect(),
        table_idents: vec![
            tungsten_ir::PermutationTableInput::PerlinNoise {
                ident: "x".into(),
                subident: None,
                subident_index: 0,
            },
            tungsten_ir::PermutationTableInput::Base3DNoise,
        ],
        ..Default::default()
    };
    let ext = tungsten_testsuite::externs::CaseExterns::new(&map, 1);
    for (i, e) in env.spec().externs.iter().enumerate() {
        let args: Vec<Value> = e
            .params
            .iter()
            .map(|t| {
                use tungsten_ir::VariableType as T;
                match t {
                    T::F64 => Value::F64(1.5f64.to_bits()),
                    T::F32 => Value::F32(1.5f32.to_bits()),
                    T::I32 => Value::I32(3),
                    T::I64 => Value::I64(3),
                    T::Vec3 => Value::Vec3(F64x3 { x: 0, y: 0, z: 0 }),
                    T::PermutationTable => Value::PermTable(TableId(0)),
                    T::InterpolatedNoiseSampler => Value::Sampler(TableId(1)),
                    other => panic!("no sample argument for {other:?}"),
                }
            })
            .collect();
        ext.call(tungsten_core::ExternId(i), &args)
            .map_err(|_| Failed::from(format!("extern `{}` failed on sample arguments", e.name)))?;
    }
    Ok(())
}
