use std::collections::HashMap;
use bincode::{Decode, Encode};
use crate::generic_type_parameter::GenericTypeParameter;
use crate::type_parameter::TypeParameter;
use crate::{FunctionTypeRegistry, RegistryKey, RegistryValue};

// Instance is more or less a subset of FunctionTypeRegistry (currently this function type registry corresponds
// to only 1 component)
// FunctionTypeRegistry is a collection of all the functions across all the interfaces and packages
// in a component, where as an InstanceType is a collection of functions that are available to a
// particular instance of a component - meaning specific to a ns:pkg, or interface or if ambiguous,
// ns:pkg/interface. Further disambiguation can be done with respect to version

#[derive(Debug, Clone, Eq, PartialEq, Encode, Decode)]
pub enum InstanceType {
    Durable {
        worker_name: String,
        component_id: String,
        type_parameter: TypeParameter, // Simply propagate the details of type-parameter from where InstanceType was formed
        registry: FunctionTypeRegistry, // This needs to be revisited
    },

    Ephemeral {
        component_id: String,
        type_parameter: TypeParameter, // Simply propagate the details of type-parameter from where InstanceType was formed
        registry: FunctionTypeRegistry, // This needs to be revisited
    },
}

impl InstanceType {
    // Handle the case when package name isn't specified

    // This is doubtful implementation. The function type registry is a collection of all the functions
    // across all the interfaces and packages in 1 component.
    // As such there is no resolution of a specific component here therefore.
    pub fn from_generic_type_parameter(
        generic_type_parameter: &GenericTypeParameter,
        component_metadata: FunctionTypeRegistry,
        worker_name: Option<String>,
    ) -> Result<InstanceType, String> {
        let instance_type = TypeParameter::from_str(&generic_type_parameter.value)?;

        let required_package_name = instance_type.get_package_name().ok_or(
            format!("Instance cannot be created from {} since it doesn't specify a package name", generic_type_parameter)
        )?;

        let mut new_registry: HashMap<RegistryKey, RegistryValue> = HashMap::new();

        for (k, v) in component_metadata.types {
            match &k {
                RegistryKey::FunctionNameWithInterface {
                    interface_name,
                    ..
                } => {
                    let instance = TypeParameter::from_str(&interface_name)?;

                    let package_name = instance.get_package_name().ok_or(
                        "Internal Error: Component Metadata doesn't have a package name".to_string()
                    )?;

                    if package_name == required_package_name {
                        new_registry.insert(k, v);
                    }
                }

                _ => todo!("Handle global cases")
            }
        }

        // We get a new instance module that corresponds to the optional instance type

        match worker_name {
            Some(worker_name) => Ok(InstanceType::Durable {
                component_id: "TODO".to_string(),
                worker_name,
                type_parameter: instance_type,
                registry: FunctionTypeRegistry {
                    types: new_registry,
                }
            }),

            None => Ok(InstanceType::Ephemeral {
                component_id: "TODO".to_string(),
                type_parameter: instance_type,
                registry: FunctionTypeRegistry {
                    types: new_registry,
                }
            })
        }

    }
}
