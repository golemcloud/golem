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
        function_dict: FunctionDictionary,
    },

    Ephemeral {
        component_id: String,
        function_dict: FunctionDictionary,
    }
}

impl InstanceType {
    pub fn get_function(
        &self,
        function_name: &str,
        type_parameter: Option<TypeParameter>,
    ) -> Result<Function, String> {
        let functions: Vec<&(FunctionName, FunctionType)> = self
            .function_dict()
            .map
            .iter()
            .filter(|(f, _)| f.name() == function_name.to_string())
            .collect();

        match type_parameter {
            Some(param) => {
                for (fqfn, ftype) in &functions {
                    match &param {
                        TypeParameter::Interface(iface)
                            if fqfn.interface_name().as_ref() == Some(iface) =>
                        {
                            return Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            });
                        }
                        TypeParameter::PackageName(pkg)
                            if fqfn.package_name() == Some(pkg.clone()) =>
                        {
                            return Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            });
                        }
                        TypeParameter::FullyQualifiedInterface(fq_iface)
                            if fqfn.package_name() == Some(fq_iface.package_name.clone())
                                && fqfn.interface_name()
                                    == Some(fq_iface.interface_name.clone()) =>
                        {
                            return Ok(Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            });
                        }
                        TypeParameter::Interface(_) => {}
                        TypeParameter::PackageName(_) => {}
                        TypeParameter::FullyQualifiedInterface(_) => {}
                    }
                }
                Err(format!("No function '{}' found", function_name))
            }
            None => {
                if functions.is_empty() {
                    return Err(format!("Function '{}' not found.", function_name));
                }

                let mut package_map: HashMap<Option<PackageName>, HashSet<Option<InterfaceName>>> =
                    HashMap::new();

                for (fqfn, _) in &functions {
                    package_map
                        .entry(fqfn.package_name())
                        .or_insert_with(HashSet::new)
                        .insert(fqfn.interface_name());
                }

                match package_map.len() {
                    1 => {
                        let interfaces = package_map.values().flatten().cloned().collect();
                        handle_single_package_multiple_interfaces(
                            interfaces,
                            functions,
                            function_name,
                        )
                    }
                    _ => handle_multiple_packages_multiple_interfaces(function_name, package_map),
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
    pub function_name: FunctionName,
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
    pub map: Vec<(FunctionName, FunctionType)>,
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
                       let function_name = resolve_function_name(None, None, &function_name);

                        map.push((
                            function_name,
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

                        let function_name = resolve_function_name(package_name, interface_name, &function_name);

                        map.push((
                            function_name,
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

fn resolve_function_name(package_name: Option<PackageName>,  interface_name: Option<InterfaceName>, function_name: &str) -> FunctionName {
    match get_resource_name(&function_name) {
        Some(resource_name) => FunctionName::ResourceConstructor(FullyQualifiedResourceName {
            package_name: None,
            interface_name: None,
            resource_name,
        }),
        None => match get_resource_method_name(&function_name) {
            Some(resource_method_name) => FunctionName::ResourceMethod(resource_method_name),
            None => FunctionName::Function(FullyQualifiedFunctionName {
                package_name: None,
                interface_name: None,
                function_name: function_name.to_string(),
            }),
        }
    }
}

fn get_resource_name(function_name: &str) -> Option<String> {
    if function_name.starts_with("[constructor]") {
        Some(function_name.trim_start_matches("[constructor]").to_string())
    } else {
        None
    }
}

fn get_resource_method_name(function_name: &str) -> Option<String> {
    if function_name.starts_with("[method]") {
        Some(function_name.trim_start_matches("[method]").to_string())
    } else {
        None
    }
}


#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum FunctionName {
   Function(FullyQualifiedFunctionName),
   ResourceConstructor(FullyQualifiedResourceName),
   ResourceMethod(String)
}

impl FunctionName {
    pub fn interface_name(&self) -> Option<InterfaceName> {
        match self {
            FunctionName::Function(fqfn) => fqfn.interface_name.clone(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.interface_name.clone(),
            FunctionName::ResourceMethod(_) => None,
        }
    }

    pub fn package_name(&self) -> Option<PackageName> {
        match self {
            FunctionName::Function(fqfn) => fqfn.package_name.clone(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.package_name.clone(),
            FunctionName::ResourceMethod(_) => None,
        }
    }

    pub fn name(&self) -> String {
        match self {
            FunctionName::Function(fqfn) => fqfn.function_name.to_string(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.resource_name.to_string(),
            FunctionName::ResourceMethod(name) => name.to_string(),
        }
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedResourceName {
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    resource_name: String,
}


#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedFunctionName {
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    function_name: String,
}

impl Display for FullyQualifiedFunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(package_name) = &self.package_name {
            write!(f, "{}", package_name)?
        }

        if let Some(interface_name) = &self.interface_name {
            write!(f, "/{}.", interface_name)?
        }

        write!(f, "{{{}}}", self.function_name)
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct FunctionType {
    parameter_types: Vec<InferredType>,
    return_type: Vec<InferredType>,
}

fn handle_single_package_multiple_interfaces(
    interfaces: HashSet<Option<InterfaceName>>,
    functions: Vec<&(FunctionName, FunctionType)>,
    function_name: &str,
) -> Result<Function, String> {
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

fn handle_multiple_packages_multiple_interfaces(
    function_name: &str,
    package_map: HashMap<Option<PackageName>, HashSet<Option<InterfaceName>>>,
) -> Result<Function, String> {
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
