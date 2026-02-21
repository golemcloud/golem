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

use combine::parser::char::{alpha_num, string};
use combine::parser::repeat::take_until;
use combine::stream::position::Stream;
use combine::{attempt, choice, eof, many1, optional, token, EasyParser, ParseError, Parser};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use semver::{BuildMetadata, Prerelease};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

// ---------------------------------------------------------------------------
// Parse error helper
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone)]
pub enum FunctionNameParseError {
    Message(String),
}

impl Display for FunctionNameParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionNameParseError::Message(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for FunctionNameParseError {}

// ---------------------------------------------------------------------------
// Source position helpers (needed as trait bounds for the combine parser)
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct SourcePosition {
    pub line: i32,
    pub column: i32,
}

pub trait GetSourcePosition {
    fn get_source_position(&self) -> SourcePosition;
}

impl GetSourcePosition for combine::stream::position::SourcePosition {
    fn get_source_position(&self) -> SourcePosition {
        SourcePosition {
            line: self.line,
            column: self.column,
        }
    }
}

// ---------------------------------------------------------------------------
// SemVer
// ---------------------------------------------------------------------------

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

impl BinarySerializer for SemVer {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        BinarySerializer::serialize(&self.0.major, context)?;
        BinarySerializer::serialize(&self.0.minor, context)?;
        BinarySerializer::serialize(&self.0.patch, context)?;
        BinarySerializer::serialize(&self.0.pre.as_str(), context)?;
        BinarySerializer::serialize(&self.0.build.as_str(), context)?;
        Ok(())
    }
}

impl BinaryDeserializer for SemVer {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let major = <u64 as BinaryDeserializer>::deserialize(context)?;
        let minor = <u64 as BinaryDeserializer>::deserialize(context)?;
        let patch = <u64 as BinaryDeserializer>::deserialize(context)?;
        let pre_str = <std::string::String as BinaryDeserializer>::deserialize(context)?;
        let build_str = <std::string::String as BinaryDeserializer>::deserialize(context)?;
        let pre = Prerelease::new(&pre_str)
            .map_err(|_| desert_rust::Error::DeserializationFailure("Invalid prerelease".into()))?;
        let build = BuildMetadata::new(&build_str).map_err(|_| {
            desert_rust::Error::DeserializationFailure("Invalid build metadata".into())
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

// ---------------------------------------------------------------------------
// ParsedFunctionSite
// ---------------------------------------------------------------------------

#[derive(Debug, Hash, PartialEq, Eq, Clone, BinaryCodec, Ord, PartialOrd)]
#[desert(evolution())]
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

// ---------------------------------------------------------------------------
// DynamicParsedFunctionReference (parser-internal intermediate)
// ---------------------------------------------------------------------------

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd, BinaryCodec)]
#[desert(evolution())]
pub enum DynamicParsedFunctionReference {
    Function { function: String },
    RawResourceConstructor { resource: String },
    RawResourceDrop { resource: String },
    RawResourceMethod { resource: String, method: String },
    RawResourceStaticMethod { resource: String, method: String },
}

impl DynamicParsedFunctionReference {
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
        }
    }
}

// ---------------------------------------------------------------------------
// ParsedFunctionReference
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone, Hash, BinaryCodec)]
#[desert(evolution())]
pub enum ParsedFunctionReference {
    Function { function: String },
    RawResourceConstructor { resource: String },
    RawResourceDrop { resource: String },
    RawResourceMethod { resource: String, method: String },
    RawResourceStaticMethod { resource: String, method: String },
}

impl Display for ParsedFunctionReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let function_name = match self {
            Self::Function { function } => function.clone(),
            Self::RawResourceConstructor { resource } => format!("{resource}.new"),
            Self::RawResourceMethod { resource, method } => format!("{resource}.{method}"),
            Self::RawResourceStaticMethod { resource, method } => {
                format!("[static]{resource}.{method}")
            }
            Self::RawResourceDrop { resource } => format!("{resource}.drop"),
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
        }
    }

    pub fn resource_method_name(&self) -> Option<String> {
        match self {
            Self::RawResourceMethod { method, .. }
            | Self::RawResourceStaticMethod { method, .. } => Some(method.clone()),
            _ => None,
        }
    }

    pub fn method_as_static(&self) -> Option<ParsedFunctionReference> {
        match self {
            Self::RawResourceMethod { resource, method } => Some(Self::RawResourceStaticMethod {
                resource: resource.clone(),
                method: method.clone(),
            }),

            _ => None,
        }
    }

    pub fn resource_name(&self) -> Option<&String> {
        match self {
            Self::RawResourceConstructor { resource }
            | Self::RawResourceDrop { resource }
            | Self::RawResourceMethod { resource, .. }
            | Self::RawResourceStaticMethod { resource, .. } => Some(resource),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DynamicParsedFunctionName (parser-internal intermediate)
// ---------------------------------------------------------------------------

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd, BinaryCodec)]
#[desert(evolution())]
pub struct DynamicParsedFunctionName {
    pub site: ParsedFunctionSite,
    pub function: DynamicParsedFunctionReference,
}

impl DynamicParsedFunctionName {
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

// ---------------------------------------------------------------------------
// ParsedFunctionName
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Clone, Hash, BinaryCodec)]
#[desert(evolution())]
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
        let function_name = <String as Deserialize>::deserialize(deserializer)?;
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

        let mut parser = parse_function_name().skip(eof());

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
            ParsedFunctionReference::RawResourceConstructor { resource, .. } => Some(resource),
            _ => None,
        }
    }

    pub fn is_method(&self) -> Option<&str> {
        match &self.function {
            ParsedFunctionReference::RawResourceMethod { resource, .. }
            | ParsedFunctionReference::RawResourceStaticMethod { resource, .. } => Some(resource),
            _ => None,
        }
    }

    pub fn is_static_method(&self) -> Option<&str> {
        match &self.function {
            ParsedFunctionReference::RawResourceStaticMethod { resource, .. } => Some(resource),
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

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn parse_function_name<Input>() -> impl Parser<Input, Output = DynamicParsedFunctionName>
where
    Input: combine::Stream<Token = char>,
    FunctionNameParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    let identifier = || many1(alpha_num().or(token('-'))).map(|string: String| string);
    let namespace = many1(identifier());
    let package = many1(identifier());
    let ns_pkg = (namespace, token(':'), package).map(|(ns, _, pkg)| (ns, pkg));
    let interface = many1(identifier());

    let version = attempt(token('@'))
        .with(take_until(attempt(string(".{"))))
        .and_then(|v: String| {
            let stripped = v.strip_suffix('.').unwrap_or(&v);
            match semver::Version::parse(stripped) {
                Ok(version) => Ok(version),
                Err(_) => {
                    Err(FunctionNameParseError::Message("Invalid version".to_string()).into())
                }
            }
        })
        .message("version");

    let single_function =
        identifier().map(|id| DynamicParsedFunctionReference::Function { function: id });

    let raw_constructor_syntax = (identifier(), token('.'), string("new"))
        .map(|(resource, _, _)| DynamicParsedFunctionReference::RawResourceConstructor { resource })
        .or(
            (string("[constructor]"), identifier()).map(|(_, resource)| {
                DynamicParsedFunctionReference::RawResourceConstructor { resource }
            }),
        );
    let raw_drop_syntax = (identifier(), token('.'), string("drop"))
        .map(|(resource, _, _)| DynamicParsedFunctionReference::RawResourceDrop { resource })
        .or((string("[drop]"), identifier())
            .map(|(_, resource)| DynamicParsedFunctionReference::RawResourceDrop { resource }));
    let raw_method_syntax = (identifier(), token('.'), identifier())
        .map(
            |(resource, _, method)| DynamicParsedFunctionReference::RawResourceMethod {
                resource,
                method,
            },
        )
        .or(
            (string("[method]"), identifier(), token('.'), identifier()).map(
                |(_, resource, _, method)| DynamicParsedFunctionReference::RawResourceMethod {
                    resource,
                    method,
                },
            ),
        );
    let raw_static_method_syntax = (string("[static]"), identifier(), token('.'), identifier())
        .map(
            |(_, resource, _, method)| DynamicParsedFunctionReference::RawResourceStaticMethod {
                resource,
                method,
            },
        );

    let function = choice((
        attempt(raw_constructor_syntax),
        attempt(raw_drop_syntax),
        attempt(raw_method_syntax),
        attempt(raw_static_method_syntax),
        attempt(single_function),
    ));

    attempt(
        (
            optional(attempt((ns_pkg, token('/')))),
            interface,
            optional(version),
            token('.'),
            token('{'),
            function,
            token('}'),
        )
            .map(|(nspkg, iface, ver, _, _, function, _)| {
                let site = match nspkg {
                    Some(((ns, pkg), _)) => ParsedFunctionSite::PackagedInterface {
                        namespace: ns,
                        package: pkg,
                        interface: iface,
                        version: ver.map(SemVer),
                    },
                    None => ParsedFunctionSite::Interface { name: iface },
                };
                DynamicParsedFunctionName { site, function }
            }),
    )
    .or(identifier().map(|id| DynamicParsedFunctionName {
        site: ParsedFunctionSite::Global,
        function: DynamicParsedFunctionReference::Function { function: id },
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod function_name_tests {
    use super::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer};
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
        assert_eq!(parsed.function().function_name(), "run-example".to_string());
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
    fn parse_function_name_in_exported_versioned_interface() {
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
        round_trip_function_name_parse("ns:name/interface.{resource1.do-something}");
        round_trip_function_name_parse("ns:name/interface.{[static]resource1.do-something-static}");
        round_trip_function_name_parse("ns:name/interface.{resource1.drop}");
        round_trip_function_name_parse("ns:name/interface.{[drop]resource1}");
    }
}
