// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::parser::{PackageName, TypeParameter};
use crate::type_parameter::InterfaceName;
use crate::FunctionName;
use crate::{
    ComponentDependencies, ComponentDependencyKey, Expr, FullyQualifiedResourceConstructor,
    FunctionDictionary, FunctionType, InferredType, ResourceMethodDictionary,
};
use golem_api_grpc::proto::golem::rib::{
    FullyQualifiedResourceConstructor as ProtoFullyQualifiedResourceConstructor,
    FunctionType as ProtoFunctionType, InterfaceName as ProtoInterfaceName,
    PackageName as ProtoPackageName,
};
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt::Debug;
use std::ops::Deref;

// `InstanceType` will be the type (`InferredType`) of the variable associated with creation of an instance
// `InstanceType` is structured to help with compilation logic better. Example: a random `instance()` call
// is of type `Global` to begin with and as soon as method invocations becomes a real function call,
// the type of instance becomes more and more precise.
//
// Please look at `InstanceCreationType`
// for a tangible view on the fact that an instance can be either worker or a resource.
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub enum InstanceType {
    // Holds functions across every package and interface in every component
    Global {
        worker_name: Option<Box<Expr>>,
        component_dependency: ComponentDependencies,
    },

    // A component can refer to a Package x, which can exist in other components
    Package {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        component_dependency: ComponentDependencies,
    },

    // Holds all functions across (may be across packages or components) for a specific interface
    Interface {
        worker_name: Option<Box<Expr>>,
        interface_name: InterfaceName,
        component_dependency: ComponentDependencies,
    },

    // Most granular level, holds functions for a specific package and interface
    // That said, this package and interface may exist in multiple components
    PackageInterface {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        interface_name: InterfaceName,
        component_dependency: ComponentDependencies,
    },

    // Holds the resource creation and the functions in the resource
    // that may or may not be addressed
    Resource {
        analysed_resource_id: u64,
        analysed_resource_mode: u8,
        worker_name: Option<Box<Expr>>,
        package_name: Option<PackageName>,
        interface_name: Option<InterfaceName>,
        resource_constructor: String,
        resource_args: Vec<Expr>,
        component_dependency_key: ComponentDependencyKey,
        resource_method_dictionary: ResourceMethodDictionary,
    },
}

impl InstanceType {
    pub fn narrow_to_single_component(
        &mut self,
        component_dependency_key: &ComponentDependencyKey,
    ) {
        match self {
            InstanceType::Global {
                component_dependency,
                ..
            } => {
                component_dependency.narrow_to_component(component_dependency_key);
            }
            InstanceType::Package {
                component_dependency,
                ..
            } => {
                component_dependency.narrow_to_component(component_dependency_key);
            }
            InstanceType::Interface {
                component_dependency,
                ..
            } => {
                component_dependency.narrow_to_component(component_dependency_key);
            }
            InstanceType::PackageInterface {
                component_dependency,
                ..
            } => {
                component_dependency.narrow_to_component(component_dependency_key);
            }
            // A resource is already narrowed down to a component
            InstanceType::Resource { .. } => {}
        }
    }

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

    pub fn get_resource_instance_type(
        &self,
        fully_qualified_resource_constructor: FullyQualifiedResourceConstructor,
        resource_args: Vec<Expr>,
        worker_name: Option<Box<Expr>>,
        analysed_resource_id: u64,
        analysed_resource_mode: u8,
    ) -> Result<InstanceType, String> {
        let interface_name = fully_qualified_resource_constructor.interface_name.clone();
        let package_name = fully_qualified_resource_constructor.package_name.clone();
        let resource_constructor_name = fully_qualified_resource_constructor.resource_name.clone();

        let mut tree = vec![];
        for (component_info, function_type) in self.component_dependencies().dependencies.iter() {
            let mut resource_method_dict = BTreeMap::new();

            for (name, typ) in function_type.name_and_types.iter() {
                if let FunctionName::ResourceMethod(resource_method) = name {
                    if resource_method.resource_name == resource_constructor_name
                        && resource_method.interface_name == interface_name
                        && resource_method.package_name == package_name
                    {
                        resource_method_dict.insert(resource_method.clone(), typ.clone());
                    }
                }
            }

            tree.push((component_info.clone(), resource_method_dict));
        }

        if tree.len() == 1 {
            let (component_dependency_key, resource_methods) = tree.pop().unwrap();

            let resource_method_dict = ResourceMethodDictionary {
                map: resource_methods,
            };

            Ok(InstanceType::Resource {
                worker_name,
                package_name,
                interface_name,
                resource_constructor: resource_constructor_name,
                resource_args,
                component_dependency_key,
                resource_method_dictionary: resource_method_dict,
                analysed_resource_id,
                analysed_resource_mode,
            })
        } else if tree.is_empty() {
            Err(format!(
                "No components found have the resource constructor '{resource_constructor_name}'"
            ))
        } else {
            Err(format!(
                "Multiple components found with the resource constructor '{resource_constructor_name}'. Please specify the type parameter"
            ))
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
    ) -> Result<(ComponentDependencyKey, Function), String> {
        match type_parameter {
            Some(tp) => match tp {
                TypeParameter::Interface(iface) => {
                    let component_dependency =
                        self.component_dependencies().filter_by_interface(&iface)?;

                    if component_dependency.size() == 1 {
                        let (info, function_dictionary) =
                            component_dependency.dependencies.first_key_value().unwrap();

                        let functions = function_dictionary
                            .name_and_types
                            .iter()
                            .filter(|(f, _)| f.name() == method_name)
                            .collect::<Vec<_>>();

                        if functions.is_empty() {
                            return Err(format!(
                                "Function '{method_name}' not found in interface '{iface}'"
                            ));
                        }

                        if functions.len() == 1 {
                            let (fqfn, ftype) = &functions[0];
                            Ok((
                                info.clone(),
                                Function {
                                    function_name: fqfn.clone(),
                                    function_type: ftype.clone(),
                                },
                            ))
                        } else {
                            search_function_in_instance(self, method_name, Some(info))
                        }
                    } else {
                        Err(format!("Interface '{iface}' found in multiple components"))
                    }
                }

                TypeParameter::PackageName(pkg) => {
                    let component_dependency =
                        self.component_dependencies().filter_by_package_name(&pkg)?;

                    if component_dependency.size() == 1 {
                        let (info, function_dictionary) =
                            component_dependency.dependencies.first_key_value().unwrap();

                        let packages = function_dictionary
                            .name_and_types
                            .iter()
                            .filter(|(f, _)| f.package_name() == Some(pkg.clone()))
                            .collect::<Vec<_>>();

                        if packages.is_empty() {
                            return Err(format!("package '{pkg}' not found"));
                        }

                        let functions = packages
                            .into_iter()
                            .filter(|(f, _)| f.name() == method_name)
                            .collect::<Vec<_>>();

                        if functions.is_empty() {
                            return Err(format!(
                                "function '{method_name}' not found in package '{pkg}'"
                            ));
                        }

                        if functions.len() == 1 {
                            let (fqfn, ftype) = &functions[0];
                            Ok((
                                info.clone(),
                                Function {
                                    function_name: fqfn.clone(),
                                    function_type: ftype.clone(),
                                },
                            ))
                        } else {
                            search_function_in_instance(self, method_name, Some(info))
                        }
                    } else {
                        Err(format!(
                            "package '{pkg}' found in multiple components. Please specify the root package name instead"
                        ))
                    }
                }

                TypeParameter::FullyQualifiedInterface(fq_iface) => {
                    let component_dependency = self
                        .component_dependencies()
                        .filter_by_fully_qualified_interface(&fq_iface)?;

                    if component_dependency.size() == 1 {
                        let (info, function_dictionary) =
                            component_dependency.dependencies.first_key_value().unwrap();

                        let functions = function_dictionary
                            .name_and_types
                            .iter()
                            .filter(|(f, _)| {
                                f.package_name() == Some(fq_iface.package_name.clone())
                                    && f.interface_name() == Some(fq_iface.interface_name.clone())
                                    && f.name() == method_name
                            })
                            .collect::<Vec<_>>();

                        if functions.is_empty() {
                            return Err(format!(
                                "function '{method_name}' not found in interface '{fq_iface}'"
                            ));
                        }

                        if functions.len() == 1 {
                            let (fqfn, ftype) = &functions[0];
                            Ok((
                                info.clone(),
                                Function {
                                    function_name: fqfn.clone(),
                                    function_type: ftype.clone(),
                                },
                            ))
                        } else {
                            search_function_in_instance(self, method_name, Some(info))
                        }
                    } else {
                        Err(format!(
                            "interface '{fq_iface}' found in multiple components. Please specify the root package name instead"
                        ))
                    }
                }
            },
            None => search_function_in_instance(self, method_name, None),
        }
    }

    // A flattened list of all resource methods
    pub fn resource_method_dictionary(&self) -> FunctionDictionary {
        let name_and_types = self
            .component_dependencies()
            .dependencies
            .values()
            .flat_map(|function_dictionary| {
                function_dictionary
                    .name_and_types
                    .iter()
                    .filter(|(f, _)| matches!(f, FunctionName::ResourceMethod(_)))
                    .map(|(f, t)| (f.clone(), t.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();

        FunctionDictionary { name_and_types }
    }

    pub fn function_dict_without_resource_methods(&self) -> FunctionDictionary {
        let name_and_types = self
            .component_dependencies()
            .dependencies
            .values()
            .flat_map(|function_dictionary| {
                function_dictionary
                    .name_and_types
                    .iter()
                    .filter(|(f, _)| {
                        !matches!(f, FunctionName::ResourceMethod(_))
                            && !matches!(f, FunctionName::Variant(_))
                            && !matches!(f, FunctionName::Enum(_))
                    })
                    .map(|(f, t)| (f.clone(), t.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();

        FunctionDictionary { name_and_types }
    }

    pub fn component_dependencies(&self) -> ComponentDependencies {
        match self {
            InstanceType::Global {
                component_dependency,
                ..
            } => component_dependency.clone(),
            InstanceType::Package {
                component_dependency,
                ..
            } => component_dependency.clone(),
            InstanceType::Interface {
                component_dependency,
                ..
            } => component_dependency.clone(),
            InstanceType::PackageInterface {
                component_dependency,
                ..
            } => component_dependency.clone(),
            InstanceType::Resource {
                resource_method_dictionary,
                component_dependency_key,
                ..
            } => {
                let function_dictionary = FunctionDictionary::from(resource_method_dictionary);
                let mut dependencies = BTreeMap::new();
                dependencies.insert(component_dependency_key.clone(), function_dictionary);

                ComponentDependencies { dependencies }
            }
        }
    }

    pub fn from(
        dependency: &ComponentDependencies,
        worker_name: Option<&Expr>,
        type_parameter: Option<TypeParameter>,
    ) -> Result<InstanceType, String> {
        match type_parameter {
            None => Ok(InstanceType::Global {
                worker_name: worker_name.cloned().map(Box::new),
                component_dependency: dependency.clone(),
            }),
            Some(type_parameter) => match type_parameter {
                TypeParameter::Interface(interface_name) => {
                    let new_dependency = dependency.filter_by_interface(&interface_name)?;

                    Ok(InstanceType::Interface {
                        worker_name: worker_name.cloned().map(Box::new),
                        interface_name,
                        component_dependency: new_dependency,
                    })
                }
                TypeParameter::PackageName(package_name) => {
                    let new_dependency = dependency.filter_by_package_name(&package_name)?;

                    Ok(InstanceType::Package {
                        worker_name: worker_name.cloned().map(Box::new),
                        package_name,
                        component_dependency: new_dependency,
                    })
                }
                TypeParameter::FullyQualifiedInterface(fqi) => {
                    let component_dependency =
                        dependency.filter_by_fully_qualified_interface(&fqi)?;

                    Ok(InstanceType::PackageInterface {
                        worker_name: worker_name.cloned().map(Box::new),
                        package_name: fqi.package_name,
                        interface_name: fqi.interface_name,
                        component_dependency,
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

fn search_function_in_instance(
    instance: &InstanceType,
    function_name: &str,
    component_info: Option<&ComponentDependencyKey>,
) -> Result<(ComponentDependencyKey, Function), String> {
    match component_info {
        Some(component_info) => {
            let dependencies = instance.component_dependencies();

            let function_dictionary =
                dependencies
                    .dependencies
                    .get(component_info)
                    .ok_or(format!(
                        "component '{component_info}' not found in dependencies"
                    ))?;

            let functions = function_dictionary
                .name_and_types
                .iter()
                .filter(|(f, _)| f.name() == function_name)
                .collect::<Vec<_>>();

            if functions.is_empty() {
                return Err(format!(
                    "function '{function_name}' not found in component '{component_info}'"
                ));
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
                    let function =
                        search_function_in_single_package(interfaces, functions, function_name)?;

                    Ok((component_info.clone(), function))
                }
                _ => {
                    let function =
                        search_function_in_multiple_packages(function_name, package_map)?;
                    Ok((component_info.clone(), function))
                }
            }
        }
        None => {
            let mut component_info_functions = vec![];

            for (info, function_dictionary) in instance.component_dependencies().dependencies.iter()
            {
                let functions = function_dictionary
                    .name_and_types
                    .iter()
                    .filter(|(f, _)| f.name() == function_name)
                    .collect::<Vec<_>>();

                if functions.is_empty() {
                    continue;
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
                        let function = search_function_in_single_package(
                            interfaces,
                            functions,
                            function_name,
                        )?;

                        component_info_functions.push((info.clone(), function));
                    }
                    _ => {
                        let function =
                            search_function_in_multiple_packages(function_name, package_map)?;

                        component_info_functions.push((info.clone(), function));
                    }
                }
            }

            if component_info_functions.len() == 1 {
                let (info, function) = &component_info_functions[0];
                Ok((info.clone(), function.clone()))
            } else if component_info_functions.is_empty() {
                Err(format!("function '{function_name}' not found"))
            } else {
                Err(format!(
                    "function '{function_name}' found in multiple components. Please specify the type parameter"
                ))
            }
        }
    }
}

fn search_function_in_single_package(
    interfaces: HashSet<Option<InterfaceName>>,
    functions: Vec<&(FunctionName, FunctionType)>,
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
        "function '{function_name}' exists in multiple packages. specify a package name as type parameter from: "
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
                    format!("{p}")
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

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::{
        FullyQualifiedFunctionName, FullyQualifiedResourceConstructor,
        FullyQualifiedResourceMethod, InterfaceName, PackageName,
    };
    use golem_api_grpc::proto::golem::rib::FullyQualifiedFunctionName as ProtoFullyQualifiedFunctionName;
    use golem_api_grpc::proto::golem::rib::FullyQualifiedResourceConstructor as ProtoFullyQualifiedResourceConstructor;
    use golem_api_grpc::proto::golem::rib::FullyQualifiedResourceMethod as ProtoFullyQualifiedResourceMethod;
    use golem_api_grpc::proto::golem::rib::InterfaceName as ProtoInterfaceName;
    use golem_api_grpc::proto::golem::rib::PackageName as ProtoPackageName;

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

    impl From<FullyQualifiedFunctionName> for ProtoFullyQualifiedFunctionName {
        fn from(value: FullyQualifiedFunctionName) -> Self {
            ProtoFullyQualifiedFunctionName {
                package_name: value.package_name.map(ProtoPackageName::from),
                interface_name: value.interface_name.map(ProtoInterfaceName::from),
                function_name: value.function_name,
            }
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
                static_function: false, //FIXME
            })
        }
    }

    impl From<FullyQualifiedResourceMethod> for ProtoFullyQualifiedResourceMethod {
        fn from(value: FullyQualifiedResourceMethod) -> Self {
            ProtoFullyQualifiedResourceMethod {
                resource_name: value.resource_name,
                method_name: value.method_name,
                package_name: value.package_name.map(ProtoPackageName::from),
                interface_name: value.interface_name.map(ProtoInterfaceName::from),
            }
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
}

impl TryFrom<ProtoFunctionType> for FunctionType {
    type Error = String;

    fn try_from(proto: ProtoFunctionType) -> Result<Self, Self::Error> {
        let mut parameter_types = Vec::new();
        for param in proto.parameter_types {
            parameter_types.push(InferredType::from(&AnalysedType::try_from(&param)?));
        }

        let return_type = proto
            .return_type
            .as_ref()
            .map(|ret| AnalysedType::try_from(ret).map(|ret| InferredType::from(&ret)))
            .transpose()?;

        Ok(Self {
            parameter_types,
            return_type,
        })
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
