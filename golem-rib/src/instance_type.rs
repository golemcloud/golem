use crate::parser::{PackageName, TypeParameter};
use crate::type_parameter::InterfaceName;
use crate::{
    DynamicParsedFunctionName, Expr, FunctionTypeRegistry, InferredType, RegistryKey, RegistryValue,
};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;

// InstanceType will be the type (`InferredType`) of the variable associated with creation of an instance
// This will be more or less a propagation of the original component metadata (structured as FunctionTypeRegistry),
// but with better structure and mandates the fact that it belongs to a specific component
// with better lookups in terms of namespace:package and interfaces.
// Here we will add the resource type as well as the resource creation itself can be be part of this InstanceType
// allowing lazy loading of resource and invoke the functions in them!
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
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
        let functions: Vec<&(FullyQualifiedFunctionName, FunctionType)> = self
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
                if functions.is_empty() {
                    return Err(format!("Function '{}' not found.", function_name));
                }

                // Group functions by package name
                let mut package_map: HashMap<Option<PackageName>, HashSet<Option<InterfaceName>>> =
                    HashMap::new();

                for (fqfn, _) in &functions {
                    package_map
                        .entry(fqfn.package_name.clone())
                        .or_insert_with(HashSet::new)
                        .insert(fqfn.interface_name.clone());
                }

                match package_map.len() {
                    1 => {
                        // Only one package, check if multiple interfaces exist
                        let (_, interfaces) = package_map.into_iter().next().unwrap();
                        if interfaces.len() == 1 {
                            // Single package, single interface -> Return the function
                            let (fqfn, ftype) = functions[0];
                            Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            })
                        } else {
                            let mut interfaces = interfaces
                                .into_iter()
                                .filter_map(|iface| iface.map(|i| i.name))
                                .collect::<Vec<_>>();

                            interfaces.sort();

                            // Multiple interfaces in the same package -> Ask for an interface name
                            Err(format!(
                                "Multiple interfaces contain function '{}'. Specify an interface name as type parameter from: {}",
                                function_name,
                                interfaces
                                    .join(", ")
                            ))
                        }
                    }
                    _ => {
                        // Multiple packages -> Ask for package first
                        let mut error_msg = format!(
                            "Function '{}' exists in multiple packages. Specify a package name as type parameter from: ",
                            function_name
                        );

                        let mut package_interface_list = package_map
                            .into_iter()
                            .filter_map(|(pkg, interfaces)| {
                                pkg.map(|p| {
                                    let mut interface_list = interfaces
                                        .into_iter()
                                        .filter_map(|iface| iface.map(|i| i.name))
                                        .collect::<Vec<_>>();

                                    interface_list.sort();

                                    if interface_list.is_empty() {
                                        format!("{}", p)
                                    } else {
                                        format!("{} (interfaces: {})", p, interface_list.join(", "))
                                    }
                                })
                            })
                            .collect::<Vec<_>>();

                        package_interface_list.sort();

                        error_msg.push_str(&package_interface_list.join(", "));
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
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
                                parameter_types: parameter_types
                                    .into_iter()
                                    .map(|x| x.into())
                                    .collect(),
                                return_type: return_types.into_iter().map(|x| x.into()).collect(),
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
                                parameter_types: parameter_types
                                    .into_iter()
                                    .map(|x| x.into())
                                    .collect(),
                                return_type: return_types.into_iter().map(|x| x.into()).collect(),
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
#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedFunctionName {
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    function_name: String,
}

impl Display for FullyQualifiedFunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(package_name) = &self.package_name { write!(f, "{}", package_name)? }

        if let Some(interface_name) = &self.interface_name { write!(f, "/{}.", interface_name)? }

        write!(f, "{{{}}}", self.function_name)
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct FunctionType {
    parameter_types: Vec<InferredType>,
    return_type: Vec<InferredType>,
}
