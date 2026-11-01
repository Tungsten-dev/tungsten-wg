//! Facade over the `tungsten-*` workspace crates.
//!
//! Build an [`OptimizedProgram`](ir::model::OptimizedProgram), create the backends you need, and
//! pass them to [`compile`]. Each backend keeps its own output.

pub use tungsten_codegen as codegen;
pub use tungsten_interface as interface;
pub use tungsten_ir as ir;
pub use tungsten_legacy as spmt;
pub use tungsten_optimizer as optimizer;

pub use tungsten_legacy::{body, build, extern_fns, pos3, vec3};

#[cfg(feature = "rust")]
pub use tungsten_rust as rust;
#[cfg(feature = "rust")]
pub use tungsten_rust::backend::{RustBackend, RustConfig, RustOutput};

#[cfg(feature = "java")]
pub use tungsten_java as java;
#[cfg(feature = "java")]
pub use tungsten_java::backend::{JavaBackend, JavaConfig, JavaOutput};

#[cfg(feature = "cuda")]
pub use tungsten_cuda as cuda;
#[cfg(feature = "cuda")]
pub use tungsten_cuda::backend::{CudaBackend, CudaConfig, CudaOutput};

#[cfg(feature = "inspect")]
pub use tungsten_inspect as inspect;
#[cfg(feature = "inspect")]
pub use tungsten_inspect::{InspectBackend, InspectConfig, InspectOutput};

pub use tungsten_interface::{Backend, BackendError};

use tungsten_interface::Interface;
use tungsten_ir::model::OptimizedProgram;

/// Errors from [`compile`].
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// A backend failed. `name` is its [`Backend::name`].
    #[error("backend `{name}` failed")]
    Backend {
        name: String,
        #[source]
        source: BackendError,
    },
}

/// Compiles an optimized program with the given backends.
///
/// Each backend receives the same shared [`Interface`] built from the program and its naming rules.
/// Stops at the first failing backend. Backends that already ran keep their output.
pub fn compile(
    program: &OptimizedProgram,
    backends: &mut [&mut dyn Backend],
) -> Result<(), CompileError> {
    for backend in backends.iter_mut() {
        let interface = Interface::build(program, backend.naming_rules());
        backend
            .generate(program, &interface)
            .map_err(|source| CompileError::Backend {
                name: backend.name().to_string(),
                source,
            })?;
    }
    Ok(())
}
