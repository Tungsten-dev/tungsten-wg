use tungsten_interface::naming::{NamingRules, cuda_sanitize};
use tungsten_interface::{Backend, BackendError, Interface, OrchestrationCodegen};
use tungsten_ir::model::OptimizedProgram;

use crate::{CudaCodegen, TransformCudaOrchestration};

/// Numeric profile, fixed at generation time (SPEC.md §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// Bitwise equal to the Rust output; ports of the Rust runtime's math, `--fmad=false`.
    #[default]
    Exact,
    /// Epsilon-equal; libdevice and intrinsics allowed, any flags.
    Tolerance,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::Exact => "exact",
            Profile::Tolerance => "tolerance",
        }
    }

    /// The `#define` at the top of `density.cu`.
    pub fn define(self) -> &'static str {
        match self {
            Profile::Exact => "TUNGSTEN_PROFILE_EXACT",
            Profile::Tolerance => "TUNGSTEN_PROFILE_TOLERANCE",
        }
    }

    /// File name of the profile's extern header, next to the runtime include.
    pub fn header(self) -> &'static str {
        match self {
            Profile::Exact => "profile_exact.cuh",
            Profile::Tolerance => "profile_fast.cuh",
        }
    }

    /// Flags the device code must be compiled with; empty when any flags work.
    pub fn required_flags(self) -> &'static str {
        match self {
            Profile::Exact => "--fmad=false --prec-div=true --prec-sqrt=true --ftz=false",
            Profile::Tolerance => "",
        }
    }

    /// `required_flags`, worded for the generated header comment.
    pub fn required_flags_comment(self) -> String {
        match self {
            Profile::Exact => format!("{} (never --use_fast_math)", self.required_flags()),
            Profile::Tolerance => "any (--use_fast_math allowed)".to_string(),
        }
    }
}

/// Qualifier of generated density and helper functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InlinePolicy {
    /// `__device__`: nvcc decides.
    #[default]
    Auto,
    /// `__device__ __forceinline__`
    Force,
    /// `__device__ __noinline__`
    Never,
}

/// Configuration of the CUDA backend. Invalid values panic in [`CudaBackend::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaConfig {
    /// Namespace of all generated C++ code. Device code goes in `<namespace>::dev`.
    pub namespace: String,
    /// Prefix of every `extern "C"` kernel symbol. `None` means `"<namespace>_"`.
    pub kernel_prefix: Option<String>,
    /// File names of the four outputs (also used in the generated `#include`s).
    pub public_header_name: String,
    pub interface_header_name: String,
    pub density_source_name: String,
    pub orchestration_source_name: String,
    /// Include path of the runtime entry header. The profile header sits next to it.
    pub runtime_include: String,
    pub profile: Profile,
    /// Max threads per block; a power of two in 32..=1024.
    pub block_size: u32,
    /// Pinned staging slots for `HostConsts` uploads (at least 1).
    pub host_consts_ring: u32,
    /// Emit bounds and division checks (SPEC.md §13).
    pub debug_checks: bool,
    pub inline_policy: InlinePolicy,
}

impl Default for CudaConfig {
    fn default() -> Self {
        Self {
            namespace: "tungsten_gen".to_string(),
            kernel_prefix: None,
            public_header_name: "orchestration.h".to_string(),
            interface_header_name: "interface.cuh".to_string(),
            density_source_name: "density.cu".to_string(),
            orchestration_source_name: "orchestration.cu".to_string(),
            runtime_include: "tw/tungsten_runtime.cuh".to_string(),
            profile: Profile::Exact,
            block_size: 128,
            host_consts_ring: 2,
            debug_checks: false,
            inline_policy: InlinePolicy::Auto,
        }
    }
}

impl CudaConfig {
    pub fn to_backend(self) -> CudaBackend {
        CudaBackend::new(self)
    }

    /// The kernel symbol prefix, defaulting to `<namespace>_`.
    pub fn effective_kernel_prefix(&self) -> String {
        self.kernel_prefix.clone().unwrap_or_else(|| format!("{}_", self.namespace))
    }

    /// Include path of the profile header: next to `runtime_include`.
    pub fn profile_include(&self) -> String {
        match self.runtime_include.rfind('/') {
            Some(i) => format!("{}{}", &self.runtime_include[..=i], self.profile.header()),
            None => self.profile.header().to_string(),
        }
    }

    /// Panics on values the generated code can't work with.
    fn validate(&self) {
        let identifier = |s: &str| {
            !s.is_empty()
                && !s.starts_with(|c: char| c.is_ascii_digit())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        assert!(
            identifier(&self.namespace) && cuda_sanitize(&self.namespace) == self.namespace,
            "CudaConfig::namespace `{}` is not a valid C++ identifier",
            self.namespace
        );
        if let Some(prefix) = &self.kernel_prefix {
            assert!(
                prefix.is_empty() || identifier(prefix),
                "CudaConfig::kernel_prefix `{}` is not a valid identifier prefix",
                prefix
            );
        }
        assert!(
            self.block_size.is_power_of_two() && (32..=1024).contains(&self.block_size),
            "CudaConfig::block_size must be a power of two in 32..=1024, got {}",
            self.block_size
        );
        assert!(self.host_consts_ring >= 1, "CudaConfig::host_consts_ring must be at least 1");
    }
}

/// Generated CUDA sources, available after [`Backend::generate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaOutput {
    /// `orchestration.h`: CUDA-free public API.
    pub public_header: String,
    /// `interface.cuh`: device structs and kernel declarations.
    pub interface_header: String,
    /// `density.cu`: device code.
    pub density_source: String,
    /// `orchestration.cu`: host orchestration.
    pub orchestration_source: String,
    /// Non-fatal diagnostics (device and orchestration).
    pub warnings: Vec<String>,
    file_names: [String; 4],
}

impl CudaOutput {
    /// `(file name, contents)` of every generated file.
    pub fn files(&self) -> Vec<(String, String)> {
        let contents = [
            &self.public_header,
            &self.interface_header,
            &self.density_source,
            &self.orchestration_source,
        ];
        self.file_names.iter().cloned().zip(contents.into_iter().cloned()).collect()
    }
}

/// CUDA backend: generates the four CUDA C++ files for an optimized program.
#[derive(Debug)]
pub struct CudaBackend {
    config: CudaConfig,
    output: Option<CudaOutput>,
    /// Reserves the namespace and kernel prefix so no generated name collides with them.
    naming_rules: NamingRules,
}

impl CudaBackend {
    pub fn new(config: CudaConfig) -> Self {
        config.validate();
        let mut reserved = vec![config.namespace.clone()];
        let prefix = config.effective_kernel_prefix();
        if !prefix.is_empty() {
            reserved.push(prefix);
        }
        let naming_rules = NamingRules::cuda().with_reserved(reserved);
        Self { config, output: None, naming_rules }
    }

    /// The generated code, or `None` before a successful [`Backend::generate`].
    pub fn output(&self) -> Option<&CudaOutput> {
        self.output.as_ref()
    }

    pub fn config(&self) -> &CudaConfig {
        &self.config
    }
}

impl Backend for CudaBackend {
    fn name(&self) -> &str {
        "cuda"
    }

    fn generate(&mut self, program: &OptimizedProgram, interface: &Interface) -> Result<(), BackendError> {
        let device = CudaCodegen::new(program, interface, &self.naming_rules, &self.config);
        let mut orchestration = TransformCudaOrchestration::new(Box::new(device), &self.config, &self.naming_rules);
        orchestration.convert_program(program, interface);
        let files = orchestration.files();
        let c = &self.config;
        self.output = Some(CudaOutput {
            public_header: files.public_header,
            interface_header: files.interface_header,
            density_source: files.density_source,
            orchestration_source: files.orchestration_source,
            warnings: orchestration.warnings(),
            file_names: [
                c.public_header_name.clone(),
                c.interface_header_name.clone(),
                c.density_source_name.clone(),
                c.orchestration_source_name.clone(),
            ],
        });
        Ok(())
    }

    fn naming_rules(&self) -> &NamingRules {
        &self.naming_rules
    }
}
