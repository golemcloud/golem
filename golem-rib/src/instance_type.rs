use crate::parser::{PackageName, TypeParameter};
use crate::type_parameter::InterfaceName;
use crate::{Expr, FunctionTypeRegistry, InferredType, RegistryKey, RegistryValue, };
use std::collections::{HashMap, HashSet};
use std::fmt::Display;

// InstanceType will be the type (`InferredType`) of the variable associated with creation of an instance
// This will be more or less a propagation of the original component metadata (structured as FunctionTypeRegistry),
// but with better structure and mandates the fact that it belongs to a specific component
// with better lookups in terms of namespace:package and interfaces.
// Here we will add the resource type as well as the resource creation itself can be be part of this InstanceType
// allowing lazy loading of resource and invoke the functions in them!
// The distinction is only to disallow compiler to see only the functions that are part of a location (package/interface/package-interface/resoruce or all)
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub enum InstanceType {
    // Holds functions across every package and interface in the component
    Global {
        worker_name: Option<Box<Expr>>,
        component_id: String,
        function_dict: FunctionDictionary,
    },

    // Holds functions across every interface in the package
    Package {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        component_id: String,
        function_dict: FunctionDictionary,
    },

    // Holds all functions across (may be across packages) for a specific interface
    Interface {
        worker_name: Option<Box<Expr>>,
        interface_name: InterfaceName,
        component_id: String,
        function_dict: FunctionDictionary,
    },

    // Most granular level, holds functions for a specific package and interface
    PackageInterface {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        interface_name: InterfaceName,
        component_id: String,
        function_dict: FunctionDictionary,
    },

    // Holds the resource creation and the functions in the resource
    // that may or may not be addressed
    Resource {
        worker_name: Option<Box<Expr>>,
        package_name: Option<PackageName>,
        interface_name: Option<InterfaceName>,
        resource_args: Vec<Expr>,
        component_id: String,
        function_dict: ResourceMethodDictionary,
    }
}

impl InstanceType {
    pub fn get_function(
        &self,
        function_name: &str,
        type_parameter: Option<TypeParameter>,
    ) -> Result<Function, String> {
        match type_parameter {
            Some(tp) => {
                match tp {
                    TypeParameter::Interface(iface) => {
                        let functions = self
                            .function_dict()
                            .map
                            .iter()
                            .filter(|(f, _)| f.interface_name() == Some(iface.clone()))
                            .collect::<Vec<_>>();

                        if functions.is_empty() {
                            return Err(format!("Function '{}' not found in interface '{}'", function_name, iface));
                        }

                        search_function_in_instance(
                            self,
                            function_name,
                        )
                    }

                    TypeParameter::PackageName(pkg) => {
                        let functions = self
                            .function_dict()
                            .map
                            .iter()
                            .filter(|(f, _)| f.package_name() == Some(pkg.clone()))
                            .collect::<Vec<_>>();

                        if functions.is_empty() {
                            return Err(format!("Function '{}' not found in package '{}'", function_name, pkg));
                        }

                        search_function_in_instance(
                            self,
                            function_name,
                        )
                    }

                    TypeParameter::FullyQualifiedInterface(fq_iface) => {
                        let functions = self
                            .function_dict()
                            .map
                            .iter()
                            .filter(|(f, _)| f.package_name() == Some(fq_iface.package_name.clone()) && f.interface_name() == Some(fq_iface.interface_name.clone()))
                            .collect::<Vec<_>>();

                        if functions.is_empty() {
                            return Err(format!("Function '{}' not found in interface '{}'", function_name, fq_iface));
                        }

                        search_function_in_instance(
                            self,
                            function_name,
                        )
                    }
                }
            }
            None => {
                search_function_in_instance(
                    self,
                    function_name,
                )
            }
        }
    }

    fn function_dict(&self) -> &FunctionDictionary {
        match self {
            InstanceType::Global { function_dict, .. } => function_dict,
            InstanceType::Package { function_dict, .. } => function_dict,
            InstanceType::Interface { function_dict, .. } => function_dict,
            InstanceType::PackageInterface { function_dict, .. } => function_dict,
            InstanceType::Resource { function_dict, .. } => function_dict.into(),
        }
    }

    pub fn from(
        component_id: String,
        registry: FunctionTypeRegistry,
        worker_name: Option<Expr>,
        type_parameter: Option<TypeParameter>,
    ) -> Result<InstanceType, String> {
        let function_dict =
            FunctionDictionary::from_function_type_registry(registry)?;

        match type_parameter {
            None => {
                Ok(InstanceType::Global {
                    component_id,
                    worker_name: worker_name.map(Box::new),
                    function_dict,
                })
            }
            Some(type_parameter) => {
                match type_parameter {
                    TypeParameter::Interface(interface_name) => {
                        let function_dict =
                            function_dict.map.iter().filter(|(f, _)| f.interface_name() == Some(interface_name.clone())).collect();

                        Ok(InstanceType::Interface {
                            component_id,
                            worker_name: worker_name.map(Box::new),
                            interface_name,
                            function_dict,
                        })
                    }
                    TypeParameter::PackageName(package_name) => {
                        let function_dict =
                            function_dict.map.iter().filter(|(f, _)| f.package_name() == Some(package_name.clone())).collect();

                        Ok(InstanceType::Package {
                            component_id,
                            worker_name: worker_name.map(Box::new),
                            package_name,
                            function_dict,
                        })
                    }
                    TypeParameter::FullyQualifiedInterface(fq_interface) => {
                        let function_dict = function_dict.map.iter().filter(|(f, _)| f.package_name() == Some(fq_interface.package_name.clone()) && f.interface_name() == Some(fq_interface.interface_name.clone())).collect();

                        Ok(InstanceType::PackageInterface {
                            component_id,
                            worker_name: worker_name.map(Box::new),
                            package_name: fq_interface.package_name,
                            interface_name: fq_interface.interface_name,
                            function_dict,
                        })
                    }
                }
            }
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

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FunctionDictionary {
    pub map: Vec<(FunctionName, FunctionType)>,
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ResourceMethodDictionary {
    map: Vec<(FullyQualifiedResourceMethod, FunctionType)>,
}

impl From<ResourceMethodDictionary> for FunctionDictionary {
    fn from(value: ResourceMethodDictionary) -> Self {
        FunctionDictionary {
            map: value.map.into_iter().map(|(k, v)| (FunctionName::ResourceMethod(k), v)).collect(),
        }
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ResourceMethod {
    constructor_name: String,
    resource_name: String
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

                        let function_name =
                            resolve_function_name(package_name, interface_name, &function_name);

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
        Some(resource_name) => FunctionName::ResourceConstructor(FullyQualifiedResourceConstructor {
            package_name,
            interface_name,
            resource_name,
        }),
        None => match get_resource_method_name(&function_name) {
            Ok(Some((constructor, method))) => FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                package_name,
                interface_name,
                resource_name: constructor,
                method_name: method,
            }),
            None => FunctionName::Function(FullyQualifiedFunctionName {
                package_name,
                interface_name,
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

fn get_resource_method_name(function_name: &str) -> Result<Option<(String, String)> , String> {
    if function_name.starts_with("[method]") {
        let constructor_and_method = function_name.trim_start_matches("[method]").to_string();
        let mut constructor_and_method = constructor_and_method.split('.');
        let constructor = constructor_and_method.next();
        let method = constructor_and_method.next();

        match (constructor, method) {
            (Some(constructor), Some(method)) => Ok(Some((constructor.to_string(), method.to_string()))),
            _ => Err(format!("Invalid resource method name: {}", function_name))
        }

    } else {
        Ok(None)
    }
}


#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum FunctionName {
   Function(FullyQualifiedFunctionName),
   ResourceConstructor(FullyQualifiedResourceConstructor),
   ResourceMethod(FullyQualifiedResourceMethod)
}

impl FunctionName {
    pub fn interface_name(&self) -> Option<InterfaceName> {
        match self {
            FunctionName::Function(fqfn) => fqfn.interface_name.clone(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.interface_name.clone(),
            FunctionName::ResourceMethod(resource_method) => resource_method.interface_name.clone(),
        }
    }

    pub fn package_name(&self) -> Option<PackageName> {
        match self {
            FunctionName::Function(fqfn) => fqfn.package_name.clone(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.package_name.clone(),
            FunctionName::ResourceMethod(fqfn) => fqfn.package_name.clone(),
        }
    }

    pub fn name(&self) -> String {
        match self {
            FunctionName::Function(fqfn) => fqfn.function_name.to_string(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.resource_name.to_string(),
            FunctionName::ResourceMethod(fqfn) => fqfn.method_name.to_string(),
        }
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedResourceConstructor {
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

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedResourceMethod {
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    resource_name: String,
    method_name: String,
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

fn search_function_in_instance(
    instance: &InstanceType,
    function_name: &str
) -> Result<Function, String> {
    let functions: Vec<&(FunctionName, FunctionType)> = instance
        .function_dict()
        .map
        .iter()
        .filter(|(f, _)| f.name() == function_name.to_string())
        .collect();

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
            search_function_in_single_package(
                interfaces,
                functions,
                function_name,
            )
        }
        _ => search_function_in_multiple_packages(function_name, package_map),
    }
}

fn search_function_in_single_package(
    interfaces: HashSet<Option<InterfaceName>>,
    functions: Vec<&(FunctionName, FunctionType)>,
    function_name: &str,
) -> Result<Function, String> {
    if interfaces.len() == 1 {
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

fn search_function_in_multiple_packages(
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
