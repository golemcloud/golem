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
    CallType, DynamicParsedFunctionName, DynamicParsedFunctionReference, FunctionTypeRegistry,
    InferredType, ParsedFunctionSite, RegistryKey, RegistryValue, SemVer,
};
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedType, TypeEnum, TypeVariant};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fmt::{Debug, Display, Formatter};

// Global Function Dictionary is a user friendly projection of FunctionTypeRegistry for functions and arguments.
// In fact, type inference phases make use of FunctionDictionary.
// Unlike FunctionTypeRegistry, the function names in `FunctionDictionary` is closer to Rib grammar
// of invoking functions. Example: A RegistryKey of `[constructor]cart` in FunctionTypeRegistry becomes
// FunctionName::ResourceConstructor(cart) in FunctionDictionary
#[derive(Debug, Hash, Clone, Eq, PartialEq, Ord, PartialOrd, Default)]
pub struct FunctionDictionary {
    pub name_and_types: Vec<(FunctionName, FunctionType)>,
}

impl FunctionDictionary {
    pub fn get(&self, function_name: &FunctionName) -> Option<&FunctionType> {
        self.name_and_types.iter().find_map(|(name, ftype)| {
            if name == function_name {
                Some(ftype)
            } else {
                None
            }
        })
    }

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

// A `ResourceMethodDictionary` is a typesafe subset or projection of resource methods in
// `FunctionDictionary`.
// The `InstanceType` holds resource method dictionary instead of a full function method dictionary,
// if the instance is a resource creation.
// Given the Dictionaries do become part of InferredType (InferredType::InstanceType::Dictionaries)
// order of component loading into the rib context shouldn't change it's type.
#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ResourceMethodDictionary {
    pub map: BTreeMap<FullyQualifiedResourceMethod, FunctionType>,
}

impl From<&ResourceMethodDictionary> for FunctionDictionary {
    fn from(value: &ResourceMethodDictionary) -> Self {
        FunctionDictionary {
            name_and_types: value
                .map
                .iter()
                .map(|(key, value)| (FunctionName::ResourceMethod(key.clone()), value.clone()))
                .collect(),
        }
    }
}

impl FunctionDictionary {
    pub fn from_exports(exports: &[AnalysedExport]) -> Result<FunctionDictionary, String> {
        let registry = FunctionTypeRegistry::from_export_metadata(exports);
        Self::from_function_type_registry(&registry)
    }

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
                        let cases = variant_type
                            .cases
                            .iter()
                            .map(|x| (x.name.clone(), x.typ.as_ref().map(InferredType::from)))
                            .collect::<Vec<_>>();

                        map.push((
                            function_name,
                            FunctionType {
                                parameter_types: parameter_types.iter().map(|x| x.into()).collect(),
                                return_type: Some(InferredType::variant(cases)),
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
                    AnalysedType::Variant(variant_type) => match key {
                        RegistryKey::FunctionName(name) => {
                            let function_name = FunctionName::Variant(name.to_string());

                            let cases = variant_type
                                .cases
                                .iter()
                                .map(|x| (x.name.clone(), x.typ.as_ref().map(InferredType::from)))
                                .collect::<Vec<_>>();

                            map.push((
                                function_name,
                                FunctionType {
                                    parameter_types: vec![],
                                    return_type: Some(InferredType::variant(cases)),
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

            Err(e) => Err(format!("invalid function call. {e}")),
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
            _ => Err(format!("Invalid resource method name: {function_name}")),
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

impl Display for FunctionName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
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
                    version: version.as_ref().map(|v| v.0.to_string()),
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

impl FullyQualifiedResourceConstructor {
    pub fn parsed_function_site(&self) -> ParsedFunctionSite {
        if let Some(package_name) = &self.package_name {
            let interface_name = self.interface_name.clone().unwrap();

            ParsedFunctionSite::PackagedInterface {
                namespace: package_name.namespace.clone(),
                package: package_name.package_name.clone(),
                interface: self
                    .interface_name
                    .as_ref()
                    .map_or_else(|| "".to_string(), |i| i.name.clone()),
                version: interface_name
                    .version
                    .map(|x| SemVer(semver::Version::parse(&x).unwrap())),
            }
        } else if let Some(interface_name) = &self.interface_name {
            ParsedFunctionSite::Interface {
                name: interface_name.name.clone(),
            }
        } else {
            ParsedFunctionSite::Global
        }
    }
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
    pub fn dynamic_parsed_function_name(&self) -> Result<DynamicParsedFunctionName, String> {
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
            write!(f, "{package_name}")?
        }

        if let Some(interface_name) = &self.interface_name {
            write!(f, "/{interface_name}.")?;
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

    pub fn parameter_types(&self) -> Vec<AnalysedType> {
        self.parameter_types
            .iter()
            .map(|x| AnalysedType::try_from(x).unwrap())
            .collect()
    }

    pub fn return_type(&self) -> Option<AnalysedType> {
        self.return_type
            .clone()
            .map(|x| AnalysedType::try_from(&x).unwrap())
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::FunctionName;

    impl TryFrom<golem_api_grpc::proto::golem::rib::function_name_type::FunctionName> for FunctionName {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::function_name_type::FunctionName,
        ) -> Result<Self, Self::Error> {
            match value {
                golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::VariantName(name) => {
                    Ok(FunctionName::Variant(name))
                }
                golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::EnumName(name) => {
                    Ok(FunctionName::Enum(name))
                }
                golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::Function(fqfn) => {
                    Ok(FunctionName::Function(fqfn.try_into()?))
                }
                golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::ResourceConstructor(fqrc) => {
                    Ok(FunctionName::ResourceConstructor(fqrc.try_into()?))
                }
                golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::ResourceMethod(fqrm) => {
                    Ok(FunctionName::ResourceMethod(fqrm.try_into()?))
                }
            }
        }
    }

    impl From<FunctionName> for golem_api_grpc::proto::golem::rib::function_name_type::FunctionName {
        fn from(value: FunctionName) -> Self {
            match value {
                FunctionName::Variant(name) => {
                    golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::VariantName(name)
                }
                FunctionName::Enum(name) => {
                    golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::EnumName(name)
                }
                FunctionName::Function(fqfn) => {
                    golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::Function(fqfn.into())
                }
                FunctionName::ResourceConstructor(fqrc) => {
                    golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::ResourceConstructor(fqrc.into())
                }
                FunctionName::ResourceMethod(fqrm) => {
                    golem_api_grpc::proto::golem::rib::function_name_type::FunctionName::ResourceMethod(fqrm.into())
                }
            }
        }
    }
}
