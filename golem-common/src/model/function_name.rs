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

use combine::error::Commit;
use combine::parser::char::{alpha_num, string};
use combine::stream::easy;
use combine::{any, attempt, choice, many1, none_of, optional, parser, token, EasyParser, Parser};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{TypeAnnotatedValue, Value};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ParsedFunctionSite {
    Global,
    Interface {
        name: String,
    },
    PackagedInterface {
        namespace: String,
        package: String,
        interface: String,
        version: Option<semver::Version>,
    },
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
            } => Some(format!("{namespace}:{package}/{interface}@{version}")),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
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

#[derive(Debug, PartialEq, Clone)]
pub struct ParsedFunctionName {
    site: ParsedFunctionSite,
    function: ParsedFunctionReference,
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

        let identifier = || many1(alpha_num().or(token('-'))).map(|string: String| string);
        let namespace = many1(identifier()).message("namespace");
        let package = many1(identifier()).message("package");
        let ns_pkg = (namespace, token(':'), package).map(|(ns, _, pkg)| (ns, pkg));
        let interface = many1(identifier()).message("interface");

        let capture_resource_params = || {
            parser(|input| {
                let _: &mut easy::Stream<&str> = input;
                let mut nesting = 1;
                let mut current_param = String::new();
                let mut result = Vec::new();
                let mut result_committed: Option<Commit<()>> = None;
                while nesting > 0 {
                    let (next_char, committed) = any().parse_stream(input).into_result()?;
                    if next_char == ')' {
                        nesting -= 1;
                        if nesting > 0 {
                            current_param.push(next_char);
                        }
                    } else if next_char == '(' {
                        nesting += 1;
                        current_param.push(next_char);
                    } else if next_char == ',' && nesting == 1 {
                        result.push(current_param.trim().to_string());
                        current_param.clear();
                    } else {
                        current_param.push(next_char);
                    }

                    result_committed = match result_committed {
                        Some(c) => Some(c.merge(committed)),
                        None => Some(committed),
                    };
                }

                if !current_param.is_empty() {
                    result.push(current_param.trim().to_string());
                }

                Ok((result, result_committed.unwrap()))
            })
        };

        let version = attempt(token('@'))
            .with(many1(none_of(vec!['{'])))
            .and_then(|v: String| semver::Version::parse(&v))
            .message("version");

        let single_function =
            identifier().map(|id| ParsedFunctionReference::Function { function: id });

        let indexed_resource_syntax = || (identifier(), token('(').with(capture_resource_params()));
        let indexed_constructor_syntax = (indexed_resource_syntax(), token('.'), string("new"))
            .map(|((resource, resource_params), _, _)| {
                ParsedFunctionReference::IndexedResourceConstructor {
                    resource,
                    resource_params,
                }
            });
        let indexed_drop_syntax = (indexed_resource_syntax(), token('.'), string("drop")).map(
            |((resource, resource_params), _, _)| ParsedFunctionReference::IndexedResourceDrop {
                resource,
                resource_params,
            },
        );
        let indexed_method_syntax = (indexed_resource_syntax(), token('.'), identifier()).map(
            |((resource, resource_params), _, method)| {
                ParsedFunctionReference::IndexedResourceMethod {
                    resource,
                    resource_params,
                    method,
                }
            },
        );

        let raw_constructor_syntax = (identifier(), token('.'), string("new"))
            .map(|(resource, _, _)| ParsedFunctionReference::RawResourceConstructor { resource })
            .or((string("[constructor]"), identifier())
                .map(|(_, resource)| ParsedFunctionReference::RawResourceConstructor { resource }));
        let raw_drop_syntax = (identifier(), token('.'), string("drop"))
            .map(|(resource, _, _)| ParsedFunctionReference::RawResourceDrop { resource })
            .or((string("[drop]"), identifier())
                .map(|(_, resource)| ParsedFunctionReference::RawResourceDrop { resource }));
        let raw_method_syntax = (identifier(), token('.'), identifier())
            .map(
                |(resource, _, method)| ParsedFunctionReference::RawResourceMethod {
                    resource,
                    method,
                },
            )
            .or(
                (string("[method]"), identifier(), token('.'), identifier()).map(
                    |(_, resource, _, method)| ParsedFunctionReference::RawResourceMethod {
                        resource,
                        method,
                    },
                ),
            );
        let raw_static_method_syntax = (string("[static]"), identifier(), token('.'), identifier())
            .map(
                |(_, resource, _, method)| ParsedFunctionReference::RawResourceStaticMethod {
                    resource,
                    method,
                },
            );

        let function = choice((
            attempt(indexed_constructor_syntax),
            attempt(indexed_drop_syntax),
            attempt(indexed_method_syntax),
            attempt(raw_constructor_syntax),
            attempt(raw_drop_syntax),
            attempt(raw_method_syntax),
            attempt(raw_static_method_syntax),
            attempt(single_function),
        ));

        let mut parser = attempt(
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
                            version: ver,
                        },
                        None => ParsedFunctionSite::Interface { name: iface },
                    };
                    ParsedFunctionName { site, function }
                }),
        )
        .or(identifier().map(|id| {
            ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: ParsedFunctionReference::Function { function: id },
            }
        }));

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
mod tests {
    use crate::model::function_name::ParsedFunctionName;

    #[test]
    fn parse_function_name_global() {
        let parsed = ParsedFunctionName::parse("run-example").expect("Parsing failed");
        assert_eq!(parsed.site().interface_name(), None);
        assert_eq!(parsed.function().function_name(), "run-example");
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
    }

    #[test]
    fn parse_function_name_in_exported_interface() {
        let parsed = ParsedFunctionName::parse("ns:name/interface.{fn1}").expect("Parsing failed");
        assert_eq!(
            parsed.site().interface_name(),
            Some("ns:name/interace".to_string())
        );
        assert_eq!(parsed.function().function_name(), "fn1".to_string());
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
    }
}
