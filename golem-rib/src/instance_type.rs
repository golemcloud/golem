use crate::parser::{PackageName, TypeParameter};
use crate::type_parameter::InterfaceName;
use crate::{DynamicParsedFunctionName, Expr, FunctionTypeRegistry, RegistryKey, RegistryValue};
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::AnalysedType;
use std::fmt::Display;

// InstanceType will be the type (`InferredType`) of the variable associated with creation of an instance
// This will be more or less a propagation of the original component metadata (structured as FunctionTypeRegistry),
// but with better structure and mandates the fact that it belongs to a specific component
// with better lookups in terms of namespace:package and interfaces.
// Here we will add the resource type as well as the resource creation itself can be be part of this InstanceType
// allowing lazy loading of resource and invoke the functions in them!
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

impl InstanceType {
    pub fn get_function(
        &self,
        function_name: &str,
        type_parameter: Option<TypeParameter>,
    ) -> Result<Function, String> {
        let functions: Vec<_> = self
            .function_dict()
            .map
            .iter()
            .filter(|(fqfn, _)| fqfn.function_name == function_name)
            .collect();

        match type_parameter {
            Some(param) => {
                for (fqfn, ftype) in &functions {
                    match &param {
                        TypeParameter::Interface(iface)
                            if fqfn.interface_name.as_ref() == Some(iface) =>
                        {
                            return Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            });
                        }
                        TypeParameter::PackageName(pkg)
                            if fqfn.package_name.as_ref() == Some(pkg) =>
                        {
                            return Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            });
                        }
                        TypeParameter::FullyQualifiedInterface(fq_iface)
                            if fqfn.package_name.as_ref() == Some(&fq_iface.package_name)
                                && fqfn.interface_name.as_ref()
                                    == Some(&fq_iface.interface_name) =>
                        {
                            return Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            });
                        }
                        _ => continue,
                    }
                }
                Err(format!(
                    "No function '{}' found for the given type parameter.",
                    function_name
                ))
            }
            None => {
                let unique_packages: Vec<_> = functions
                    .iter()
                    .filter_map(|(fqfn, _)| fqfn.package_name.as_ref())
                    .collect();
                let unique_interfaces: Vec<_> = functions
                    .iter()
                    .filter_map(|(fqfn, _)| fqfn.interface_name.as_ref())
                    .collect();

                match functions.len() {
                    0 => Err(format!("Function '{}' not found.", function_name)),
                    1 => Ok(Function {
                        function_name: functions[0].0.clone(),
                        function_type: functions[0].1.clone(),
                    }),
                    _ => {
                        let mut error_msg = format!(
                            "Multiple functions named '{}' found. Please specify a type parameter.",
                            function_name
                        );
                        if unique_packages.len() > 1 && unique_interfaces.len() > 1 {
                            error_msg.push_str(&format!(
                                " Conflicting locations: {:?}.",
                                unique_packages
                                    .iter()
                                    .flat_map(|pkg| {
                                        unique_interfaces.iter().map(move |iface| {
                                            format!("{}::{}", pkg.package_name, iface.name)
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            ));
                        } else if unique_packages.len() > 1 {
                            error_msg.push_str(&format!(
                                " Conflicting packages: {:?}.",
                                unique_packages
                                    .iter()
                                    .map(|pkg| format!("{}", pkg.package_name))
                                    .collect::<Vec<_>>()
                            ));
                        } else if unique_interfaces.len() > 1 {
                            error_msg.push_str(&format!(
                                " Conflicting interfaces: {:?}.",
                                unique_interfaces
                                    .iter()
                                    .map(|iface| iface.name.clone())
                                    .collect::<Vec<_>>()
                            ));
                        }
                        Err(error_msg)
                    }
                }
            }
        }
    }

    fn function_dict(&self) -> &FunctionDictionary {
        match self {
            InstanceType::Durable { function_dict, .. } => function_dict,
            InstanceType::Ephemeral { function_dict, .. } => function_dict,
        }
    }

    pub fn from(
        component_id: String,
        registry: FunctionTypeRegistry,
        worker_name: Option<Expr>,
    ) -> Result<InstanceType, String> {
        let function_dict = FunctionDictionary::from_function_type_registry(registry)?;

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

// TODO; This can be resource type too and not fully qualified function name
// But we will add this as part of tests
pub struct Function {
    pub function_name: FullyQualifiedFunctionName,
    pub function_type: FunctionType,
}
impl Function {
    pub fn dynamic_parsed_function_name(&self) -> Result<DynamicParsedFunctionName, String> {
        let name = self.function_name.to_string();
        DynamicParsedFunctionName::parse(name)
    }
}

// FunctionDictionary is a map of function names (not variant or any enums)
// to their respective function details
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct FunctionDictionary {
    pub map: Vec<(FullyQualifiedFunctionName, FunctionType)>,
}

impl FunctionDictionary {
    pub fn from_function_type_registry(
        registry: FunctionTypeRegistry,
    ) -> Result<FunctionDictionary, String> {
        let mut map = vec![];

        for (key, value) in registry.types {
            match value {
                RegistryValue::Function {
                    parameter_types,
                    return_types,
                } => match key {
                    RegistryKey::FunctionName(function_name) => {
                        map.push((
                            FullyQualifiedFunctionName {
                                package_name: None,
                                interface_name: None,
                                function_name,
                            },
                            FunctionType {
                                parameter_types,
                                return_type: return_types,
                            },
                        ));
                    }

                    RegistryKey::FunctionNameWithInterface {
                        interface_name,
                        function_name,
                    } => {
                        let type_parameter = TypeParameter::from_str(interface_name.as_str())?;

                        let interface_name = type_parameter.get_interface_name();
                        let package_name = type_parameter.get_package_name();

                        map.push((
                            FullyQualifiedFunctionName {
                                package_name,
                                interface_name,
                                function_name,
                            },
                            FunctionType {
                                parameter_types,
                                return_type: return_types,
                            },
                        ));
                    }
                },

                _ => continue,
            };
        }

        Ok(FunctionDictionary { map })
    }
}
#[derive(Debug, Hash, Clone, Eq, PartialEq, Encode, Decode)]
pub struct FullyQualifiedFunctionName {
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    function_name: String,
}

impl Display for FullyQualifiedFunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.package_name {
            Some(package_name) => write!(f, "{}:", package_name)?,
            None => {}
        }

        match &self.interface_name {
            Some(interface_name) => write!(f, "/{}.", interface_name)?,
            None => {}
        }

        write!(f, ".{{{}}}", self.function_name)
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Encode, Decode)]
pub struct FunctionType {
    parameter_types: Vec<AnalysedType>,
    return_type: Vec<AnalysedType>,
}
