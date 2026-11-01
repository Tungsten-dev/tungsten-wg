use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tungsten_interface::naming::NamingRules;
use tungsten_interface::{Backend, BackendError, Interface};
use tungsten_ir::model::OptimizedProgram;

use crate::model::Inspection;
use crate::{analyze, render};

/// What to emit. All view families are on by default. `inspection.json` is always emitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectConfig {
    pub views: Views,
    pub element_sizes: ElementSizes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Views {
    /// `index.md`
    pub index: bool,
    /// `ir/program.txt`
    pub ir_text: bool,
    /// Per-entry dependency and wave graphs.
    pub entries: bool,
    /// Per-entry buffer charts and tables.
    pub buffers: bool,
    /// Per-function DAGs (the expensive one). When off, the model has no DAGs either.
    pub dataflow: bool,
    /// Call graph and resource graph.
    pub program_graphs: bool,
}

impl Default for Views {
    fn default() -> Self {
        Self {
            index: true,
            ir_text: true,
            entries: true,
            buffers: true,
            dataflow: false,
            program_graphs: true,
        }
    }
}

/// Bytes per buffer element. Defaults model the Rust runtime.
/// Override e.g. `f64 = 4` to model a GPU backend that downcasts.
///
/// `Array(t, n)` is `n × size(t)`. Other types (`DensityInput`, `PermutationTable`,
/// `InterpolatedNoiseSampler`, `Extern`) are not valid buffer elements: size 0 and a note in
/// [`Inspection::notes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementSizes {
    pub f32: usize,
    pub f64: usize,
    pub i8: usize,
    pub i32: usize,
    pub i64: usize,
    pub bool: usize,
    /// 3 × f64
    pub vec3: usize,
    /// 3 × i32
    pub pos3: usize,
}

impl Default for ElementSizes {
    fn default() -> Self {
        Self {
            f32: 4,
            f64: 8,
            i8: 1,
            i32: 4,
            i64: 8,
            bool: 1,
            vec3: 24,
            pos3: 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InspectOutput {
    /// Relative path → file contents. A `BTreeMap` so iteration order is stable.
    pub files: BTreeMap<PathBuf, String>,
    pub inspection: Inspection,
}

impl InspectOutput {
    /// Creates directories as needed and overwrites existing files. Never deletes anything,
    /// so stale files from earlier runs stay unless the caller uses a fresh directory.
    pub fn write_to(&self, dir: &Path) -> std::io::Result<()> {
        for (path, contents) in &self.files {
            let path = dir.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, contents)?;
        }
        Ok(())
    }
}

/// Inspection backend: emits diagrams, an IR dump and a JSON model instead of code.
#[derive(Debug)]
pub struct InspectBackend {
    config: InspectConfig,
    output: Option<InspectOutput>,
    naming_rules: NamingRules,
}

impl InspectBackend {
    pub fn new(config: InspectConfig) -> Self {
        Self {
            config,
            output: None,
            naming_rules: NamingRules::inspect(),
        }
    }

    /// The output, or `None` before a successful [`Backend::generate`].
    pub fn output(&self) -> Option<&InspectOutput> {
        self.output.as_ref()
    }
}

impl Backend for InspectBackend {
    fn name(&self) -> &str {
        "inspect"
    }

    fn generate(
        &mut self,
        program: &OptimizedProgram,
        interface: &Interface,
    ) -> Result<(), BackendError> {
        self.output = Some(run(program, interface, &self.config));
        Ok(())
    }

    fn naming_rules(&self) -> &NamingRules {
        &self.naming_rules
    }
}

/// Standalone: builds its own `Interface` with [`NamingRules::inspect`], then follows the same
/// path as [`InspectBackend`], so both give identical output.
pub fn inspect(program: &OptimizedProgram, config: &InspectConfig) -> InspectOutput {
    let interface = Interface::build(program, &NamingRules::inspect());
    run(program, &interface, config)
}

fn run(program: &OptimizedProgram, interface: &Interface, config: &InspectConfig) -> InspectOutput {
    let options = analyze::Options {
        dataflow: config.views.dataflow,
        element_sizes: config.element_sizes.clone(),
    };
    let mut inspection = analyze::build(program, interface, &options);
    let files = render::render(&mut inspection, config);
    InspectOutput { files, inspection }
}
