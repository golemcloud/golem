use crate::parser::{PackageName, TypeParameter};
use crate::type_parameter::InterfaceName;
use crate::{
    DynamicParsedFunctionName, Expr, FunctionTypeRegistry, InferredType, RegistryKey, RegistryValue,
};
use golem_api_grpc::proto::golem::rib::instance_type::Instance;
use golem_api_grpc::proto::golem::rib::{
    function_name_type, FullyQualifiedFunctionName as ProtoFullyQualifiedFunctionName,
    FullyQualifiedResourceConstructor as ProtoFullyQualifiedResourceConstructor,
    FullyQualifiedResourceMethod as ProtoFullyQualifiedResourceMethod,
    FunctionDictionary as ProtoFunctionDictionary, FunctionNameType as ProtoFunctionName,
    FunctionType as ProtoFunctionType, InstanceType as ProtoInstanceType,
    InterfaceName as ProtoInterfaceName, PackageName as ProtoPackageName,
    ResourceMethodDictionary as ProtoResourceMethodDictionary,
};
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt::{Debug, Display};
use std::ops::Deref;

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
        functions_global: FunctionDictionary,
    },

    // Holds functions across every interface in the package
    Package {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        functions_in_package: FunctionDictionary,
    },

    // Holds all functions across (may be across packages) for a specific interface
    Interface {
        worker_name: Option<Box<Expr>>,
        interface_name: InterfaceName,
        functions_in_interface: FunctionDictionary,
    },

    // Most granular level, holds functions for a specific package and interface
    PackageInterface {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        interface_name: InterfaceName,
        functions_in_package_interface: FunctionDictionary,
    },

    // Holds the resource creation and the functions in the resource
    // that may or may not be addressed
    Resource {
        worker_name: Option<Box<Expr>>,
        package_name: Option<PackageName>,
        interface_name: Option<InterfaceName>,
        resource_constructor: String,
        resource_args: Vec<Expr>,
        resource_method_dict: ResourceMethodDictionary,
    },
}

impl InstanceType {
    pub fn set_worker_name(&mut self, worker_name: Expr) {
        match self {
            InstanceType::Global {
                worker_name: wn, ..
            } => {
                *wn = Some(Box::new(worker_name));
            }
            InstanceType::Package {
                worker_name: wn, ..
            } => {
                *wn = Some(Box::new(worker_name));
            }
            InstanceType::Interface {
                worker_name: wn, ..
            } => {
                *wn = Some(Box::new(worker_name));
            }
            InstanceType::PackageInterface {
                worker_name: wn, ..
            } => {
                *wn = Some(Box::new(worker_name));
            }
            InstanceType::Resource {
                worker_name: wn, ..
            } => {
                *wn = Some(Box::new(worker_name));
            }
        }
    }

    pub fn worker_mut(&mut self) -> Option<&mut Box<Expr>> {
        match self {
            InstanceType::Global { worker_name, .. } => worker_name.as_mut(),
            InstanceType::Package { worker_name, .. } => worker_name.as_mut(),
            InstanceType::Interface { worker_name, .. } => worker_name.as_mut(),
            InstanceType::PackageInterface { worker_name, .. } => worker_name.as_mut(),
            InstanceType::Resource { worker_name, .. } => worker_name.as_mut(),
        }
    }

    pub fn worker(&self) -> Option<&Expr> {
        match self {
            InstanceType::Global { worker_name, .. } => worker_name.as_ref().map(|v| v.deref()),
            InstanceType::Package { worker_name, .. } => worker_name.as_ref().map(|v| v.deref()),
            InstanceType::Interface { worker_name, .. } => worker_name.as_ref().map(|v| v.deref()),
            InstanceType::PackageInterface { worker_name, .. } => {
                worker_name.as_ref().map(|v| v.deref())
            }
            InstanceType::Resource { worker_name, .. } => worker_name.as_ref().map(|v| v.deref()),
        }
    }

    // Get InstanceType::Resource from the fully qualified resource constructor
    // from an existing instance type
    pub fn get_resource_instance_type(
        &self,
        fully_qualified_resource_constructor: FullyQualifiedResourceConstructor,
        resource_args: Vec<Expr>,
        worker_name: Option<Box<Expr>>,
    ) -> InstanceType {
        let interface_name = fully_qualified_resource_constructor.interface_name.clone();
        let package_name = fully_qualified_resource_constructor.package_name.clone();
        let resource_constructor_name = fully_qualified_resource_constructor.resource_name.clone();

        let mut resource_method_dict = vec![];
        for (f, function_type) in self.function_dict().name_and_types.iter() {
            if let FunctionName::ResourceMethod(resource_method) = f {
                if resource_method.resource_name == resource_constructor_name
                    && resource_method.interface_name == interface_name
                    && resource_method.package_name == package_name
                {
                    resource_method_dict.push((resource_method.clone(), function_type.clone()));
                }
            }
        }

        let resource_method_dict = ResourceMethodDictionary {
            map: resource_method_dict,
        };

        InstanceType::Resource {
            worker_name,
            package_name,
            interface_name,
            resource_constructor: resource_constructor_name,
            resource_args,
            resource_method_dict,
        }
    }

    pub fn interface_name(&self) -> Option<InterfaceName> {
        match self {
            InstanceType::Global { .. } => None,
            InstanceType::Package { .. } => None,
            InstanceType::Interface { interface_name, .. } => Some(interface_name.clone()),
            InstanceType::PackageInterface { interface_name, .. } => Some(interface_name.clone()),
            InstanceType::Resource { interface_name, .. } => interface_name.clone(),
        }
    }

    pub fn package_name(&self) -> Option<PackageName> {
        match self {
            InstanceType::Global { .. } => None,
            InstanceType::Package { package_name, .. } => Some(package_name.clone()),
            InstanceType::Interface { .. } => None,
            InstanceType::PackageInterface { package_name, .. } => Some(package_name.clone()),
            InstanceType::Resource { package_name, .. } => package_name.clone(),
        }
    }

    pub fn worker_name(&self) -> Option<Box<Expr>> {
        match self {
            InstanceType::Global { worker_name, .. } => worker_name.clone(),
            InstanceType::Package { worker_name, .. } => worker_name.clone(),
            InstanceType::Interface { worker_name, .. } => worker_name.clone(),
            InstanceType::PackageInterface { worker_name, .. } => worker_name.clone(),
            InstanceType::Resource { worker_name, .. } => worker_name.clone(),
        }
    }
    pub fn get_function(
        &self,
        method_name: &str,
        type_parameter: Option<TypeParameter>,
    ) -> Result<Function, String> {
        match type_parameter {
            Some(tp) => match tp {
                TypeParameter::Interface(iface) => {
                    let interfaces = self
                        .function_dict()
                        .name_and_types
                        .into_iter()
                        .filter(|(f, _)| f.interface_name() == Some(iface.clone()))
                        .collect::<Vec<_>>();

                    if interfaces.is_empty() {
                        return Err(format!("Interface '{}' not found", iface));
                    }

                    let functions = interfaces
                        .into_iter()
                        .filter(|(f, _)| f.name() == method_name)
                        .collect::<Vec<_>>();

                    if functions.is_empty() {
                        return Err(format!(
                            "Function '{}' not found in interface '{}'",
                            method_name, iface
                        ));
                    }

                    // There is only 1 interface, and there cannot exist any more conflicts
                    // with an interface
                    if functions.len() == 1 {
                        let (fqfn, ftype) = &functions[0];
                        Ok(Function {
                            function_name: fqfn.clone(),
                            function_type: ftype.clone(),
                        })
                    } else {
                        search_function_in_instance(self, method_name)
                    }
                }

                TypeParameter::PackageName(pkg) => {
                    let packages = self
                        .function_dict()
                        .name_and_types
                        .into_iter()
                        .filter(|(f, _)| f.package_name() == Some(pkg.clone()))
                        .collect::<Vec<_>>();

                    if packages.is_empty() {
                        return Err(format!("package '{}' not found", pkg));
                    }

                    let functions = packages
                        .into_iter()
                        .filter(|(f, _)| f.name() == method_name)
                        .collect::<Vec<_>>();

                    if functions.is_empty() {
                        return Err(format!(
                            "function '{}' not found in package '{}'",
                            method_name, pkg
                        ));
                    }

                    if functions.len() == 1 {
                        let (fqfn, ftype) = &functions[0];
                        Ok(Function {
                            function_name: fqfn.clone(),
                            function_type: ftype.clone(),
                        })
                    } else {
                        search_function_in_instance(self, method_name)
                    }
                }

                TypeParameter::FullyQualifiedInterface(fq_iface) => {
                    let functions = self
                        .function_dict()
                        .name_and_types
                        .into_iter()
                        .filter(|(f, _)| {
                            f.package_name() == Some(fq_iface.package_name.clone())
                                && f.interface_name() == Some(fq_iface.interface_name.clone())
                                && f.name() == method_name
                        })
                        .collect::<Vec<_>>();

                    if functions.is_empty() {
                        return Err(format!(
                            "function '{}' not found in interface '{}'",
                            method_name, fq_iface
                        ));
                    }

                    if functions.len() == 1 {
                        let (fqfn, ftype) = &functions[0];
                        Ok(Function {
                            function_name: fqfn.clone(),
                            function_type: ftype.clone(),
                        })
                    } else {
                        search_function_in_instance(self, method_name)
                    }
                }
            },
            None => search_function_in_instance(self, method_name),
        }
    }

    pub fn resource_method_dictionary(&self) -> FunctionDictionary {
        let name_and_types = self
            .function_dict()
            .name_and_types
            .into_iter()
            .filter(|(f, _)| matches!(f, FunctionName::ResourceMethod(_)))
            .collect::<Vec<_>>();

        FunctionDictionary { name_and_types }
    }

    pub fn function_dict_without_resource_methods(&self) -> FunctionDictionary {
        let name_and_types = self
            .function_dict()
            .name_and_types
            .into_iter()
            .filter(|(f, _)| !matches!(f, FunctionName::ResourceMethod(_)))
            .collect::<Vec<_>>();

        FunctionDictionary { name_and_types }
    }

    pub fn function_dict(&self) -> FunctionDictionary {
        match self {
            InstanceType::Global {
                functions_global: function_dict,
                ..
            } => function_dict.clone(),
            InstanceType::Package {
                functions_in_package: function_dict,
                ..
            } => function_dict.clone(),
            InstanceType::Interface {
                functions_in_interface: function_dict,
                ..
            } => function_dict.clone(),
            InstanceType::PackageInterface {
                functions_in_package_interface: function_dict,
                ..
            } => function_dict.clone(),
            InstanceType::Resource {
                resource_method_dict,
                ..
            } => resource_method_dict.into(),
        }
    }

    pub fn from(
        registry: &FunctionTypeRegistry,
        worker_name: Option<&Expr>,
        type_parameter: Option<TypeParameter>,
    ) -> Result<InstanceType, String> {
        let function_dict = FunctionDictionary::from_function_type_registry(registry)?;

        match type_parameter {
            None => Ok(InstanceType::Global {
                worker_name: worker_name.cloned().map(Box::new),
                functions_global: function_dict,
            }),
            Some(type_parameter) => match type_parameter {
                TypeParameter::Interface(interface_name) => {
                    let name_and_types = function_dict
                        .name_and_types
                        .into_iter()
                        .filter(|(f, _)| f.interface_name() == Some(interface_name.clone()))
                        .collect::<Vec<_>>();

                    if name_and_types.is_empty() {
                        return Err(format!("interface `{}` not found", interface_name));
                    }

                    let function_dict = FunctionDictionary { name_and_types };

                    Ok(InstanceType::Interface {
                        worker_name: worker_name.cloned().map(Box::new),
                        interface_name,
                        functions_in_interface: function_dict,
                    })
                }
                TypeParameter::PackageName(package_name) => {
                    let name_and_types = function_dict
                        .name_and_types
                        .into_iter()
                        .filter(|(f, _)| f.package_name() == Some(package_name.clone()))
                        .collect::<Vec<_>>();

                    if name_and_types.is_empty() {
                        return Err(format!("package `{}` not found", package_name));
                    }

                    let function_dict = FunctionDictionary { name_and_types };

                    Ok(InstanceType::Package {
                        worker_name: worker_name.cloned().map(Box::new),
                        package_name,
                        functions_in_package: function_dict,
                    })
                }
                TypeParameter::FullyQualifiedInterface(fq_interface) => {
                    let name_and_types = function_dict
                        .name_and_types
                        .into_iter()
                        .filter(|(f, _)| {
                            f.package_name() == Some(fq_interface.package_name.clone())
                                && f.interface_name() == Some(fq_interface.interface_name.clone())
                        })
                        .collect::<Vec<_>>();

                    if name_and_types.is_empty() {
                        return Err(format!("`{}` not found", fq_interface));
                    }

                    let function_dict = FunctionDictionary { name_and_types };

                    Ok(InstanceType::PackageInterface {
                        worker_name: worker_name.cloned().map(Box::new),
                        package_name: fq_interface.package_name,
                        interface_name: fq_interface.interface_name,
                        functions_in_package_interface: function_dict,
                    })
                }
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Function {
    pub function_name: FunctionName,
    pub function_type: FunctionType,
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd, Default)]
pub struct FunctionDictionary {
    pub name_and_types: Vec<(FunctionName, FunctionType)>,
}

impl FunctionDictionary {
    pub fn function_names(&self) -> Vec<String> {
        self.name_and_types
            .iter()
            .map(|(f, _)| f.name())
            .collect::<Vec<_>>()
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ResourceMethodDictionary {
    pub map: Vec<(FullyQualifiedResourceMethod, FunctionType)>,
}

impl From<&ResourceMethodDictionary> for FunctionDictionary {
    fn from(value: &ResourceMethodDictionary) -> Self {
        FunctionDictionary {
            name_and_types: value
                .map
                .iter()
                .map(|(k, v)| (FunctionName::ResourceMethod(k.clone()), v.clone()))
                .collect(),
        }
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ResourceMethod {
    constructor_name: String,
    resource_name: String,
}

impl FunctionDictionary {
    pub fn from_function_type_registry(
        registry: &FunctionTypeRegistry,
    ) -> Result<FunctionDictionary, String> {
        let mut map = vec![];

        for (key, value) in registry.types.iter() {
            match value {
                RegistryValue::Function {
                    parameter_types,
                    return_types,
                } => match key {
                    RegistryKey::FunctionName(function_name) => {
                        let function_name = resolve_function_name(None, None, function_name)?;

                        map.push((
                            function_name,
                            FunctionType {
                                parameter_types: parameter_types.iter().map(|x| x.into()).collect(),
                                return_type: return_types.iter().map(|x| x.into()).collect(),
                            },
                        ));
                    }

                    RegistryKey::FunctionNameWithInterface {
                        interface_name,
                        function_name,
                    } => {
                        let type_parameter = TypeParameter::from_text(interface_name.as_str())?;

                        let interface_name = type_parameter.get_interface_name();
                        let package_name = type_parameter.get_package_name();

                        let function_name =
                            resolve_function_name(package_name, interface_name, function_name)?;

                        map.push((
                            function_name,
                            FunctionType {
                                parameter_types: parameter_types.iter().map(|x| x.into()).collect(),
                                return_type: return_types.iter().map(|x| x.into()).collect(),
                            },
                        ));
                    }
                },

                _ => continue,
            };
        }

        Ok(FunctionDictionary {
            name_and_types: map,
        })
    }
}

fn resolve_function_name(
    package_name: Option<PackageName>,
    interface_name: Option<InterfaceName>,
    function_name: &str,
) -> Result<FunctionName, String> {
    match get_resource_name(function_name) {
        Some(resource_name) => Ok(FunctionName::ResourceConstructor(
            FullyQualifiedResourceConstructor {
                package_name,
                interface_name,
                resource_name,
            },
        )),
        None => match get_resource_method_name(function_name) {
            Ok(Some((constructor, method))) => {
                Ok(FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                    package_name,
                    interface_name,
                    resource_name: constructor,
                    method_name: method,
                }))
            }
            Ok(None) => Ok(FunctionName::Function(FullyQualifiedFunctionName {
                package_name,
                interface_name,
                function_name: function_name.to_string(),
            })),

            Err(e) => Err(format!("invalid function call. {}", e)),
        },
    }
}

fn get_resource_name(function_name: &str) -> Option<String> {
    if function_name.starts_with("[constructor]") {
        Some(
            function_name
                .trim_start_matches("[constructor]")
                .to_string(),
        )
    } else {
        None
    }
}

fn get_resource_method_name(function_name: &str) -> Result<Option<(String, String)>, String> {
    if function_name.starts_with("[method]") {
        let constructor_and_method = function_name.trim_start_matches("[method]").to_string();
        let mut constructor_and_method = constructor_and_method.split('.');
        let constructor = constructor_and_method.next();
        let method = constructor_and_method.next();

        match (constructor, method) {
            (Some(constructor), Some(method)) => {
                Ok(Some((constructor.to_string(), method.to_string())))
            }
            _ => Err(format!("Invalid resource method name: {}", function_name)),
        }
    } else if function_name.starts_with("[drop]") {
        let constructor = function_name.trim_start_matches("[drop]").to_string();
        Ok(Some((constructor, "drop".to_string())))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum FunctionName {
    Function(FullyQualifiedFunctionName),
    ResourceConstructor(FullyQualifiedResourceConstructor),
    ResourceMethod(FullyQualifiedResourceMethod),
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
    pub package_name: Option<PackageName>,
    pub interface_name: Option<InterfaceName>,
    pub resource_name: String,
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedFunctionName {
    pub package_name: Option<PackageName>,
    pub interface_name: Option<InterfaceName>,
    pub function_name: String,
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct FullyQualifiedResourceMethod {
    pub package_name: Option<PackageName>,
    pub interface_name: Option<InterfaceName>,
    pub resource_name: String,
    pub method_name: String,
}

impl FullyQualifiedResourceMethod {
    // We rely on the fully parsed function name itself to retrieve the original function name
    pub fn dynamic_parsed_function_name(
        &self,
        resource_args: Vec<Expr>,
    ) -> Result<DynamicParsedFunctionName, String> {
        let mut dynamic_parsed_str = String::new();

        // Construct the package/interface prefix
        if let Some(package) = &self.package_name {
            dynamic_parsed_str.push_str(&package.to_string());
            dynamic_parsed_str.push('/');
        }

        if let Some(interface) = &self.interface_name {
            dynamic_parsed_str.push_str(&interface.to_string());
            dynamic_parsed_str.push('.');
        }

        // Start the dynamic function name with resource
        dynamic_parsed_str.push('{');
        dynamic_parsed_str.push_str(&self.resource_name);

        // If arguments exist, format them inside parentheses
        if !resource_args.is_empty() {
            dynamic_parsed_str.push('(');
            dynamic_parsed_str.push_str(
                &resource_args
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            dynamic_parsed_str.push(')');
        }

        // Append the method name
        dynamic_parsed_str.push('.');
        dynamic_parsed_str.push_str(&self.method_name);
        dynamic_parsed_str.push('}');

        DynamicParsedFunctionName::parse(dynamic_parsed_str)
    }

    pub fn method_name(&self) -> &String {
        &self.method_name
    }
}

impl Display for FullyQualifiedFunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(package_name) = &self.package_name {
            write!(f, "{}", package_name)?
        }

        if let Some(interface_name) = &self.interface_name {
            write!(f, "/{}.", interface_name)?;
            write!(f, "{{{}}}", self.function_name)
        } else {
            write!(f, "{}", self.function_name)
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct FunctionType {
    pub parameter_types: Vec<InferredType>,
    pub return_type: Vec<InferredType>,
}

impl FunctionType {
    pub fn parameter_types(&self) -> Vec<InferredType> {
        self.parameter_types.clone()
    }

    pub fn return_type(&self) -> Vec<InferredType> {
        self.return_type.clone()
    }
}

fn search_function_in_instance(
    instance: &InstanceType,
    function_name: &str,
) -> Result<Function, String> {
    let functions: Vec<(FunctionName, FunctionType)> = instance
        .function_dict()
        .name_and_types
        .into_iter()
        .filter(|(f, _)| f.name() == *function_name)
        .collect();

    if functions.is_empty() {
        return Err(format!("function '{}' not found", function_name));
    }

    let mut package_map: HashMap<Option<PackageName>, HashSet<Option<InterfaceName>>> =
        HashMap::new();

    for (fqfn, _) in &functions {
        package_map
            .entry(fqfn.package_name())
            .or_default()
            .insert(fqfn.interface_name());
    }

    match package_map.len() {
        1 => {
            let interfaces = package_map.values().flatten().cloned().collect();
            search_function_in_single_package(interfaces, functions, function_name)
        }
        _ => search_function_in_multiple_packages(function_name, package_map),
    }
}

fn search_function_in_single_package(
    interfaces: HashSet<Option<InterfaceName>>,
    functions: Vec<(FunctionName, FunctionType)>,
    function_name: &str,
) -> Result<Function, String> {
    if interfaces.len() == 1 {
        let (fqfn, ftype) = &functions[0];
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
            "multiple interfaces contain function '{}'. specify an interface name as type parameter from: {}",
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
        "function '{}' exists in multiple packages. specify a package name as type parameter from: ",
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

impl TryFrom<ProtoFunctionType> for FunctionType {
    type Error = String;

    fn try_from(proto: ProtoFunctionType) -> Result<Self, Self::Error> {
        let mut parameter_types = Vec::new();
        for param in proto.parameter_types {
            parameter_types.push(InferredType::from(&AnalysedType::try_from(&param)?));
        }

        let mut return_type = Vec::new();
        for ret in proto.return_type {
            return_type.push(InferredType::from(&AnalysedType::try_from(&ret)?));
        }

        Ok(Self {
            parameter_types,
            return_type,
        })
    }
}

impl TryFrom<ProtoResourceMethodDictionary> for ResourceMethodDictionary {
    type Error = String;

    fn try_from(proto: ProtoResourceMethodDictionary) -> Result<Self, Self::Error> {
        let mut map = Vec::new();
        for resource_method_entry in proto.map {
            let resource_method = resource_method_entry
                .key
                .ok_or("resource method not found")?;
            let function_type = resource_method_entry
                .value
                .ok_or("function type not found")?;
            let resource_method = FullyQualifiedResourceMethod::try_from(resource_method)?;
            let function_type = FunctionType::try_from(function_type)?;
            map.push((resource_method, function_type));
        }
        Ok(ResourceMethodDictionary { map })
    }
}

impl TryFrom<ProtoFunctionDictionary> for FunctionDictionary {
    type Error = String;

    fn try_from(value: ProtoFunctionDictionary) -> Result<Self, Self::Error> {
        let mut map = Vec::new();

        for function_entry in value.map {
            let function_name = function_entry.key.ok_or("Function name not found")?;
            let function_type = function_entry.value.ok_or("Function type not found")?;

            let function_name = FunctionName::try_from(function_name)?;
            let function_type = FunctionType::try_from(function_type)?;
            map.push((function_name, function_type));
        }

        Ok(FunctionDictionary {
            name_and_types: map,
        })
    }
}

impl TryFrom<ProtoPackageName> for PackageName {
    type Error = String;

    fn try_from(proto: ProtoPackageName) -> Result<Self, Self::Error> {
        Ok(PackageName {
            namespace: proto.namespace,
            package_name: proto.package_name,
            version: proto.version,
        })
    }
}

impl TryFrom<ProtoInterfaceName> for InterfaceName {
    type Error = String;

    fn try_from(value: ProtoInterfaceName) -> Result<Self, Self::Error> {
        Ok(InterfaceName {
            name: value.name,
            version: value.version,
        })
    }
}

impl TryFrom<ProtoFullyQualifiedFunctionName> for FullyQualifiedFunctionName {
    type Error = String;

    fn try_from(proto: ProtoFullyQualifiedFunctionName) -> Result<Self, Self::Error> {
        Ok(FullyQualifiedFunctionName {
            package_name: proto.package_name.map(TryFrom::try_from).transpose()?,
            interface_name: proto.interface_name.map(TryFrom::try_from).transpose()?,
            function_name: proto.function_name,
        })
    }
}

impl TryFrom<ProtoFullyQualifiedResourceMethod> for FullyQualifiedResourceMethod {
    type Error = String;

    fn try_from(proto: ProtoFullyQualifiedResourceMethod) -> Result<Self, Self::Error> {
        Ok(FullyQualifiedResourceMethod {
            resource_name: proto.resource_name,
            method_name: proto.method_name,
            package_name: proto.package_name.map(TryFrom::try_from).transpose()?,
            interface_name: proto.interface_name.map(TryFrom::try_from).transpose()?,
        })
    }
}

impl TryFrom<ProtoFullyQualifiedResourceConstructor> for FullyQualifiedResourceConstructor {
    type Error = String;

    fn try_from(proto: ProtoFullyQualifiedResourceConstructor) -> Result<Self, Self::Error> {
        Ok(FullyQualifiedResourceConstructor {
            package_name: proto.package_name.map(TryFrom::try_from).transpose()?,
            interface_name: proto.interface_name.map(TryFrom::try_from).transpose()?,
            resource_name: proto.resource_name,
        })
    }
}

impl TryFrom<ProtoFunctionName> for FunctionName {
    type Error = String;

    fn try_from(proto: ProtoFunctionName) -> Result<Self, Self::Error> {
        let proto_function_name = proto.function_name.ok_or("Function name not found")?;
        match proto_function_name {
            function_name_type::FunctionName::Function(fqfn) => {
                Ok(FunctionName::Function(TryFrom::try_from(fqfn)?))
            }
            function_name_type::FunctionName::ResourceConstructor(fqfn) => {
                Ok(FunctionName::ResourceConstructor(TryFrom::try_from(fqfn)?))
            }
            function_name_type::FunctionName::ResourceMethod(fqfn) => {
                Ok(FunctionName::ResourceMethod(TryFrom::try_from(fqfn)?))
            }
        }
    }
}

impl TryFrom<ProtoInstanceType> for InstanceType {
    type Error = String;

    fn try_from(value: ProtoInstanceType) -> Result<Self, Self::Error> {
        let instance = value.instance.ok_or("Instance not found")?;

        match instance {
            Instance::Global(global_instance) => {
                let functions_global = global_instance
                    .functions_global
                    .ok_or("Functions global not found")?;

                Ok(InstanceType::Global {
                    worker_name: global_instance
                        .worker_name
                        .map(Expr::try_from)
                        .transpose()?
                        .map(Box::new),
                    functions_global: TryFrom::try_from(functions_global)?,
                })
            }
            Instance::Package(package_instance) => {
                let package_name = package_instance
                    .package_name
                    .ok_or("Package name not found")?;
                let functions_in_package = package_instance
                    .functions_in_package
                    .ok_or("Functions in package not found")?;

                Ok(InstanceType::Package {
                    worker_name: package_instance
                        .worker_name
                        .map(Expr::try_from)
                        .transpose()?
                        .map(Box::new),
                    package_name: TryFrom::try_from(package_name)?,
                    functions_in_package: TryFrom::try_from(functions_in_package)?,
                })
            }
            Instance::Interface(interface_instance) => {
                let interface_name = interface_instance
                    .interface_name
                    .ok_or("Interface name not found")?;
                let functions_in_interface = interface_instance
                    .functions_in_interface
                    .ok_or("Functions in interface not found")?;

                Ok(InstanceType::Interface {
                    worker_name: interface_instance
                        .worker_name
                        .map(Expr::try_from)
                        .transpose()?
                        .map(Box::new),
                    interface_name: TryFrom::try_from(interface_name)?,
                    functions_in_interface: TryFrom::try_from(functions_in_interface)?,
                })
            }
            Instance::PackageInterface(package_interface_instance) => {
                let functions_in_package_interface = package_interface_instance
                    .functions_in_package_interface
                    .ok_or("Functions in package interface not found")?;

                let interface_name = package_interface_instance
                    .interface_name
                    .ok_or("Interface name not found")?;
                let package_name = package_interface_instance
                    .package_name
                    .ok_or("Package name not found")?;

                Ok(InstanceType::PackageInterface {
                    worker_name: package_interface_instance
                        .worker_name
                        .map(Expr::try_from)
                        .transpose()?
                        .map(Box::new),
                    package_name: TryFrom::try_from(package_name)?,
                    interface_name: TryFrom::try_from(interface_name)?,
                    functions_in_package_interface: TryFrom::try_from(
                        functions_in_package_interface,
                    )?,
                })
            }
            Instance::Resource(resource_instance) => {
                let resource_method_dict = resource_instance
                    .resource_method_dict
                    .ok_or("Resource method dictionary not found")?;
                Ok(InstanceType::Resource {
                    worker_name: resource_instance
                        .worker_name
                        .map(Expr::try_from)
                        .transpose()?
                        .map(Box::new),
                    package_name: resource_instance
                        .package_name
                        .map(TryFrom::try_from)
                        .transpose()?,
                    interface_name: resource_instance
                        .interface_name
                        .map(TryFrom::try_from)
                        .transpose()?,
                    resource_constructor: resource_instance.resource_constructor,
                    resource_args: resource_instance
                        .resource_args
                        .into_iter()
                        .map(TryFrom::try_from)
                        .collect::<Result<Vec<Expr>, String>>()?,
                    resource_method_dict: TryFrom::try_from(resource_method_dict)?,
                })
            }
        }
    }
}

impl From<PackageName> for ProtoPackageName {
    fn from(value: PackageName) -> Self {
        ProtoPackageName {
            namespace: value.namespace,
            package_name: value.package_name,
            version: value.version,
        }
    }
}

impl From<InterfaceName> for ProtoInterfaceName {
    fn from(value: InterfaceName) -> Self {
        ProtoInterfaceName {
            name: value.name,
            version: value.version,
        }
    }
}

impl From<FullyQualifiedResourceConstructor> for ProtoFullyQualifiedResourceConstructor {
    fn from(value: FullyQualifiedResourceConstructor) -> Self {
        ProtoFullyQualifiedResourceConstructor {
            package_name: value.package_name.map(ProtoPackageName::from),
            interface_name: value.interface_name.map(ProtoInterfaceName::from),
            resource_name: value.resource_name,
        }
    }
}
