use std::collections::HashMap;
use crate::{Expr, FunctionTypeRegistry, RegistryKey, RegistryValue};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use crate::parser::{PackageName, TypeParameter};
use crate::type_parameter::InterfaceName;

// InstanceType will be the type (`InferredType`) of the variable associated with creation of an instance
// This will be more or less a propagation of the original component metadata (structured as FunctionTypeRegistry),
// but with better structure and mandates the fact that it belongs to a specific component or a specific namespace or package or interface within a package if needed
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub enum InstanceType {
    Durable {
        worker_name: Box<Expr>,
        component_id: String,
        function_dict: FunctionDictionary, // This needs to be revisited
    },

    Ephemeral {
        component_id: String,
        function_dict: FunctionDictionary, // This needs to be revisited
    },
}

// FunctionDictionary is a map of function names (not variant or any enums)
// to their respective function details
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct FunctionDictionary {
    pub map: Vec<(FunctionName, FunctionDetails)>,
}

impl FunctionDictionary {
    pub fn from_function_type_registry(registry: FunctionTypeRegistry) -> Result<FunctionDictionary, String> {
        let mut map = vec![];

        for (key, value) in registry.types {
            match value {
                RegistryValue::Function {
                    parameter_types,
                    return_types
                } => {

                    match key {
                        RegistryKey::FunctionName(function_name) => {
                            map.push(
                                (FunctionName {
                                    package_name: None,
                                    interface_name: None,
                                    function_name,
                                },
                                FunctionDetails {
                                    parameter_types,
                                    return_type: return_types,
                                }),
                            );
                        }

                        RegistryKey::FunctionNameWithInterface {
                            interface_name,
                            function_name,
                        } => {

                            let type_parameter = TypeParameter::from_str(
                                interface_name.as_str()
                            )?;

                            let interface_name = type_parameter.get_interface_name();
                            let package_name = type_parameter.get_package_name();

                            map.push(
                                (FunctionName {
                                    package_name,
                                    interface_name,
                                    function_name,
                                },
                                FunctionDetails {
                                    parameter_types,
                                    return_type: return_types,
                                }),
                            );
                        }
                    }


                }

                _ => continue,
            };
        }

        Ok(FunctionDictionary { map })
    }
}
#[derive(Debug, Hash, Clone, Eq, PartialEq, Encode, Decode)]
pub struct FunctionName {
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    function_name: String,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Encode, Decode)]
pub struct FunctionDetails {
    parameter_types: Vec<AnalysedType>,
    return_type: Vec<AnalysedType>
}

impl InstanceType {
    pub fn from(
        component_id: String,
        registry: FunctionTypeRegistry,
        worker_name: Option<Expr>,
    ) -> Result<InstanceType, String> {

        let function_dict =
            FunctionDictionary::from_function_type_registry(registry)?;

        match worker_name {
            Some(worker_name) => Ok(InstanceType::Durable {
                component_id,
                worker_name: Box::new(worker_name),
                function_dict,
            }),

            None => Ok(InstanceType::Ephemeral {
                component_id,
                function_dict,
            }),
        }
    }
}
