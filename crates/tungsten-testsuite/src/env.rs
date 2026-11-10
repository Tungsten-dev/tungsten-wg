//! `TestEnv`: the `tungsten_parse::Env` and `tungsten_check::Registry` of the testsuite.

use tungsten_check::Registry;
use tungsten_ir::VariableType;
use tungsten_parse::{Env, ExternSig, FileId};

use crate::registry::RegistrySpec;

pub const PRELUDE: &str = include_str!("../prelude/prelude.tw");

/// The file id of the prelude; corpus files use `FileId(0)`.
pub const PRELUDE_FILE: FileId = FileId(1);

pub struct TestEnv {
    spec: RegistrySpec,
    pub registry: Registry,
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl TestEnv {
    pub fn new() -> Self {
        let spec = RegistrySpec::builtin();
        let registry = spec.build().expect("registry.toml builds");
        TestEnv { spec, registry }
    }

    pub fn spec(&self) -> &RegistrySpec {
        &self.spec
    }
}

impl Env for TestEnv {
    fn extern_fn(&self, name: &str) -> Option<ExternSig> {
        self.spec.externs.iter().find(|e| e.name == name).map(|e| ExternSig {
            parameter_types: e.params.clone(),
            return_type: e.ret.clone(),
        })
    }

    fn extern_type(&self, name: &str) -> Option<Vec<(&'static str, VariableType)>> {
        self.spec.records.iter().find(|r| r.name == name).map(|r| r.fields.clone())
    }

    fn library(&self, name: &str) -> Option<(FileId, &str)> {
        (name == "prelude").then_some((PRELUDE_FILE, PRELUDE))
    }

    fn file_name(&self, file: FileId) -> Option<String> {
        (file == PRELUDE_FILE).then(|| "prelude/prelude.tw".to_string())
    }
}
