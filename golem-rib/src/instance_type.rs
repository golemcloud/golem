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
use crate::{
    CallType, ComponentDependency, ComponentInfo, DynamicParsedFunctionName,
    DynamicParsedFunctionReference, Expr, FunctionTypeRegistry, InferredType, ParsedFunctionSite,
    RegistryKey, RegistryValue,
};
use golem_api_grpc::proto::golem::rib::{
    FullyQualifiedResourceConstructor as ProtoFullyQualifiedResourceConstructor,
    FunctionType as ProtoFunctionType,
    InterfaceName as ProtoInterfaceName, PackageName as ProtoPackageName,
};
use golem_wasm_ast::analysis::{AnalysedType, TypeEnum, TypeVariant};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    // Holds functions across every package and interface in every components
    Global {
        worker_name: Option<Box<Expr>>,
        component_dependency: ComponentDependency,
    },

    // A component can refer to a Package x, which can exist in other components
    Package {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        component_dependency: ComponentDependency,
    },

    // Holds all functions across (may be across packages or components) for a specific interface
    Interface {
        worker_name: Option<Box<Expr>>,
        interface_name: InterfaceName,
        component_dependency: ComponentDependency,
    },

    // Most granular level, holds functions for a specific package and interface
    // That said, this package and interface may exist in multiple components
    PackageInterface {
        worker_name: Option<Box<Expr>>,
        package_name: PackageName,
        interface_name: InterfaceName,
        component_dependency: ComponentDependency,
    },

    // Holds the resource creation and the functions in the resource
    // that may or may not be addressed
    Resource {
        worker_name: Option<Box<Expr>>,
        package_name: Option<PackageName>,
        interface_name: Option<InterfaceName>,
        resource_constructor: String,
        resource_args: Vec<Expr>,
        component_dependency: ResourceMethodDictionary,
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

        let mut tree = BTreeMap::new();
        for (component_info, function_type) in self.component_dependency().dependencies.iter() {
            let mut resource_method_dict = vec![];

            for (name, typ) in function_type.name_and_types.iter() {
                if let FunctionName::ResourceMethod(resource_method) = name {
                    if resource_method.resource_name == resource_constructor_name
                        && resource_method.interface_name == interface_name
                        && resource_method.package_name == package_name
                    {
                        resource_method_dict.push((resource_method.clone(), typ.clone()));
                    }
                }
            }

            tree.insert(
                component_info.clone(),
                resource_method_dict,
            );
        }

        let resource_method_dict = ResourceMethodDictionary {
            map: tree,
        };

        InstanceType::Resource {
            worker_name,
            package_name,
            interface_name,
            resource_constructor: resource_constructor_name,
            resource_args,
            component_dependency: resource_method_dict,
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
    ) -> Result<(ComponentInfo, Function), String> {
        match type_parameter {
            Some(tp) => match tp {
                TypeParameter::Interface(iface) => {
                    let component_dependency =
                        self.component_dependency().filter_by_interface(&iface)?;

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
                                "Function '{}' not found in interface '{}'",
                                method_name, iface
                            ));
                        }

                        if functions.len() == 1 {
                            let (fqfn, ftype) = &functions[0];
                            Ok((info.clone(), Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            }))
                        } else {
                            search_function_in_instance(self, method_name, Some(info))
                        }
                    } else {
                        Err(format!(
                            "Interface '{}' found in multiple components",
                            iface
                        ))
                    }
                }

                TypeParameter::PackageName(pkg) => {
                    let component_dependency =
                        self.component_dependency().filter_by_package_name(&pkg)?;

                    if component_dependency.size() == 1 {
                        let (info, function_dictionary) =
                            component_dependency.dependencies.first_key_value().unwrap();

                        let packages = function_dictionary
                            .name_and_types
                            .iter()
                            .filter(|(f, _)| f.package_name() == Some(pkg.clone()))
                            .collect::<Vec<_>>();

                        if packages.is_empty() {
                            return Err(format!("package '{}' not found", pkg));
                        }

                        let functions = packages
                            .into_iter()
                            .filter(|(f, _)| f.name() == method_name)
                            .collect::<Vec<_>>();

                        if functions.len() == 1 {
                            let (fqfn, ftype) = &functions[0];
                            Ok((info.clone(), Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            }))
                        } else {
                            search_function_in_instance(self, method_name, Some(info))
                        }
                    } else {
                        Err(format!(
                            "package '{}' found in multiple components. Please specify the root package name instead",
                            pkg
                        ))
                    }
                }

                TypeParameter::FullyQualifiedInterface(fq_iface) => {
                    let component_dependency =
                        self.component_dependency().filter_by_fully_qualified_interface(&fq_iface)?;

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
                                "function '{}' not found in interface '{}'",
                                method_name, fq_iface
                            ));
                        }

                        if functions.len() == 1 {
                            let (fqfn, ftype) = &functions[0];
                            Ok((info.clone(), Function {
                                function_name: fqfn.clone(),
                                function_type: ftype.clone(),
                            }))
                        } else {
                            search_function_in_instance(self, method_name, Some(info))
                        }
                    } else {
                        Err(format!(
                            "interface '{}' found in multiple components. Please specify the root package name instead",
                            fq_iface
                        ))
                    }
                }
            },
            None => search_function_in_instance(self, method_name, None),
        }
    }

    // A flattened list of all resource methods
    pub fn resource_method_dictionary(&self) -> FunctionDictionary {
        let name_and_types = self.component_dependency()
            .dependencies
            .values().flat_map(
                |function_dictionary| {
                    function_dictionary
                        .name_and_types
                        .iter()
                        .filter(|(f, _)| matches!(f, FunctionName::ResourceMethod(_)))
                        .map(|(f, t)| (f.clone(), t.clone()))
                        .collect::<Vec<_>>()
                }
            ).collect();

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

    pub fn component_dependency(&self) -> &ComponentDependency {
        match self {
            InstanceType::Global {
                component_dependency,
                ..
            } => component_dependency,
            InstanceType::Package {
                component_dependency,
                ..
            } => component_dependency,
            InstanceType::Interface {
                component_dependency,
                ..
            } => component_dependency,
            InstanceType::PackageInterface {
                component_dependency,
                ..
            } => component_dependency,
            InstanceType::Resource {
                component_dependency,
                ..
            } => panic!("resource method dictionary"), //resource_method_dict.into(),
        }
    }

    pub fn from(
        dependency: &ComponentDependency,
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

// Global Function Dictionary across Components,
#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd, Default)]
pub struct FunctionDictionary {
    pub name_and_types: Vec<(FunctionName, FunctionType)>,
}

impl FunctionDictionary {
    pub fn get_all_variants(&self) -> Vec<TypeVariant> {
        self.name_and_types
            .iter()
            .filter_map(|(_, ftype)| ftype.as_type_variant())
            .collect()
    }

    pub fn get_all_enums(&self) -> Vec<TypeEnum> {
        self.name_and_types
            .iter()
            .filter_map(|(_, ftype)| ftype.as_type_enum())
            .collect()
    }

    pub fn get_enum_info(&self, identifier_name: &str) -> Option<TypeEnum> {
        self.name_and_types.iter().find_map(|(f, ftype)| match f {
            FunctionName::Enum(name) => {
                if name == identifier_name {
                    ftype.as_type_enum()
                } else {
                    None
                }
            }
            _ => None,
        })
    }
    pub fn get_variant_info(&self, identifier_name: &str) -> Option<TypeVariant> {
        self.name_and_types.iter().find_map(|(f, ftype)| match f {
            FunctionName::Variant(name) => {
                if name == identifier_name {
                    ftype.as_type_variant()
                } else {
                    None
                }
            }
            _ => None,
        })
    }

    pub fn function_names(&self) -> Vec<String> {
        self.name_and_types
            .iter()
            .map(|(f, _)| f.name())
            .collect::<Vec<_>>()
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ResourceMethodDictionary {
    pub map: BTreeMap<ComponentInfo, Vec<(FullyQualifiedResourceMethod, FunctionType)>>,
}

impl From<&ResourceMethodDictionary> for ComponentDependency {
    fn from(value: &ResourceMethodDictionary) -> Self {

        let mut dict = BTreeMap::new();

        for (info, function_dictionary) in value.map {
            let function_dictionary =  FunctionDictionary {
                name_and_types: function_dictionary
                    .iter()
                    .map(|(k, v)| (FunctionName::ResourceMethod(k.clone()), v.clone()))
                    .collect(),
            };

            dict.insert(info, function_dictionary);
        }

        ComponentDependency {
            dependencies: dict,
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
                    return_type,
                } => match key {
                    RegistryKey::FunctionName(function_name) => {
                        let function_name = resolve_function_name(None, None, function_name)?;

                        map.push((
                            function_name,
                            FunctionType {
                                parameter_types: parameter_types.iter().map(|x| x.into()).collect(),
                                return_type: return_type.as_ref().map(|x| x.into()),
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
                                return_type: return_type.as_ref().map(|x| x.into()),
                            },
                        ));
                    }
                },

                RegistryValue::Variant {
                    parameter_types,
                    variant_type,
                } => match key {
                    RegistryKey::FunctionName(name) => {
                        let function_name = FunctionName::Variant(name.to_string());
                        map.push((
                            function_name,
                            FunctionType {
                                parameter_types: parameter_types.iter().map(|x| x.into()).collect(),
                                return_type: Some(InferredType::from(variant_type)),
                            },
                        ));
                    }
                    RegistryKey::FunctionNameWithInterface { .. } => {}
                },

                RegistryValue::Value(value) => match value {
                    AnalysedType::Enum(type_enum) => match key {
                        RegistryKey::FunctionName(name) => {
                            let function_name = FunctionName::Enum(name.to_string());
                            map.push((
                                function_name,
                                FunctionType {
                                    parameter_types: vec![],
                                    return_type: Some(InferredType::enum_(type_enum.cases.clone())),
                                },
                            ));
                        }
                        RegistryKey::FunctionNameWithInterface { .. } => {}
                    },
                    _ => {}
                },
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
    if function_name.trim().starts_with("[constructor]") {
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
    Variant(String),
    Enum(String),
    Function(FullyQualifiedFunctionName),
    ResourceConstructor(FullyQualifiedResourceConstructor),
    ResourceMethod(FullyQualifiedResourceMethod),
}

impl FunctionName {
    pub fn from_dynamic_parsed_function_name(
        function_name: &DynamicParsedFunctionName,
    ) -> FunctionName {
        let site = &function_name.site;
        let (package_name, interface_name) = match site {
            ParsedFunctionSite::Global => (None, None),

            ParsedFunctionSite::Interface { name } => (
                None,
                Some(InterfaceName {
                    name: name.clone(),
                    version: None,
                }),
            ),
            ParsedFunctionSite::PackagedInterface {
                namespace,
                package,
                interface,
                version,
            } => (
                Some(PackageName {
                    namespace: namespace.clone(),
                    package_name: package.clone(),
                    version: None,
                }),
                Some(InterfaceName {
                    name: interface.clone(),
                    version: version.as_ref().map(|v| v.to_string()),
                }),
            ),
        };

        match &function_name.function {
            DynamicParsedFunctionReference::Function { function } => {
                FunctionName::Function(FullyQualifiedFunctionName {
                    package_name,
                    interface_name,
                    function_name: function.clone(),
                })
            }
            DynamicParsedFunctionReference::RawResourceConstructor { resource } => {
                FunctionName::ResourceConstructor(FullyQualifiedResourceConstructor {
                    package_name,
                    interface_name,
                    resource_name: resource.clone(),
                })
            }
            DynamicParsedFunctionReference::RawResourceDrop { resource } => {
                FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                    package_name,
                    interface_name,
                    resource_name: resource.clone(),
                    method_name: "drop".to_string(),
                })
            }
            DynamicParsedFunctionReference::RawResourceMethod { resource, method } => {
                FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                    package_name,
                    interface_name,
                    resource_name: resource.clone(),
                    method_name: method.clone(),
                })
            }
            DynamicParsedFunctionReference::RawResourceStaticMethod { resource, method } => {
                FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                    package_name,
                    interface_name,
                    resource_name: resource.clone(),
                    method_name: method.clone(),
                })
            }
            DynamicParsedFunctionReference::IndexedResourceConstructor { resource, .. } => {
                FunctionName::ResourceConstructor(FullyQualifiedResourceConstructor {
                    package_name,
                    interface_name,
                    resource_name: resource.clone(),
                })
            }
            DynamicParsedFunctionReference::IndexedResourceMethod {
                resource, method, ..
            } => FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                package_name,
                interface_name,
                resource_name: resource.clone(),
                method_name: method.clone(),
            }),
            DynamicParsedFunctionReference::IndexedResourceStaticMethod {
                resource,
                method,
                ..
            } => FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                package_name,
                interface_name,
                resource_name: resource.clone(),
                method_name: method.clone(),
            }),
            DynamicParsedFunctionReference::IndexedResourceDrop { resource, .. } => {
                FunctionName::ResourceMethod(FullyQualifiedResourceMethod {
                    package_name,
                    interface_name,
                    resource_name: resource.clone(),
                    method_name: "drop".to_string(),
                })
            }
        }
    }

    pub fn from_call_type(call_type: &CallType) -> Option<FunctionName> {
        match call_type {
            CallType::VariantConstructor(variant_name) => {
                Some(FunctionName::Variant(variant_name.clone()))
            }
            CallType::EnumConstructor(enum_name) => Some(FunctionName::Enum(enum_name.clone())),
            CallType::Function { function_name, .. } => {
                Some(Self::from_dynamic_parsed_function_name(function_name))
            }
            CallType::InstanceCreation(_) => None,
        }
    }

    pub fn interface_name(&self) -> Option<InterfaceName> {
        match self {
            FunctionName::Function(fqfn) => fqfn.interface_name.clone(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.interface_name.clone(),
            FunctionName::ResourceMethod(resource_method) => resource_method.interface_name.clone(),
            FunctionName::Variant(_) => None,
            FunctionName::Enum(_) => None,
        }
    }

    pub fn package_name(&self) -> Option<PackageName> {
        match self {
            FunctionName::Function(fqfn) => fqfn.package_name.clone(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.package_name.clone(),
            FunctionName::ResourceMethod(fqfn) => fqfn.package_name.clone(),
            FunctionName::Variant(_) => None,
            FunctionName::Enum(_) => None,
        }
    }

    pub fn name(&self) -> String {
        match self {
            FunctionName::Function(fqfn) => fqfn.function_name.to_string(),
            FunctionName::ResourceConstructor(fqfn) => fqfn.resource_name.to_string(),
            FunctionName::ResourceMethod(fqfn) => fqfn.method_name.to_string(),
            FunctionName::Variant(name) => name.clone(),
            FunctionName::Enum(name) => name.clone(),
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
    pub fn get_constructor(&self) -> FullyQualifiedResourceConstructor {
        FullyQualifiedResourceConstructor {
            package_name: self.package_name.clone(),
            interface_name: self.interface_name.clone(),
            resource_name: self.resource_name.clone(),
        }
    }

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
    pub return_type: Option<InferredType>,
}

impl FunctionType {
    pub fn as_type_variant(&self) -> Option<TypeVariant> {
        let analysed_type = AnalysedType::try_from(&self.return_type.clone()?).ok()?;

        match analysed_type {
            AnalysedType::Variant(type_variant) => Some(type_variant),
            _ => None,
        }
    }

    pub fn as_type_enum(&self) -> Option<TypeEnum> {
        let analysed_type = AnalysedType::try_from(&self.return_type.clone()?).ok()?;
        match analysed_type {
            AnalysedType::Enum(type_enum) => Some(type_enum),
            _ => None,
        }
    }

    pub fn parameter_types(&self) -> Vec<InferredType> {
        self.parameter_types.clone()
    }

    pub fn return_type(&self) -> Option<InferredType> {
        self.return_type.clone()
    }
}

fn search_function_in_instance(
    instance: &InstanceType,
    function_name: &str,
    component_info: Option<&ComponentInfo>,
) -> Result<(ComponentInfo, Function), String> {

    match component_info {
        Some(info) => {
           let function_dictionary = instance.component_dependency().dependencies.get(info).ok_or(
                format!("Component info '{}' not found in instance", info)
            )?;

            let functions = function_dictionary
                .name_and_types
                .iter()
                .filter(|(f, _)| f.name() == function_name)
                .collect::<Vec<_>>();

            if functions.is_empty() {
                return Err(format!(
                    "function '{}' not found in component '{}'",
                    function_name, info
                ));
            }

            let mut package_map: HashMap<Option<PackageName>, HashSet<Option<InterfaceName>>> =
                HashMap::new();

            match package_map.len() {
                1 => {
                    let interfaces = package_map.values().flatten().cloned().collect();
                    let function = search_function_in_single_package(interfaces, functions, function_name)?;

                    Ok((info.clone(), function))

                }
                _ => {
                    let function = search_function_in_multiple_packages(function_name, package_map)?;
                    Ok((info.clone(), function))
                },
            }
        }
        None => {
            let mut component_info_functions = vec![];

            for (info, function_dictionary) in instance
                .component_dependency()
                .dependencies
                .iter()
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

                match package_map.len() {
                    1 => {
                        let interfaces = package_map.values().flatten().cloned().collect();
                        let function = search_function_in_single_package(interfaces, functions, function_name)?;

                        component_info_functions.push((info.clone(), function));

                    }
                    _ => {
                        let function = search_function_in_multiple_packages(function_name, package_map)?;
                        component_info_functions.push((info.clone(), function));
                    },
                }
            }

            if component_info_functions.len() == 1 {
                let (info, function) = &component_info_functions[0];
                Ok((info.clone(), function.clone()))
            } else if component_info_functions.is_empty() {
                Err(format!("function '{}' not found", function_name))
            } else {
                Err(format!(
                    "function '{}' found in multiple components. Please specify the type parameter",
                    function_name
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

// impl TryFrom<ProtoResourceMethodDictionary> for ResourceMethodDictionary {
//     type Error = String;
//
//     fn try_from(proto: ProtoResourceMethodDictionary) -> Result<Self, Self::Error> {
//         let mut map = Vec::new();
//         for resource_method_entry in proto.map {
//             let resource_method = resource_method_entry
//                 .key
//                 .ok_or("resource method not found")?;
//             let function_type = resource_method_entry
//                 .value
//                 .ok_or("function type not found")?;
//             let resource_method = FullyQualifiedResourceMethod::try_from(resource_method)?;
//             let function_type = FunctionType::try_from(function_type)?;
//             map.push((resource_method, function_type));
//         }
//         Ok(ResourceMethodDictionary { map })
//     }
// }

// impl TryFrom<ProtoFunctionDictionary> for FunctionDictionary {
//     type Error = String;
//
//     fn try_from(value: ProtoFunctionDictionary) -> Result<Self, Self::Error> {
//         let mut map = Vec::new();
//
//         for function_entry in value.map {
//             let function_name = function_entry.key.ok_or("Function name not found")?;
//             let function_type = function_entry.value.ok_or("Function type not found")?;
//
//             let function_name = FunctionName::try_from(function_name)?;
//             let function_type = FunctionType::try_from(function_type)?;
//             map.push((function_name, function_type));
//         }
//
//         Ok(FunctionDictionary {
//             name_and_types: map,
//         })
//     }
// }

// impl TryFrom<ProtoPackageName> for PackageName {
//     type Error = String;
//
//     fn try_from(proto: ProtoPackageName) -> Result<Self, Self::Error> {
//         Ok(PackageName {
//             namespace: proto.namespace,
//             package_name: proto.package_name,
//             version: proto.version,
//         })
//     }
// }

impl TryFrom<ProtoInterfaceName> for InterfaceName {
    type Error = String;

    fn try_from(value: ProtoInterfaceName) -> Result<Self, Self::Error> {
        Ok(InterfaceName {
            name: value.name,
            version: value.version,
        })
    }
}

// impl TryFrom<ProtoFullyQualifiedFunctionName> for FullyQualifiedFunctionName {
//     type Error = String;
//
//     fn try_from(proto: ProtoFullyQualifiedFunctionName) -> Result<Self, Self::Error> {
//         Ok(FullyQualifiedFunctionName {
//             package_name: proto.package_name.map(TryFrom::try_from).transpose()?,
//             interface_name: proto.interface_name.map(TryFrom::try_from).transpose()?,
//             function_name: proto.function_name,
//         })
//     }
// }

// impl TryFrom<ProtoFullyQualifiedResourceMethod> for FullyQualifiedResourceMethod {
//     type Error = String;
//
//     fn try_from(proto: ProtoFullyQualifiedResourceMethod) -> Result<Self, Self::Error> {
//         Ok(FullyQualifiedResourceMethod {
//             resource_name: proto.resource_name,
//             method_name: proto.method_name,
//             package_name: proto.package_name.map(TryFrom::try_from).transpose()?,
//             interface_name: proto.interface_name.map(TryFrom::try_from).transpose()?,
//         })
//     }
// }

// impl TryFrom<ProtoFullyQualifiedResourceConstructor> for FullyQualifiedResourceConstructor {
//     type Error = String;
//
//     fn try_from(proto: ProtoFullyQualifiedResourceConstructor) -> Result<Self, Self::Error> {
//         Ok(FullyQualifiedResourceConstructor {
//             package_name: proto.package_name.map(TryFrom::try_from).transpose()?,
//             interface_name: proto.interface_name.map(TryFrom::try_from).transpose()?,
//             resource_name: proto.resource_name,
//         })
//     }
// }

// impl TryFrom<ProtoFunctionName> for FunctionName {
//     type Error = String;
//
//     fn try_from(proto: ProtoFunctionName) -> Result<Self, Self::Error> {
//         let proto_function_name = proto.function_name.ok_or("Function name not found")?;
//         match proto_function_name {
//             function_name_type::FunctionName::Function(fqfn) => {
//                 Ok(FunctionName::Function(TryFrom::try_from(fqfn)?))
//             }
//             function_name_type::FunctionName::ResourceConstructor(fqfn) => {
//                 Ok(FunctionName::ResourceConstructor(TryFrom::try_from(fqfn)?))
//             }
//             function_name_type::FunctionName::ResourceMethod(fqfn) => {
//                 Ok(FunctionName::ResourceMethod(TryFrom::try_from(fqfn)?))
//             }
//         }
//     }
// }

// impl TryFrom<ProtoInstanceType> for InstanceType {
//     type Error = String;
//
//     fn try_from(value: ProtoInstanceType) -> Result<Self, Self::Error> {
//         let instance = value.instance.ok_or("Instance not found")?;
//
//         match instance {
//             Instance::Global(global_instance) => {
//                 let functions_global = global_instance
//                     .functions_global
//                     .ok_or("Functions global not found")?;
//
//                 Ok(InstanceType::Global {
//                     worker_name: global_instance
//                         .worker_name
//                         .map(Expr::try_from)
//                         .transpose()?
//                         .map(Box::new),
//                     component_dependency: TryFrom::try_from(functions_global)?,
//                 })
//             }
//             Instance::Package(package_instance) => {
//                 let package_name = package_instance
//                     .package_name
//                     .ok_or("Package name not found")?;
//                 let functions_in_package = package_instance
//                     .functions_in_package
//                     .ok_or("Functions in package not found")?;
//
//                 Ok(InstanceType::Package {
//                     worker_name: package_instance
//                         .worker_name
//                         .map(Expr::try_from)
//                         .transpose()?
//                         .map(Box::new),
//                     package_name: TryFrom::try_from(package_name)?,
//                     component_dependency: TryFrom::try_from(functions_in_package)?,
//                 })
//             }
//             Instance::Interface(interface_instance) => {
//                 let interface_name = interface_instance
//                     .interface_name
//                     .ok_or("Interface name not found")?;
//                 let functions_in_interface = interface_instance
//                     .functions_in_interface
//                     .ok_or("Functions in interface not found")?;
//
//                 Ok(InstanceType::Interface {
//                     worker_name: interface_instance
//                         .worker_name
//                         .map(Expr::try_from)
//                         .transpose()?
//                         .map(Box::new),
//                     interface_name: TryFrom::try_from(interface_name)?,
//                     component_dependency: TryFrom::try_from(functions_in_interface)?,
//                 })
//             }
//             Instance::PackageInterface(package_interface_instance) => {
//                 let functions_in_package_interface = package_interface_instance
//                     .functions_in_package_interface
//                     .ok_or("Functions in package interface not found")?;
//
//                 let interface_name = package_interface_instance
//                     .interface_name
//                     .ok_or("Interface name not found")?;
//                 let package_name = package_interface_instance
//                     .package_name
//                     .ok_or("Package name not found")?;
//
//                 Ok(InstanceType::PackageInterface {
//                     worker_name: package_interface_instance
//                         .worker_name
//                         .map(Expr::try_from)
//                         .transpose()?
//                         .map(Box::new),
//                     package_name: TryFrom::try_from(package_name)?,
//                     interface_name: TryFrom::try_from(interface_name)?,
//                     component_dependency: TryFrom::try_from(functions_in_package_interface)?,
//                 })
//             }
//             Instance::Resource(resource_instance) => {
//                 let resource_method_dict = resource_instance
//                     .resource_method_dict
//                     .ok_or("Resource method dictionary not found")?;
//                 Ok(InstanceType::Resource {
//                     worker_name: resource_instance
//                         .worker_name
//                         .map(Expr::try_from)
//                         .transpose()?
//                         .map(Box::new),
//                     package_name: resource_instance
//                         .package_name
//                         .map(TryFrom::try_from)
//                         .transpose()?,
//                     interface_name: resource_instance
//                         .interface_name
//                         .map(TryFrom::try_from)
//                         .transpose()?,
//                     resource_constructor: resource_instance.resource_constructor,
//                     resource_args: resource_instance
//                         .resource_args
//                         .into_iter()
//                         .map(TryFrom::try_from)
//                         .collect::<Result<Vec<Expr>, String>>()?,
//                     component_dependency: TryFrom::try_from(resource_method_dict)?,
//                 })
//             }
//         }
//     }
// }

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
