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

use crate::Expr;
use bincode::{BorrowDecode, Decode, Encode};
use combine::stream::position::Stream;
use combine::{eof, EasyParser, Parser};
use golem_wasm_rpc::{parse_value_and_type, ValueAndType};
use semver::{BuildMetadata, Prerelease};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Display;

#[derive(PartialEq, Hash, Eq, Clone, Ord, PartialOrd)]
pub struct SemVer(pub semver::Version);

impl SemVer {
    pub fn parse(version: &str) -> Result<Self, String> {
        semver::Version::parse(version)
            .map(SemVer)
            .map_err(|e| format!("Invalid semver string: {e}"))
    }
}

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

impl<Context> Decode<Context> for SemVer {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
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

impl<'de, Context> BorrowDecode<'de, Context> for SemVer {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        let major = u64::borrow_decode(decoder)?;
        let minor = u64::borrow_decode(decoder)?;
        let patch = u64::borrow_decode(decoder)?;
        let pre_str = <Cow<'de, str> as BorrowDecode<Context>>::borrow_decode(decoder)?;
        let build_str = <Cow<'de, str> as BorrowDecode<Context>>::borrow_decode(decoder)?;
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

#[derive(Debug, Hash, PartialEq, Eq, Clone, Encode, Decode, Ord, PartialOrd)]
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

impl ParsedFunctionSite {
    pub fn parse(name: impl AsRef<str>) -> Result<Self, String> {
        ParsedFunctionName::parse(format!("{}.{{x}}", name.as_ref()))
            .map(|ParsedFunctionName { site, .. }| site)
    }

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

    pub fn unversioned(&self) -> ParsedFunctionSite {
        match self {
            ParsedFunctionSite::Global => ParsedFunctionSite::Global,
            ParsedFunctionSite::Interface { name } => {
                ParsedFunctionSite::Interface { name: name.clone() }
            }
            ParsedFunctionSite::PackagedInterface {
                namespace,
                package,
                interface,
                version: _,
            } => ParsedFunctionSite::PackagedInterface {
                namespace: namespace.clone(),
                package: package.clone(),
                interface: interface.clone(),
                version: None,
            },
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub enum DynamicParsedFunctionReference {
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
        resource_params: Vec<Expr>,
    },
    IndexedResourceMethod {
        resource: String,
        resource_params: Vec<Expr>,
        method: String,
    },
    IndexedResourceStaticMethod {
        resource: String,
        resource_params: Vec<Expr>,
        method: String,
    },
    IndexedResourceDrop {
        resource: String,
        resource_params: Vec<Expr>,
    },
}

impl DynamicParsedFunctionReference {
    pub fn name_pretty(&self) -> String {
        match self {
            DynamicParsedFunctionReference::Function { function, .. } => function.clone(),
            DynamicParsedFunctionReference::RawResourceConstructor { resource, .. } => {
                resource.to_string()
            }
            DynamicParsedFunctionReference::RawResourceDrop { .. } => "drop".to_string(),
            DynamicParsedFunctionReference::RawResourceMethod { method, .. } => method.to_string(),
            DynamicParsedFunctionReference::RawResourceStaticMethod { method, .. } => {
                method.to_string()
            }
            DynamicParsedFunctionReference::IndexedResourceConstructor {
                resource,
                resource_params,
            } => format!(
                "{}({})",
                resource,
                resource_params
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            DynamicParsedFunctionReference::IndexedResourceMethod { method, .. } => {
                method.to_string()
            }
            DynamicParsedFunctionReference::IndexedResourceStaticMethod { method, .. } => {
                method.to_string()
            }
            DynamicParsedFunctionReference::IndexedResourceDrop { .. } => "drop".to_string(),
        }
    }

    fn to_static(&self) -> ParsedFunctionReference {
        match self {
            Self::Function { function } => ParsedFunctionReference::Function {
                function: function.clone(),
            },
            Self::RawResourceConstructor { resource } => {
                ParsedFunctionReference::RawResourceConstructor {
                    resource: resource.clone(),
                }
            }
            Self::RawResourceDrop { resource } => ParsedFunctionReference::RawResourceDrop {
                resource: resource.clone(),
            },
            Self::RawResourceMethod { resource, method } => {
                ParsedFunctionReference::RawResourceMethod {
                    resource: resource.clone(),
                    method: method.clone(),
                }
            }
            Self::RawResourceStaticMethod { resource, method } => {
                ParsedFunctionReference::RawResourceStaticMethod {
                    resource: resource.clone(),
                    method: method.clone(),
                }
            }
            Self::IndexedResourceConstructor {
                resource,
                resource_params,
            } => ParsedFunctionReference::IndexedResourceConstructor {
                resource: resource.clone(),
                resource_params: resource_params
                    .iter()
                    .map(|expr| expr.to_string())
                    .collect(),
            },
            Self::IndexedResourceMethod {
                resource,
                resource_params,
                method,
            } => ParsedFunctionReference::IndexedResourceMethod {
                resource: resource.clone(),
                resource_params: resource_params
                    .iter()
                    .map(|expr| expr.to_string())
                    .collect(),
                method: method.clone(),
            },
            Self::IndexedResourceStaticMethod {
                resource,
                resource_params,
                method,
            } => ParsedFunctionReference::IndexedResourceStaticMethod {
                resource: resource.clone(),
                resource_params: resource_params
                    .iter()
                    .map(|expr| expr.to_string())
                    .collect(),
                method: method.clone(),
            },
            Self::IndexedResourceDrop {
                resource,
                resource_params,
            } => ParsedFunctionReference::IndexedResourceDrop {
                resource: resource.clone(),
                resource_params: resource_params
                    .iter()
                    .map(|expr| expr.to_string())
                    .collect(),
            },
        }
    }

    pub fn raw_resource_params_mut(&mut self) -> Option<&mut [Expr]> {
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
            } => Some(resource_params.as_mut_slice()),
            _ => None,
        }
    }

    pub fn raw_resource_params(&self) -> Option<&Vec<Expr>> {
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
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Encode, Decode)]
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
            Self::RawResourceConstructor { resource } => format!("{resource}.new"),
            Self::IndexedResourceConstructor {
                resource,
                resource_params,
            } => {
                format!("{}({}).new", resource, resource_params.join(", "))
            }
            Self::RawResourceMethod { resource, method } => format!("{resource}.{method}"),
            Self::RawResourceStaticMethod { resource, method } => {
                format!("[static]{resource}.{method}")
            }
            Self::RawResourceDrop { resource } => format!("{resource}.drop"),
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

        write!(f, "{function_name}")
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
            Self::IndexedResourceConstructor { resource, .. } => {
                format!("[constructor]{resource}")
            }
            Self::IndexedResourceMethod {
                resource, method, ..
            } => {
                format!("[method]{resource}.{method}")
            }
            Self::IndexedResourceStaticMethod {
                resource, method, ..
            } => {
                format!("[static]{resource}.{method}")
            }
            Self::IndexedResourceDrop { resource, .. } => {
                format!("[drop]{resource}")
            }
        }
    }

    pub fn resource_method_name(&self) -> Option<String> {
        match self {
            Self::IndexedResourceStaticMethod { method, .. }
            | Self::RawResourceMethod { method, .. }
            | Self::RawResourceStaticMethod { method, .. }
            | Self::IndexedResourceMethod { method, .. } => Some(method.clone()),
            _ => None,
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

    pub fn resource_params(
        &self,
        types: &[golem_wasm_ast::analysis::AnalysedType],
    ) -> Result<Option<Vec<ValueAndType>>, String> {
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
                    let value_and_type: ValueAndType = parse_value_and_type(param_type, raw_param)?;
                    result.push(value_and_type);
                }
                Ok(Some(result))
            }
        } else {
            Ok(None)
        }
    }
}

// DynamicParsedFunctionName is different from ParsedFunctionName.
// In `DynamicParsedFunctionName` the resource parameters are `Expr` (Rib) while they are `String`
// in `ParsedFunctionName`.
// `Expr` implies the real values are yet to be computed, while `String`
// in ParsedFunctionName is a textual representation of the evaluated values.
// `Examples`:
// `DynamicParsedFunctionName` : ns:name/interface.{resource1(identifier1, { field-a: some(identifier2) }).new}
// `ParsedFunctionName` : ns:name/interface.{resource1("foo", { field-a: some("bar") }).new}
#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub struct DynamicParsedFunctionName {
    pub site: ParsedFunctionSite,
    pub function: DynamicParsedFunctionReference,
}

impl DynamicParsedFunctionName {
    pub fn parse(name: impl AsRef<str>) -> Result<Self, String> {
        let name = name.as_ref();

        let mut parser = crate::parser::call::function_name();

        let result = parser.easy_parse(Stream::new(name));

        match result {
            Ok((parsed, _)) => Ok(parsed),
            Err(error) => {
                let error_message = error.map_position(|p| p.to_string()).to_string();
                Err(error_message)
            }
        }
    }

    pub fn function_name_with_prefix_identifiers(&self) -> String {
        self.to_parsed_function_name().function.function_name()
    }

    // Usually resource name in the real metadata consist of prefixes such as [constructor]
    // However, the one obtained through the dynamic-parsed-function-name is simple without these prefix
    pub fn resource_name_simplified(&self) -> Option<String> {
        self.to_parsed_function_name()
            .function
            .resource_name()
            .cloned()
    }

    // Usually resource method in the real metadata consist of prefixes such as [method]
    pub fn resource_method_name_simplified(&self) -> Option<String> {
        self.to_parsed_function_name()
            .function
            .resource_method_name()
    }

    pub fn raw_resource_params_mut(&mut self) -> Option<&mut [Expr]> {
        self.function.raw_resource_params_mut()
    }

    //
    pub fn to_parsed_function_name(&self) -> ParsedFunctionName {
        ParsedFunctionName {
            site: self.site.clone(),
            function: self.function.to_static(),
        }
    }
}

impl Display for DynamicParsedFunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function_name = self.to_parsed_function_name().to_string();
        write!(f, "{function_name}")
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Encode, Decode)]
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
        write!(f, "{function_name}")
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

        let mut parser = crate::parser::call::function_name().skip(eof());

        let result = parser.easy_parse(Stream::new(name));

        match result {
            Ok((parsed, _)) => Ok(parsed.to_parsed_function_name()),
            Err(error) => {
                let error_message = error.map_position(|p| p.to_string()).to_string();
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

    pub fn is_constructor(&self) -> Option<&str> {
        match &self.function {
            ParsedFunctionReference::RawResourceConstructor { resource, .. }
            | ParsedFunctionReference::IndexedResourceConstructor { resource, .. } => {
                Some(resource)
            }
            _ => None,
        }
    }

    pub fn is_method(&self) -> Option<&str> {
        match &self.function {
            ParsedFunctionReference::RawResourceMethod { resource, .. }
            | ParsedFunctionReference::IndexedResourceMethod { resource, .. }
            | ParsedFunctionReference::RawResourceStaticMethod { resource, .. }
            | ParsedFunctionReference::IndexedResourceStaticMethod { resource, .. } => {
                Some(resource)
            }
            _ => None,
        }
    }

    pub fn is_static_method(&self) -> Option<&str> {
        match &self.function {
            ParsedFunctionReference::RawResourceStaticMethod { resource, .. }
            | ParsedFunctionReference::IndexedResourceStaticMethod { resource, .. } => {
                Some(resource)
            }
            _ => None,
        }
    }

    pub fn with_site(&self, site: ParsedFunctionSite) -> Self {
        Self {
            site,
            function: self.function.clone(),
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::{
        DynamicParsedFunctionName, DynamicParsedFunctionReference, Expr, ParsedFunctionName,
        ParsedFunctionReference, ParsedFunctionSite, SemVer,
    };
    use golem_api_grpc::proto::golem::rib::dynamic_parsed_function_reference::FunctionReference as ProtoDynamicFunctionReference;
    use semver::{BuildMetadata, Prerelease};

    impl TryFrom<golem_api_grpc::proto::golem::rib::SemVersion> for SemVer {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::SemVersion,
        ) -> Result<Self, Self::Error> {
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
                } => {
                    golem_api_grpc::proto::golem::rib::parsed_function_site::Site::PackageInterface(
                        golem_api_grpc::proto::golem::rib::PackageInterfaceFunctionSite {
                            namespace,
                            package,
                            interface,
                            version: version.map(|v| v.into()),
                        },
                    )
                }
            };
            golem_api_grpc::proto::golem::rib::ParsedFunctionSite { site: Some(site) }
        }
    }

    impl From<DynamicParsedFunctionReference>
        for golem_api_grpc::proto::golem::rib::DynamicParsedFunctionReference
    {
        fn from(value: DynamicParsedFunctionReference) -> Self {
            let function = match value {
                DynamicParsedFunctionReference::Function { function } => ProtoDynamicFunctionReference::Function(
                    golem_api_grpc::proto::golem::rib::FunctionFunctionReference { function },
                ),
                DynamicParsedFunctionReference::RawResourceConstructor { resource } => ProtoDynamicFunctionReference::RawResourceConstructor(
                    golem_api_grpc::proto::golem::rib::RawResourceConstructorFunctionReference { resource },
                ),
                DynamicParsedFunctionReference::RawResourceMethod { resource, method } => ProtoDynamicFunctionReference::RawResourceMethod(
                    golem_api_grpc::proto::golem::rib::RawResourceMethodFunctionReference { resource, method },
                ),
                DynamicParsedFunctionReference::RawResourceStaticMethod { resource, method } => ProtoDynamicFunctionReference::RawResourceStaticMethod(
                    golem_api_grpc::proto::golem::rib::RawResourceStaticMethodFunctionReference { resource, method },
                ),
                DynamicParsedFunctionReference::RawResourceDrop { resource } => ProtoDynamicFunctionReference::RawResourceDrop(
                    golem_api_grpc::proto::golem::rib::RawResourceDropFunctionReference { resource },
                ),
                DynamicParsedFunctionReference::IndexedResourceConstructor { resource, resource_params } => ProtoDynamicFunctionReference::IndexedResourceConstructor(
                    golem_api_grpc::proto::golem::rib::DynamicIndexedResourceConstructorFunctionReference {
                        resource,
                        resource_params: resource_params.into_iter().map(|x| x.into()).collect(),
                    },
                ),
                DynamicParsedFunctionReference::IndexedResourceMethod { resource, resource_params, method } => ProtoDynamicFunctionReference::IndexedResourceMethod(
                    golem_api_grpc::proto::golem::rib::DynamicIndexedResourceMethodFunctionReference {
                        resource,
                        resource_params: resource_params.into_iter().map(|x| x.into()).collect(),
                        method,
                    },
                ),
                DynamicParsedFunctionReference::IndexedResourceStaticMethod { resource, resource_params, method } => ProtoDynamicFunctionReference::IndexedResourceStaticMethod(
                    golem_api_grpc::proto::golem::rib::DynamicIndexedResourceStaticMethodFunctionReference {
                        resource,
                        resource_params: resource_params.into_iter().map(|x| x.into()).collect(),
                        method,
                    },
                ),
                DynamicParsedFunctionReference::IndexedResourceDrop { resource, resource_params } => ProtoDynamicFunctionReference::IndexedResourceDrop(
                    golem_api_grpc::proto::golem::rib::DynamicIndexedResourceDropFunctionReference {
                        resource,
                        resource_params: resource_params.into_iter().map(|x| x.into()).collect(),
                    },
                ),
            };

            golem_api_grpc::proto::golem::rib::DynamicParsedFunctionReference {
                function_reference: Some(function),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::rib::DynamicParsedFunctionReference>
        for DynamicParsedFunctionReference
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::DynamicParsedFunctionReference,
        ) -> Result<Self, Self::Error> {
            let function = value
                .function_reference
                .ok_or("Missing function reference".to_string())?;

            match function {
                ProtoDynamicFunctionReference::Function(golem_api_grpc::proto::golem::rib::FunctionFunctionReference {
                                                            function
                                                        }) => {
                    Ok(Self::Function { function })
                }
                ProtoDynamicFunctionReference::RawResourceConstructor(golem_api_grpc::proto::golem::rib::RawResourceConstructorFunctionReference {
                                                                          resource
                                                                      }) => {
                    Ok(Self::RawResourceConstructor { resource })
                }
                ProtoDynamicFunctionReference::RawResourceMethod(golem_api_grpc::proto::golem::rib::RawResourceMethodFunctionReference {
                                                                     resource,
                                                                     method
                                                                 }) => {
                    Ok(Self::RawResourceMethod { resource, method })
                }
                ProtoDynamicFunctionReference::RawResourceStaticMethod(golem_api_grpc::proto::golem::rib::RawResourceStaticMethodFunctionReference {
                                                                           resource,
                                                                           method
                                                                       }) => {
                    Ok(Self::RawResourceStaticMethod { resource, method })
                }
                ProtoDynamicFunctionReference::RawResourceDrop(golem_api_grpc::proto::golem::rib::RawResourceDropFunctionReference {
                                                                   resource
                                                               }) => {
                    Ok(Self::RawResourceDrop { resource })
                }
                ProtoDynamicFunctionReference::IndexedResourceConstructor(golem_api_grpc::proto::golem::rib::DynamicIndexedResourceConstructorFunctionReference {
                                                                              resource,
                                                                              resource_params
                                                                          }) => {
                    let resource_params: Vec<Expr> =
                        resource_params.into_iter().map(Expr::try_from).collect::<Result<Vec<Expr>, String>>()?;

                    Ok(Self::IndexedResourceConstructor { resource, resource_params })
                }
                ProtoDynamicFunctionReference::IndexedResourceMethod(golem_api_grpc::proto::golem::rib::DynamicIndexedResourceMethodFunctionReference {
                                                                         resource,
                                                                         resource_params,
                                                                         method
                                                                     }) => {
                    let resource_params: Vec<Expr> =
                        resource_params.into_iter().map(Expr::try_from).collect::<Result<Vec<Expr>, String>>()?;

                    Ok(Self::IndexedResourceMethod { resource, resource_params, method })
                }
                ProtoDynamicFunctionReference::IndexedResourceStaticMethod(golem_api_grpc::proto::golem::rib::DynamicIndexedResourceStaticMethodFunctionReference {
                                                                               resource,
                                                                               resource_params,
                                                                               method
                                                                           }) => {
                    let resource_params: Vec<Expr> =
                        resource_params.into_iter().map(Expr::try_from).collect::<Result<Vec<Expr>, String>>()?;

                    Ok(Self::IndexedResourceStaticMethod { resource, resource_params, method })
                }
                ProtoDynamicFunctionReference::IndexedResourceDrop(golem_api_grpc::proto::golem::rib::DynamicIndexedResourceDropFunctionReference {
                                                                       resource,
                                                                       resource_params
                                                                   }) => {
                    let resource_params: Vec<Expr> =
                        resource_params.into_iter().map(Expr::try_from).collect::<Result<Vec<Expr>, String>>()?;

                    Ok(Self::IndexedResourceDrop { resource, resource_params })
                }
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

    impl TryFrom<golem_api_grpc::proto::golem::rib::DynamicParsedFunctionName>
        for DynamicParsedFunctionName
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::DynamicParsedFunctionName,
        ) -> Result<Self, Self::Error> {
            let site = ParsedFunctionSite::try_from(value.site.ok_or("Missing site".to_string())?)?;
            let function = DynamicParsedFunctionReference::try_from(
                value.function.ok_or("Missing function".to_string())?,
            )?;
            Ok(Self { site, function })
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

    impl From<DynamicParsedFunctionName>
        for golem_api_grpc::proto::golem::rib::DynamicParsedFunctionName
    {
        fn from(value: DynamicParsedFunctionName) -> Self {
            golem_api_grpc::proto::golem::rib::DynamicParsedFunctionName {
                site: Some(value.site.into()),
                function: Some(value.function.into()),
            }
        }
    }
}

#[cfg(test)]
mod function_name_tests {
    use super::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer};
    use golem_wasm_ast::analysis::analysed_type::{field, record, u64};
    use golem_wasm_rpc::Value;
    use test_r::test;

    #[test]
    fn parse_function_name_does_not_accept_partial_matches() {
        let result = ParsedFunctionName::parse("x:y/z");
        assert!(result.is_err());
    }

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
                "{field-a: some(1)}".to_string(),
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
                        "{field-a: some(1)}".to_string(),
                    ],
                },
            },
        );
    }

    #[test]
    fn parse_function_name_indexed_method() {
        let parsed = ParsedFunctionName::parse(
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).something}",
        )
        .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[method]resource1.something".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(
            parsed.function().raw_resource_params(),
            Some(&vec![
                "\"hello\"".to_string(),
                "{field-a: some(1)}".to_string(),
            ])
        );
        assert_eq!(
            parsed.function().resource_method_name(),
            Some("something".to_string())
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
                function: ParsedFunctionReference::IndexedResourceMethod {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "{field-a: some(1)}".to_string(),
                    ],
                    method: "something".to_string(),
                },
            },
        );
    }

    #[test]
    fn parse_function_name_indexed_static_method() {
        let parsed = ParsedFunctionName::parse(
            "ns:name/interface.{[static]resource1(\"hello\", { field-a: some(1) }).something}",
        )
        .expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interface".to_string())
        );
        assert_eq!(
            parsed.function().function_name(),
            "[static]resource1.something".to_string()
        );
        assert!(parsed.function().is_indexed_resource());
        assert_eq!(
            parsed.function().raw_resource_params(),
            Some(&vec![
                "\"hello\"".to_string(),
                "{field-a: some(1)}".to_string(),
            ])
        );
        assert_eq!(
            parsed.function().resource_method_name(),
            Some("something".to_string())
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
                function: ParsedFunctionReference::IndexedResourceStaticMethod {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "{field-a: some(1)}".to_string(),
                    ],
                    method: "something".to_string(),
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
                "{field-a: some(1)}".to_string(),
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
                        "{field-a: some(1)}".to_string(),
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
        let parsed = ParsedFunctionName::parse(input)
            .unwrap_or_else(|_| panic!("Input Parsing failed for {input}"));
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

    #[test]
    fn test_parsed_function_name_complex_resource_args() {
        round_trip_function_name_parse(
            r#"golem:api/oplog-processor@1.1.0-rc1.{processor({ account-id: { value: "-1" } }, { high-bits: 11637111831105389641, low-bits: 11277240687824975272 }, []).process}"#,
        )
    }

    #[test]
    fn test_parsed_function_name_complex_resource_args_large_nums() {
        let parsed = ParsedFunctionName::parse(r#"golem:api/oplog-processor@1.1.0-rc1.{processor({ high-bits: 18389549593665948372, low-bits: 12287617583649128209 }).process}"#).expect("Input Parsing failed");
        let args = parsed
            .function
            .resource_params(&[record(vec![
                field("high-bits", u64()),
                field("low-bits", u64()),
            ])])
            .expect("Resource params parsing failed")
            .expect("Resource params not found");
        let nums = if let Value::Record(nums) = &args[0].value {
            nums.clone()
        } else {
            panic!("Expected record")
        };

        assert_eq!(
            nums,
            vec![
                Value::U64(18389549593665948372u64),
                Value::U64(12287617583649128209u64),
            ]
        )
    }
}
