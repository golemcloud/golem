use crate::generic_type_parameter::GenericTypeParameter;
use crate::type_parameter::TypeParameter;
use crate::{Expr, FunctionTypeRegistry, RegistryKey, RegistryValue};
use bincode::{Decode, Encode};
use std::collections::HashMap;

// InstanceType will be the type (`InferredType`) of the variable associated with creation of an instance
// This will be more or less a propagation of the original component metadata (structured as FunctionTypeRegistry),
// but with better structure and mandates the fact that it belongs to a specific component or a specific namespace or package or interface within a package if needed
#[derive(Debug, Clone, Eq, PartialEq, Encode, Decode)]
pub enum InstanceType {
    Durable {
        worker_name: Expr,
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
        worker_name: Option<Expr>,
    ) -> InstanceType {
        match worker_name {
            Some(worker_name) => InstanceType::Durable {
                component_id,
                worker_name,
                registry,
            },

            None => InstanceType::Ephemeral {
                component_id,
                registry,
            },
        }
    }
}
