use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::collections::HashMap;

// Acts as the structure to hold the global input values
#[derive(Debug, Default, Clone)]
pub struct RibInput {
    pub input: HashMap<String, TypeAnnotatedValue>,
}

impl RibInput {
    pub fn empty() -> RibInput {
        RibInput {
            input: HashMap::default()
        }
    }

    pub fn new(input: HashMap<String, TypeAnnotatedValue>) -> RibInput {
        RibInput { input }
    }

    pub fn merge(&self, other: RibInput) -> RibInput {
        let mut cloned = self.clone();
        cloned.input.extend(other.input);
        cloned
    }
}
