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

use combine::parser::char::string;
use combine::{choice, Parser};

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

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ParsedFunctionReference {
    Function { function: String },
    RawResourceConstructor { resource: String },
    RawResourceDrop { resource: String },
    RawResourceMethod { resource: String, method: String },
    RawResourceStaticMethod { resource: String, method: String },
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

    pub fn method_as_static(&self) -> Option<ParsedFunctionReference> {
        match self {
            Self::RawResourceMethod { resource, method } => Some(Self::RawResourceStaticMethod {
                resource: resource.clone(),
                method: method.clone(),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParsedFunctionName {
    site: ParsedFunctionSite,
    function: ParsedFunctionReference,
}

impl ParsedFunctionName {
    pub fn parse(name: impl AsRef<str>) -> Result<Self, String> {
        let name = name.as_ref();

        use combine::parser::char::alpha_num;
        use combine::stream::easy;
        use combine::{attempt, many1, none_of, optional, token, EasyParser, Parser};

        let identifier = || many1(alpha_num().or(token('-'))).map(|string: String| string);
        let namespace = many1(identifier()).message("namespace");
        let package = many1(identifier()).message("package");
        let ns_pkg = (namespace, token(':'), package).map(|(ns, _, pkg)| (ns, pkg));
        let interface = many1(identifier()).message("interface");

        let version = attempt(token('@'))
            .with(many1(none_of(vec!['{'])))
            .and_then(|v: String| semver::Version::parse(&v))
            .message("version");

        let single_function =
            identifier().map(|id| ParsedFunctionReference::Function { function: id });
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
        .or(identifier().map(|id| ParsedFunctionName {
            site: ParsedFunctionSite::Global,
            function: ParsedFunctionReference::Function { function: id },
        }));

        let result: Result<(ParsedFunctionName, &str), easy::ParseError<&str>> =
            parser.easy_parse(name);
        match result {
            Ok((parsed, _)) => Ok(parsed),
            Err(error) => {
                let error_message = error
                    .map_position(|p| p.translate_position(&name))
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
            Some("ns:name/interface".to_string())
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
