/* 
transformation!(DeviceCodegenRCL: OptimizedProgram=O -> RCL=R;
    map O::Model -> R::Model
    map O::Function -> R::Function
    map O::Constant -> R::Constant
    fn  O::Statement -> R::Statement
    fn  O::Expression -> R::Expression
    fn  O::Type -> R::Type
    map O::Var @ O::Function -> R::Var @ R::Function
);
*/

// use std::collections::HashMap;
// use crate::spmt_optimize::model::Ref;
// use crate::transform_rcl_new

// pub struct DeviceCodegenRCL {
//     mapping_model_model: HashMap<Ref<>
// }