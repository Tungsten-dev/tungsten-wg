//! A backend defined outside the workspace crates: Cargo builds this file as its own crate, so
//! it only sees the public API. If it stops compiling, custom backends are no longer first-class.

use std::borrow::Cow;
use std::sync::Arc;

use tungsten_wg::interface::{Interface, naming::NamingRules};
use tungsten_wg::ir::model::Program;
use tungsten_wg::{Backend, BackendError, CompileError, Compiler, Inline, Passes, check};

struct MyConfig {
    level: u8,
    warn: bool,
}

struct MyBackend {
    config: MyConfig,
    rules: NamingRules,
}

struct MyOutput {
    text: String,
    warnings: Vec<String>,
}

impl MyBackend {
    fn new() -> Self {
        Self { config: MyConfig { level: 0, warn: false }, rules: NamingRules::inspect() }
    }
}

impl Backend for MyBackend {
    type Output = MyOutput;

    fn name(&self) -> &str {
        "mine"
    }

    fn naming_rules(&self) -> Cow<'_, NamingRules> {
        Cow::Borrowed(&self.rules)
    }

    fn validate(&self) -> Result<(), BackendError> {
        if self.config.level > 3 {
            return Err(BackendError::InvalidConfig(format!("level {}", self.config.level)));
        }
        Ok(())
    }

    fn default_passes(&self) -> Passes {
        Passes::default().inline(Inline::All)
    }

    fn generate(&self, p: &Program, _: &Interface) -> Result<MyOutput, BackendError> {
        Ok(MyOutput {
            text: format!("level {}, {} unit(s)", self.config.level, p.compute_units.iter().count()),
            warnings: if self.config.warn { vec!["careful".into()] } else { vec![] },
        })
    }

    fn warnings(o: &MyOutput) -> &[String] {
        &o.warnings
    }
}

/// Local to this crate, so the orphan rule allows the impl on `Compiler<MyBackend>`.
trait MyCompile: Sized {
    fn level(self, n: u8) -> Self;
    fn warn(self) -> Self;
}

impl MyCompile for Compiler<MyBackend> {
    fn level(mut self, n: u8) -> Self {
        self.backend_mut().config.level = n;
        self
    }
    fn warn(mut self) -> Self {
        self.backend_mut().config.warn = true;
        self
    }
}

fn program() -> Program {
    use tungsten_wg::builder::prelude::*;
    let mut pb = ProgramBuilder::new();
    let mut f = FnBuilder::new("m", F64);
    f.ret(1.0);
    let u = pb.finish_unit(f);
    pb.entry(u, Grid::new((1, 1, 1)));
    pb.build().unwrap()
}

fn registry() -> Arc<check::Registry> {
    Arc::new(check::RegistryBuilder::new().build().unwrap())
}

#[test]
fn setter_passes_and_deny_warnings_work() {
    let reg = registry();
    let out = Compiler::new(program(), reg.clone(), MyBackend::new()).level(2).compile().unwrap();
    assert_eq!(out.text, "level 2, 1 unit(s)");

    // An explicit setting wins over the backend's default.
    Compiler::new(program(), reg.clone(), MyBackend::new()).passes(Passes::default()).compile().unwrap();

    let r = Compiler::new(program(), reg.clone(), MyBackend::new()).warn().deny_warnings().compile();
    assert!(matches!(r, Err(CompileError::Warnings { .. })));
    Compiler::new(program(), reg.clone(), MyBackend::new()).warn().compile().unwrap();
}

#[test]
fn invalid_config_is_an_error() {
    let reg = registry();
    let r = Compiler::new(program(), reg.clone(), MyBackend::new()).level(9).compile();
    assert!(matches!(r, Err(CompileError::Backend { source: BackendError::InvalidConfig(_), .. })));
}
