use std::collections::HashMap;
use bincode::{Decode, Encode};
use crate::generic_type_parameter::GenericTypeParameter;
use crate::type_parameter::TypeParameter;
use crate::{FunctionTypeRegistry, RegistryKey, RegistryValue};

// InstanceType will be the type (`InferredType`) of the variable associated with creation of an instance
// This will be more or less a propagation of the original component metadata (structured as FunctionTypeRegistry),
// but with better structure
#[derive(Debug, Clone, Eq, PartialEq, Encode, Decode)]
pub enum InstanceType {
    Durable {
        worker_name: String,
        component_id: String,
        registry: FunctionTypeRegistry, // This needs to be revisited
    },

    Ephemeral {
        component_id: String,
        registry: FunctionTypeRegistry, // This needs to be revisited
    },
}

impl InstanceType {
    pub fn from(
        component_id: String,
        registry: FunctionTypeRegistry,
        worker_name: Option<String>,
    ) -> Result<InstanceType, String> {
        match worker_name {
            Some(worker_name) => Ok(InstanceType::Durable {
                component_id: "TODO".to_string(),
                worker_name,
                registry
            }),

            None => Ok(InstanceType::Ephemeral {
                component_id: "TODO".to_string(),
                registry
            })
        }
    }
}
