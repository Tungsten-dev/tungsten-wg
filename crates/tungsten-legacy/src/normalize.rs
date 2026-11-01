use crate::spmt::model::{Expression, MainDensityFunction, Var};

/// Normalization utilities for SPMT programs.
/// These utilities help in normalizing SPMT programs, ensuring consistent formatting and structure.

pub trait NormalizedSPMT<'m> {
    fn normal_density_list(&self) -> Vec<super::model::DensityFunctionRef<'m>>;
    fn normal_main_density_list(&self) -> Vec<MainDensityFunction<'m>>;
}

pub trait NormalizedDensityFunction<'m> {
    fn normal_density_inputs(&self) -> Vec<super::model::DensityInput<'m>>;
    fn normal_permutation_table_inputs(&self) -> Vec<super::model::PermutationTableInput>;
    fn normal_variables(&self) -> Vec<Var<'m>>;
    fn normal_constants(&self) -> Vec<(Var<'m>, Expression<'m>)>;
}   

pub trait NormalizedFunction<'m> {
    fn normal_parameters(&self) -> Vec<super::model::Var<'m>>;
}

// pub fn normalize_spmt_program(program: &mut super::model::SPMT<'_>) {
//     // sort functions by source hash for consistent ordering between runs
//     program.main_density_functions.sort_by(|a, b| a.0.source_hash.cmp(&b.0.source_hash));
//     program.density_functions.sort_by(|a, b| a.source_hash.cmp(&b.source_hash));
    
//     for func in &mut program.main_density_functions {
//         // sort density inputs by density function for consistent ordering
//         func.0.density_inputs.sort_by(|a, b| 
//             a.density_function.source_hash.cmp(&b.density_function.source_hash));

//         func.0.permutation_table_inputs.sort();
//         func.0.variables.sort_by(|a, b| a.name.cmp(&b.name));
//         func.0.constants.sort_by(|a, b| a.0.name.cmp(&b.0.name));
//     }
    
//     for func in &mut program.density_functions {
//         // sort density inputs by density function for consistent ordering
//         func.density_inputs.sort_by(|a, b| 
//             a.density_function.source_hash.cmp(&b.density_function.source_hash));

//         func.permutation_table_inputs.sort();
//         func.variables.sort_by(|a, b| a.name.cmp(&b.name));
//         func.constants.sort_by(|a, b| a.0.name.cmp(&b.0.name));
//     }
// }

impl<'m> NormalizedSPMT<'m> for super::model::SPMT<'m> {
    fn normal_density_list(&self) -> Vec<super::model::DensityFunctionRef<'m>> {
        let mut list = self.density_functions.clone();
        list.sort_by(|a, b| a.source_hash.cmp(&b.source_hash));
        list
    }

    fn normal_main_density_list(&self) -> Vec<MainDensityFunction<'m>> {
        let mut list = self.main_density_functions.clone();
        list.sort_by(|a, b| a.density_function.source_hash.cmp(&b.density_function.source_hash));
        list
    }
}

impl<'m> NormalizedDensityFunction<'m> for super::model::DensityFunction<'m> {
    fn normal_density_inputs(&self) -> Vec<super::model::DensityInput<'m>> {
        let mut list = self.density_inputs.clone();
        list.sort_by(|a, b| a.density_function.source_hash.cmp(&b.density_function.source_hash));
        list
    }  
    fn normal_permutation_table_inputs(&self) -> Vec<super::model::PermutationTableInput> {
        let mut list = self.permutation_table_inputs.clone();
        list.sort();
        list
    }
    fn normal_variables(&self) -> Vec<Var<'m>> {
        let mut list = self.variables.clone();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
    fn normal_constants(&self) -> Vec<(Var<'m>, Expression<'m>)> {
        let mut list = self.constants.clone();
        list.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        list
    }
}

impl<'m> NormalizedFunction<'m> for super::model::Function<'m> {
    fn normal_parameters(&self) -> Vec<super::model::Var<'m>> {
        let mut list = self.parameters.clone();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}
