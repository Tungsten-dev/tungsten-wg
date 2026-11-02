//! L1: `parse(print(m)) ≅ compact(m)` and `print` is a fixpoint, for every corpus file.

use libtest_mimic::{Arguments, Failed, Trial};
use tungsten_ir::{compact, isomorphic};
use tungsten_parse::{FileId, load, print, print_test_file};
use tungsten_testsuite::corpus::{discover, load_file};
use tungsten_testsuite::env::TestEnv;

fn check(name: &str) -> Result<(), Failed> {
    let env = TestEnv::new();
    let f = discover().into_iter().find(|f| f.name == name).unwrap();
    let tf = load_file(f, &env).result.map_err(Failed::from)?;
    let text = print(&tf.program);
    let again = load(&text, FileId(0), &env).map_err(|d| Failed::from(format!("{d}\n{text}")))?;
    if !isomorphic(&again.program, &compact(&tf.program)) {
        return Err(format!("parse(print(m)) is not isomorphic to compact(m)\n{text}").into());
    }
    if print(&again.program) != text {
        return Err(format!("print is not a fixpoint\n{text}").into());
    }
    // The envelope prints and loads again too.
    let full = print_test_file(&tf);
    let tf2 = load(&full, FileId(0), &env).map_err(|d| Failed::from(format!("{d}\n{full}")))?;
    if print_test_file(&tf2) != full {
        return Err("print_test_file is not a fixpoint".to_string().into());
    }
    Ok(())
}

fn main() {
    let args = Arguments::from_args();
    let trials = discover()
        .into_iter()
        .map(|f| {
            let name = f.name.clone();
            Trial::test(format!("{name}::roundtrip"), move || check(&name))
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}
