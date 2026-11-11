//! Temporary, `specs/combined-rollout.md` §3.1: between a feature landing in the IR and a backend's
//! own phase, the backend refuses programs that use it instead of generating wrong code. Each
//! backend calls this first, listing what it implements. Deleted in phase 4.

use tungsten_ir::model::{AxisFactor, DensityMapping, OptimizedProgram};

use crate::BackendError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Mappings,
    Refinement,
}

/// `BackendError::Codegen` naming the first feature `program` uses outside `implemented`.
pub fn check_implemented(
    program: &OptimizedProgram,
    implemented: &[Feature],
) -> Result<(), BackendError> {
    // `Expand([Repeat; 3])` reads cell 0 of a `(1,1,1)` input, which every backend already does.
    let cell_zero = DensityMapping::Expand([AxisFactor::Repeat; 3]);
    let mapped = program
        .density_inputs
        .values()
        .any(|d| d.mapping != DensityMapping::Identity && d.mapping != cell_zero);
    if mapped && !implemented.contains(&Feature::Mappings) {
        return Err(BackendError::Codegen(
            "density mappings are not implemented on this backend yet".into(),
        ));
    }
    let refined = !program.refinements.is_empty() || program.density_inputs.values().any(|d| d.previous);
    if refined && !implemented.contains(&Feature::Refinement) {
        return Err(BackendError::Codegen(
            "refinement is not implemented on this backend yet".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungsten_ir::model::{
        Arena, ComputeUnit, DensityInput, Function, Name, Refinement, VariableType,
    };

    fn program(mapping: DensityMapping, previous: bool) -> OptimizedProgram {
        let mut p = OptimizedProgram {
            compute_units: Arena::new(),
            functions: Arena::new(),
            entry_points: vec![],
            refinements: vec![],
            constants: Arena::new(),
            host_inputs: Arena::new(),
            auto_params: Arena::new(),
            auto_consts: Arena::new(),
            density_inputs: Arena::new(),
        };
        let function = p.functions.insert(Function {
            canonical_name: None,
            host_inputs: vec![],
            body: vec![],
            variables: Arena::new(),
            parameters: vec![],
            return_type: VariableType::F64,
            source_hash: 0,
        });
        let density_function = p.compute_units.insert(ComputeUnit::DensityFunction {
            function,
            density_inputs: vec![],
            auto_consts: vec![],
            host_inputs: vec![],
            helper_functions: vec![],
            constants: vec![],
            source_hash: 0,
        });
        p.density_inputs.insert(DensityInput {
            name: Name::Anonymous,
            density_function,
            scaled_origin: (1.0, 1.0, 1.0),
            scaled_position: (1.0, 1.0, 1.0),
            dimensions: (1, 1, 1),
            mapping,
            previous,
        });
        p
    }

    fn refused(p: &OptimizedProgram, implemented: &[Feature]) -> Option<String> {
        match check_implemented(p, implemented) {
            Ok(()) => None,
            Err(BackendError::Codegen(m)) => Some(m),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn identity_and_cell_zero_expand_pass() {
        assert_eq!(refused(&program(DensityMapping::Identity, false), &[]), None);
        let m = DensityMapping::Expand([AxisFactor::Repeat; 3]);
        assert_eq!(refused(&program(m, false), &[]), None);
    }

    #[test]
    fn other_mappings_are_refused_until_implemented() {
        use AxisFactor::{Factor, Repeat};
        for m in [
            DensityMapping::Expand([Factor(1), Repeat, Factor(1)]),
            DensityMapping::Expand([Factor(1); 3]),
            DensityMapping::Lerp([Repeat; 3]),
            DensityMapping::Lerp([Factor(4), Factor(8), Factor(4)]),
        ] {
            let p = program(m, false);
            assert!(refused(&p, &[]).unwrap().contains("mappings"), "{m:?}");
            assert!(refused(&p, &[Feature::Refinement]).unwrap().contains("mappings"), "{m:?}");
            assert_eq!(refused(&p, &[Feature::Mappings]), None, "{m:?}");
        }
    }

    #[test]
    fn refinement_and_previous_are_refused_until_implemented() {
        let previous = program(DensityMapping::Identity, true);
        let mut declared = program(DensityMapping::Identity, false);
        declared.refinements.push(Refinement {
            name: "r".into(),
            entry: 0,
            march: (0, 0, 0),
            reruns: vec![],
        });
        for p in [&previous, &declared] {
            assert!(refused(p, &[]).unwrap().contains("refinement"));
            assert!(refused(p, &[Feature::Mappings]).unwrap().contains("refinement"));
            assert_eq!(refused(p, &[Feature::Refinement]), None);
        }
    }
}
