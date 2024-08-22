// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bincode::{BorrowDecode, Decode, Encode};
use combine::stream::easy;
use combine::EasyParser;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::type_annotated_value_from_str;
use golem_wasm_rpc::Value;
use semver::{BuildMetadata, Prerelease};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Display;

#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub enum ParsedFunctionSite {
    Global,
    Interface {
        name: String,
    },
    PackagedInterface {
        namespace: String,
        package: String,
        interface: String,
        version: Option<SemVer>,
    },
}

#[derive(PartialEq, Eq, Clone)]
pub struct SemVer(pub semver::Version);

impl std::fmt::Debug for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Encode for SemVer {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        self.0.major.encode(encoder)?;
        self.0.minor.encode(encoder)?;
        self.0.patch.encode(encoder)?;
        self.0.pre.as_str().encode(encoder)?;
        self.0.build.as_str().encode(encoder)?;
        Ok(())
    }
}

impl Decode for SemVer {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        let major = u64::decode(decoder)?;
        let minor = u64::decode(decoder)?;
        let patch = u64::decode(decoder)?;
        let pre_str = String::decode(decoder)?;
        let build_str = String::decode(decoder)?;
        let pre = Prerelease::new(&pre_str)
            .map_err(|_| bincode::error::DecodeError::OtherString("Invalid prerelease".into()))?;
        let build = BuildMetadata::new(&build_str).map_err(|_| {
            bincode::error::DecodeError::OtherString("Invalid build metadata".into())
        })?;

        Ok(SemVer(semver::Version {
            major,
            minor,
            patch,
            pre,
            build,
        }))
    }
}

impl<'de> BorrowDecode<'de> for SemVer {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        let major = u64::borrow_decode(decoder)?;
        let minor = u64::borrow_decode(decoder)?;
        let patch = u64::borrow_decode(decoder)?;
        let pre_str = <Cow<'de, str> as BorrowDecode>::borrow_decode(decoder)?;
        let build_str = <Cow<'de, str> as BorrowDecode>::borrow_decode(decoder)?;
        let pre = Prerelease::new(&pre_str)
            .map_err(|_| bincode::error::DecodeError::OtherString("Invalid prerelease".into()))?;
        let build = BuildMetadata::new(&build_str).map_err(|_| {
            bincode::error::DecodeError::OtherString("Invalid build metadata".into())
        })?;
        Ok(SemVer(semver::Version {
            major,
            minor,
            patch,
            pre,
            build,
        }))
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::SemVersion> for SemVer {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::rib::SemVersion) -> Result<Self, Self::Error> {
        Ok(SemVer(semver::Version {
            major: value.major,
            minor: value.minor,
            patch: value.patch,
            pre: Prerelease::new(&value.pre).map_err(|_| "Invalid prerelease".to_string())?,
            build: BuildMetadata::new(&value.build)
                .map_err(|_| "Invalid build metadata".to_string())?,
        }))
    }
}

impl From<SemVer> for golem_api_grpc::proto::golem::rib::SemVersion {
    fn from(value: SemVer) -> Self {
        golem_api_grpc::proto::golem::rib::SemVersion {
            major: value.0.major,
            minor: value.0.minor,
            patch: value.0.patch,
            pre: value.0.pre.to_string(),
            build: value.0.build.to_string(),
        }
    }
}

impl ParsedFunctionSite {
    pub fn interface_name(&self) -> Option<String> {
        match self {
            Self::Global => None,
            Self::Interface { name } => Some(name.clone()),
            Self::PackagedInterface {
                namespace,
                package,
                interface,
                version: None,
            } => Some(format!("{namespace}:{package}/{interface}")),
            Self::PackagedInterface {
                namespace,
                package,
                interface,
                version: Some(version),
            } => Some(format!("{namespace}:{package}/{interface}@{}", version.0)),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::ParsedFunctionSite> for ParsedFunctionSite {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::rib::ParsedFunctionSite,
    ) -> Result<Self, Self::Error> {
        let site = value.site.ok_or("Missing site".to_string())?;
        match site {
            golem_api_grpc::proto::golem::rib::parsed_function_site::Site::Global(_) => {
                Ok(Self::Global)
            }
            golem_api_grpc::proto::golem::rib::parsed_function_site::Site::Interface(
                golem_api_grpc::proto::golem::rib::InterfaceFunctionSite { name },
            ) => Ok(Self::Interface { name }),
            golem_api_grpc::proto::golem::rib::parsed_function_site::Site::PackageInterface(
                golem_api_grpc::proto::golem::rib::PackageInterfaceFunctionSite {
                    namespace,
                    package,
                    interface,
                    version,
                },
            ) => {
                let version = match version {
                    Some(version) => Some(version.try_into()?),
                    None => None,
                };

                Ok(Self::PackagedInterface {
                    namespace,
                    package,
                    interface,
                    version,
                })
            }
        }
    }
}

impl From<ParsedFunctionSite> for golem_api_grpc::proto::golem::rib::ParsedFunctionSite {
    fn from(value: ParsedFunctionSite) -> Self {
        let site = match value {
            ParsedFunctionSite::Global => {
                golem_api_grpc::proto::golem::rib::parsed_function_site::Site::Global(
                    golem_api_grpc::proto::golem::rib::GlobalFunctionSite {},
                )
            }
            ParsedFunctionSite::Interface { name } => {
                golem_api_grpc::proto::golem::rib::parsed_function_site::Site::Interface(
                    golem_api_grpc::proto::golem::rib::InterfaceFunctionSite { name },
                )
            }
            ParsedFunctionSite::PackagedInterface {
                namespace,
                package,
                interface,
                version,
            } => golem_api_grpc::proto::golem::rib::parsed_function_site::Site::PackageInterface(
                golem_api_grpc::proto::golem::rib::PackageInterfaceFunctionSite {
                    namespace,
                    package,
                    interface,
                    version: version.map(|v| v.into()),
                },
            ),
        };
        golem_api_grpc::proto::golem::rib::ParsedFunctionSite { site: Some(site) }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub enum ParsedFunctionReference {
    Function {
        function: String,
    },
    RawResourceConstructor {
        resource: String,
    },
    RawResourceDrop {
        resource: String,
    },
    RawResourceMethod {
        resource: String,
        method: String,
    },
    RawResourceStaticMethod {
        resource: String,
        method: String,
    },
    IndexedResourceConstructor {
        resource: String,
        resource_params: Vec<String>,
    },
    IndexedResourceMethod {
        resource: String,
        resource_params: Vec<String>,
        method: String,
    },
    IndexedResourceStaticMethod {
        resource: String,
        resource_params: Vec<String>,
        method: String,
    },
    IndexedResourceDrop {
        resource: String,
        resource_params: Vec<String>,
    },
}

impl Display for ParsedFunctionReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function_name = match self {
            Self::Function { function } => function.clone(),
            Self::RawResourceConstructor { resource } => format!("{}.new", resource),
            Self::IndexedResourceConstructor {
                resource,
                resource_params,
            } => {
                format!("{}({}).new", resource, resource_params.join(", "))
            }
            Self::RawResourceMethod { resource, method } => format!("{}.{}", resource, method),
            Self::RawResourceStaticMethod { resource, method } => {
                format!("[static]{}.{}", resource, method)
            }
            Self::RawResourceDrop { resource } => format!("{}.drop", resource),
            Self::IndexedResourceDrop {
                resource,
                resource_params,
            } => {
                format!("{}({}).drop", resource, resource_params.join(", "))
            }
            Self::IndexedResourceMethod {
                resource,
                resource_params,
                method,
            } => {
                format!("{}({}).{}", resource, resource_params.join(", "), method)
            }
            Self::IndexedResourceStaticMethod {
                resource,
                resource_params,
                method,
            } => {
                format!(
                    "[static]{}({}).{}",
                    resource,
                    resource_params.join(", "),
                    method
                )
            }
        };

        write!(f, "{}", function_name)
    }
}

impl ParsedFunctionReference {
    pub fn function_name(&self) -> String {
        match self {
            Self::Function { function, .. } => function.clone(),
            Self::RawResourceConstructor { resource, .. } => format!("[constructor]{resource}"),
            Self::RawResourceDrop { resource, .. } => format!("[drop]{resource}"),
            Self::RawResourceMethod {
                resource, method, ..
            } => format!("[method]{resource}.{method}"),
            Self::RawResourceStaticMethod {
                resource, method, ..
            } => format!("[static]{resource}.{method}"),
            ParsedFunctionReference::IndexedResourceConstructor { resource, .. } => {
                format!("[constructor]{resource}")
            }
            ParsedFunctionReference::IndexedResourceMethod {
                resource, method, ..
            } => {
                format!("[method]{resource}.{method}")
            }
            ParsedFunctionReference::IndexedResourceStaticMethod {
                resource, method, ..
            } => {
                format!("[static]{resource}.{method}")
            }
            ParsedFunctionReference::IndexedResourceDrop { resource, .. } => {
                format!("[drop]{resource}")
            }
        }
    }

    pub fn method_as_static(&self) -> Option<ParsedFunctionReference> {
        match self {
            Self::RawResourceMethod { resource, method } => Some(Self::RawResourceStaticMethod {
                resource: resource.clone(),
                method: method.clone(),
            }),
            Self::IndexedResourceMethod {
                resource,
                resource_params,
                method,
            } => Some(Self::IndexedResourceStaticMethod {
                resource: resource.clone(),
                resource_params: resource_params.clone(),
                method: method.clone(),
            }),
            _ => None,
        }
    }

    pub fn is_indexed_resource(&self) -> bool {
        matches!(
            self,
            Self::IndexedResourceConstructor { .. }
                | Self::IndexedResourceMethod { .. }
                | Self::IndexedResourceStaticMethod { .. }
                | Self::IndexedResourceDrop { .. }
        )
    }

    pub fn raw_resource_params(&self) -> Option<&Vec<String>> {
        match self {
            Self::IndexedResourceConstructor {
                resource_params, ..
            }
            | Self::IndexedResourceMethod {
                resource_params, ..
            }
            | Self::IndexedResourceStaticMethod {
                resource_params, ..
            }
            | Self::IndexedResourceDrop {
                resource_params, ..
            } => Some(resource_params),
            _ => None,
        }
    }

    pub fn resource_name(&self) -> Option<&String> {
        match self {
            Self::RawResourceConstructor { resource }
            | Self::RawResourceDrop { resource }
            | Self::RawResourceMethod { resource, .. }
            | Self::RawResourceStaticMethod { resource, .. }
            | Self::IndexedResourceConstructor { resource, .. }
            | Self::IndexedResourceMethod { resource, .. }
            | Self::IndexedResourceStaticMethod { resource, .. }
            | Self::IndexedResourceDrop { resource, .. } => Some(resource),
            _ => None,
        }
    }

    pub fn resource_params(&self, types: &[AnalysedType]) -> Result<Option<Vec<Value>>, String> {
        if let Some(raw_params) = self.raw_resource_params() {
            if raw_params.len() != types.len() {
                Err(format!(
                    "Resource params count mismatch: expected {}, got {}",
                    types.len(),
                    raw_params.len()
                ))
            } else {
                let mut result = Vec::new();
                for (raw_param, param_type) in raw_params.iter().zip(types.iter()) {
                    let type_annotated_value: TypeAnnotatedValue =
                        type_annotated_value_from_str(param_type, raw_param)
                            .map_err(|err| err.to_string())?;
                    let value = type_annotated_value.try_into()?;
                    result.push(value);
                }
                Ok(Some(result))
            }
        } else {
            Ok(None)
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::ParsedFunctionReference>
    for ParsedFunctionReference
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::rib::ParsedFunctionReference,
    ) -> Result<Self, Self::Error> {
        let function = value
            .function_reference
            .ok_or("Missing function".to_string())?;
        match function {
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::Function(golem_api_grpc::proto::golem::rib::FunctionFunctionReference {
                                                                                                          function
                                                                                                      }) => {
                Ok(Self::Function { function })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceConstructor(golem_api_grpc::proto::golem::rib::RawResourceConstructorFunctionReference {
                                                                                                                        resource
                                                                                                                    }) => {
                Ok(Self::RawResourceConstructor { resource })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceMethod(golem_api_grpc::proto::golem::rib::RawResourceMethodFunctionReference {
                                                                                                                   resource,
                                                                                                                   method
                                                                                                               }) => {
                Ok(Self::RawResourceMethod { resource, method })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceStaticMethod(golem_api_grpc::proto::golem::rib::RawResourceStaticMethodFunctionReference {
                                                                                                                         resource,
                                                                                                                         method
                                                                                                                     }) => {
                Ok(Self::RawResourceStaticMethod { resource, method })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceDrop(golem_api_grpc::proto::golem::rib::RawResourceDropFunctionReference {
                                                                                                                 resource
                                                                                                             }) => {
                Ok(Self::RawResourceDrop { resource })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceConstructor(golem_api_grpc::proto::golem::rib::IndexedResourceConstructorFunctionReference {
                                                                                                                            resource,
                                                                                                                            resource_params
                                                                                                                        }) => {
                Ok(Self::IndexedResourceConstructor {
                    resource,
                    resource_params,
                })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceMethod(golem_api_grpc::proto::golem::rib::IndexedResourceMethodFunctionReference {
                                                                                                                       resource,
                                                                                                                       resource_params,
                                                                                                                       method
                                                                                                                   }) => {
                Ok(Self::IndexedResourceMethod {
                    resource,
                    resource_params,
                    method,
                })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceStaticMethod(golem_api_grpc::proto::golem::rib::IndexedResourceStaticMethodFunctionReference {
                                                                                                                             resource,
                                                                                                                             resource_params,
                                                                                                                             method
                                                                                                                         }) => {
                Ok(Self::IndexedResourceStaticMethod {
                    resource,
                    resource_params,
                    method,
                })
            }
            golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceDrop(golem_api_grpc::proto::golem::rib::IndexedResourceDropFunctionReference {
                                                                                                                     resource,
                                                                                                                     resource_params
                                                                                                                 }) => {
                Ok(Self::IndexedResourceDrop {
                    resource,
                    resource_params,
                })
            }
        }
    }
}

impl From<ParsedFunctionReference> for golem_api_grpc::proto::golem::rib::ParsedFunctionReference {
    fn from(value: ParsedFunctionReference) -> Self {
        let function = match value {
            ParsedFunctionReference::Function { function } => golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::Function(
                golem_api_grpc::proto::golem::rib::FunctionFunctionReference { function },
            ),
            ParsedFunctionReference::RawResourceConstructor { resource } => {
                golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceConstructor(
                    golem_api_grpc::proto::golem::rib::RawResourceConstructorFunctionReference {
                        resource,
                    },
                )
            }
            ParsedFunctionReference::RawResourceMethod { resource, method } => {
                golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceMethod(
                    golem_api_grpc::proto::golem::rib::RawResourceMethodFunctionReference {
                        resource,
                        method,
                    },
                )
            }
            ParsedFunctionReference::RawResourceStaticMethod { resource, method } => {
                golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceStaticMethod(
                    golem_api_grpc::proto::golem::rib::RawResourceStaticMethodFunctionReference {
                        resource,
                        method,
                    },
                )
            }
            ParsedFunctionReference::RawResourceDrop { resource } => golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::RawResourceDrop(
                golem_api_grpc::proto::golem::rib::RawResourceDropFunctionReference { resource },
            ),
            ParsedFunctionReference::IndexedResourceConstructor {
                resource,
                resource_params,
            } => golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceConstructor(
                golem_api_grpc::proto::golem::rib::IndexedResourceConstructorFunctionReference {
                    resource,
                    resource_params,
                },
            ),
            ParsedFunctionReference::IndexedResourceMethod {
                resource,
                resource_params,
                method,
            } => golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceMethod(
                golem_api_grpc::proto::golem::rib::IndexedResourceMethodFunctionReference {
                    resource,
                    resource_params,
                    method,
                },
            ),
            ParsedFunctionReference::IndexedResourceStaticMethod {
                resource,
                resource_params,
                method,
            } => golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceStaticMethod(
                golem_api_grpc::proto::golem::rib::IndexedResourceStaticMethodFunctionReference {
                    resource,
                    resource_params,
                    method,
                },
            ),
            ParsedFunctionReference::IndexedResourceDrop {
                resource,
                resource_params,
            } => golem_api_grpc::proto::golem::rib::parsed_function_reference::FunctionReference::IndexedResourceDrop(
                golem_api_grpc::proto::golem::rib::IndexedResourceDropFunctionReference {
                    resource,
                    resource_params,
                },
            ),
        };
        golem_api_grpc::proto::golem::rib::ParsedFunctionReference {
            function_reference: Some(function),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct ParsedFunctionName {
    pub site: ParsedFunctionSite,
    pub function: ParsedFunctionReference,
}

impl Serialize for ParsedFunctionName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let function_name = self.to_string();
        serializer.serialize_str(&function_name)
    }
}

impl<'de> Deserialize<'de> for ParsedFunctionName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let function_name = String::deserialize(deserializer)?;
        ParsedFunctionName::parse(function_name).map_err(serde::de::Error::custom)
    }
}

impl Display for ParsedFunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function_name = self
            .site
            .interface_name()
            .map_or(self.function.function_name(), |interface| {
                format!("{}.{{{}}}", interface, self.function)
            });
        write!(f, "{}", function_name)
    }
}

impl ParsedFunctionName {
    pub fn new(site: ParsedFunctionSite, function: ParsedFunctionReference) -> Self {
        Self { site, function }
    }

    pub fn global(name: String) -> Self {
        Self {
            site: ParsedFunctionSite::Global,
            function: ParsedFunctionReference::Function { function: name },
        }
    }

    pub fn on_interface(interface: String, function: String) -> Self {
        Self {
            site: ParsedFunctionSite::Interface { name: interface },
            function: ParsedFunctionReference::Function { function },
        }
    }

    pub fn parse(name: impl AsRef<str>) -> Result<Self, String> {
        let name = name.as_ref();

        let mut parser = crate::parser::call::function_name();

        let result: Result<(ParsedFunctionName, &str), easy::ParseError<&str>> =
            parser.easy_parse(name);

        match result {
            Ok((parsed, _)) => Ok(parsed),
            Err(error) => {
                let error_message = error
                    .map_position(|p| p.translate_position(name))
                    .to_string();
                Err(error_message)
            }
        }
    }

    pub fn site(&self) -> &ParsedFunctionSite {
        &self.site
    }

    pub fn function(&self) -> &ParsedFunctionReference {
        &self.function
    }

    pub fn method_as_static(&self) -> Option<Self> {
        self.function.method_as_static().map(|function| Self {
            site: self.site.clone(),
            function,
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::rib::ParsedFunctionName> for ParsedFunctionName {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::rib::ParsedFunctionName,
    ) -> Result<Self, Self::Error> {
        let site = ParsedFunctionSite::try_from(value.site.ok_or("Missing site".to_string())?)?;
        let function = ParsedFunctionReference::try_from(
            value.function.ok_or("Missing function".to_string())?,
        )?;
        Ok(Self { site, function })
    }
}

impl From<ParsedFunctionName> for golem_api_grpc::proto::golem::rib::ParsedFunctionName {
    fn from(value: ParsedFunctionName) -> Self {
        golem_api_grpc::proto::golem::rib::ParsedFunctionName {
            site: Some(value.site.into()),
            function: Some(value.function.into()),
        }
    }
}

#[cfg(test)]
mod function_name_tests {
    use super::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer};

    #[test]
    fn parse_function_name_global() {
        let parsed = ParsedFunctionName::parse("run-example").expect("Parsing failed");
        assert_eq!(parsed.site().interface_name(), None);
        assert_eq!(parsed.function().function_name(), "run-example");
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function {
                    function: "run-example".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_in_exported_interface_no_package() {
        let parsed = ParsedFunctionName::parse("interface.{fn1}").expect("Parsing failed");
        println!("{:?}", parsed);
        assert_eq!(
            parsed.site().interface_name(),
            Some("interface".to_string())
        );
        assert_eq!(parsed.function().function_name(), "fn1".to_string());
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::Interface {
                    name: "interface".to_string()
                },
                function: ParsedFunctionReference::Function {
                    function: "fn1".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_in_exported_interface() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{fn1}").expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(parsed.function().function_name(), "fn1".to_string());
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::Function {
                    function: "fn1".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_in_versioned_exported_interface() {
        let parsed = ParsedFunctionName::parse("wasi:cli/run@0.2.0.{run}").expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("wasi:cli/run@0.2.0".to_string())
        );
        assert_eq!(parsed.function().function_name(), "run".to_string());
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "wasi".to_string(),
                    package: "cli".to_string(),
                    interface: "run".to_string(),
                    version: Some(SemVer(semver::Version::new(0, 2, 0))),
                },
                function: ParsedFunctionReference::Function {
                    function: "run".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_constructor_syntax_sugar() {
        let parsed =
            ParsedFunctionName::parse("ns:name/interface.{resource1.new}").expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[constructor]resource1".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceConstructor {
                    resource: "resource1".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_constructor() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{[constructor]resource1}")
            .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[constructor]resource1".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceConstructor {
                    resource: "resource1".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_indexed_constructor_1() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{resource1().new}")
            .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[constructor]resource1".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(parsed.function().raw_resource_params(), Some(&vec![]));
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![],
                },
            }
        );
    }

    #[test]
    fn parse_function_name_indexed_constructor_2() {
        let parsed =
            ParsedFunctionName::parse("ns:name/interface.{resource1(\"hello\", 1, true).new}")
                .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[constructor]resource1".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(
            parsed.function().raw_resource_params(),
            Some(&vec![
                "\"hello\"".to_string(),
                "1".to_string(),
                "true".to_string(),
            ])
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "1".to_string(),
                        "true".to_string(),
                    ],
                },
            },
        );
    }

    #[test]
    fn parse_function_name_indexed_constructor_3() {
        let parsed = ParsedFunctionName::parse(
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).new}",
        )
        .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[constructor]resource1".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(
            parsed.function().raw_resource_params(),
            Some(&vec![
                "\"hello\"".to_string(),
                "{ field-a: some(1) }".to_string(),
            ])
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "{ field-a: some(1) }".to_string(),
                    ],
                },
            },
        );
    }

    #[test]
    fn parse_function_name_method_syntax_sugar() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{resource1.do-something}")
            .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[method]resource1.do-something".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something".to_string(),
                },
            }
        );
    }

    #[test]
    fn parse_function_name_method() {
        let parsed =
            ParsedFunctionName::parse("ns:name/interface.{[method]resource1.do-something}")
                .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[method]resource1.do-something".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something".to_string(),
                },
            }
        );
    }

    #[test]
    fn parse_function_name_static_method_syntax_sugar() {
        // Note: the syntax sugared version cannot distinguish between method and static - so we need to check the actual existence of
        // the function and fallback.
        let parsed = ParsedFunctionName::parse("ns:name/interface.{resource1.do-something-static}")
            .expect("Parsing failed")
            .method_as_static()
            .unwrap();
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[static]resource1.do-something-static".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceStaticMethod {
                    resource: "resource1".to_string(),
                    method: "do-something-static".to_string(),
                },
            }
        );
    }

    #[test]
    fn parse_function_name_static() {
        let parsed =
            ParsedFunctionName::parse("ns:name/interface.{[static]resource1.do-something-static}")
                .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[static]resource1.do-something-static".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceStaticMethod {
                    resource: "resource1".to_string(),
                    method: "do-something-static".to_string(),
                },
            }
        );
    }

    #[test]
    fn parse_function_name_drop_syntax_sugar() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{resource1.drop}")
            .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[drop]resource1".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceDrop {
                    resource: "resource1".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_function_name_indexed_drop_1() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{resource1().drop}")
            .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[drop]resource1".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(parsed.function().raw_resource_params(), Some(&vec![]));
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![],
                },
            }
        )
    }

    #[test]
    fn parse_function_name_indexed_drop_2() {
        let parsed =
            ParsedFunctionName::parse("ns:name/interface.{resource1(\"hello\", 1, true).drop}")
                .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[drop]resource1".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(
            parsed.function().raw_resource_params(),
            Some(&vec![
                "\"hello\"".to_string(),
                "1".to_string(),
                "true".to_string(),
            ])
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "1".to_string(),
                        "true".to_string(),
                    ],
                },
            }
        );
    }

    #[test]
    fn parse_function_name_indexed_drop_3() {
        let parsed = ParsedFunctionName::parse(
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).drop}",
        )
        .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[drop]resource1".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(
            parsed.function().raw_resource_params(),
            Some(&vec![
                "\"hello\"".to_string(),
                "{ field-a: some(1) }".to_string(),
            ])
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "{ field-a: some(1) }".to_string(),
                    ],
                },
            },
        );
    }

    #[test]
    fn parse_function_name_drop() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{[drop]resource1}")
            .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[drop]resource1".to_string()
        );
        assert_eq!(
            parsed,
            ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: ParsedFunctionReference::RawResourceDrop {
                    resource: "resource1".to_string()
                },
            }
        );
    }

    fn round_trip_function_name_parse(input: &str) {
        let parsed = ParsedFunctionName::parse(input).expect("Input Parsing failed");
        let parsed_written =
            ParsedFunctionName::parse(parsed.to_string()).expect("Round-trip parsing failed");
        assert_eq!(parsed, parsed_written);
    }

    #[test]
    fn test_parsed_function_name_display() {
        round_trip_function_name_parse("run-example");
        round_trip_function_name_parse("interface.{fn1}");
        round_trip_function_name_parse("wasi:cli/run@0.2.0.{run}");
        round_trip_function_name_parse("ns:name/interface.{resource1.new}");
        round_trip_function_name_parse("ns:name/interface.{[constructor]resource1}");
        round_trip_function_name_parse("ns:name/interface.{resource1().new}");
        round_trip_function_name_parse("ns:name/interface.{resource1(\"hello\", 1, true).new}");
        round_trip_function_name_parse(
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).new}",
        );
        round_trip_function_name_parse("ns:name/interface.{resource1.do-something}");
        round_trip_function_name_parse(
            "ns:name/interface.{resource1(\"hello\", 1, true).do-something}",
        );
        round_trip_function_name_parse(
            "ns:name/interface.{resource1(\"hello\", 1, { field-a: some(1) }).do-something}",
        );
        round_trip_function_name_parse("ns:name/interface.{[static]resource1.do-something-static}");
        round_trip_function_name_parse(
            "ns:name/interface.{[static]resource1(\"hello\", 1, true).do-something-static}",
        );
        round_trip_function_name_parse("ns:name/interface.{[static]resource1(\"hello\", 1, { field-a: some(1) }).do-something-static}");
        round_trip_function_name_parse("ns:name/interface.{resource1.drop}");
        round_trip_function_name_parse("ns:name/interface.{resource1().drop}");
        round_trip_function_name_parse("ns:name/interface.{resource1(\"hello\", 1, true).drop}");
        round_trip_function_name_parse(
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).drop}",
        );
        round_trip_function_name_parse("ns:name/interface.{[drop]resource1}");
    }
}
