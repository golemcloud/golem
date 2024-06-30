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
use golem_wasm_rpc::{TypeAnnotatedValue, Value};
use semver::{BuildMetadata, Prerelease};
use std::borrow::Cow;

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
                    let type_annotated_value =
                        wasm_wave::from_str::<TypeAnnotatedValue>(param_type, raw_param)
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

#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct ParsedFunctionName {
    pub site: ParsedFunctionSite,
    pub function: ParsedFunctionReference,
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
                }
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
                }
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
                    version: None
                },
                function: ParsedFunctionReference::Function {
                    function: "fn1".to_string()
                }
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
                    version: Some(SemVer(semver::Version::new(0, 2, 0)))
                },
                function: ParsedFunctionReference::Function {
                    function: "run".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceConstructor {
                    resource: "resource1".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceConstructor {
                    resource: "resource1".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![]
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceStaticMethod {
                    resource: "resource1".to_string(),
                    method: "do-something-static".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceStaticMethod {
                    resource: "resource1".to_string(),
                    method: "do-something-static".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceDrop {
                    resource: "resource1".to_string()
                }
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
                    version: None
                },
                function: ParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![]
                }
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
                    version: None
                },
                function: ParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        "\"hello\"".to_string(),
                        "1".to_string(),
                        "true".to_string(),
                    ]
                }
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
                    version: None
                },
                function: ParsedFunctionReference::RawResourceDrop {
                    resource: "resource1".to_string()
                }
            }
        );
    }
}
