use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::collections::HashMap;

// Acts as the structure to hold the global input values
#[derive(Debug, Default)]
pub struct RibInterpreterInput {
    pub input: HashMap<String, TypeAnnotatedValue>,
}

impl RibInterpreterInput {
    pub fn new(input: HashMap<String, TypeAnnotatedValue>) -> RibInterpreterInput {
        RibInterpreterInput { input }
    }
}
