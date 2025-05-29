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

use crate::expr::Expr;
use crate::function_name::{ParsedFunctionSite, SemVer};
use crate::parser::errors::RibParseError;
use crate::parser::generic_type_parameter::generic_type_parameter;
use crate::parser::rib_expr::rib_expr;
use crate::rib_source_span::GetSourcePosition;
use crate::{DynamicParsedFunctionName, DynamicParsedFunctionReference};
use combine::error::{Commit, StreamError};
use combine::parser::char::{alpha_num, string};
use combine::parser::char::{char, spaces};
use combine::parser::repeat::take_until;
use combine::{any, attempt, between, choice, many1, optional, parser, token, ParseError, Parser};
use combine::{sep_by, ParseResult, Positioned};

// A call can be a function or constructing an anonymous variant at the type of writing Rib which user expects to work at runtime
pub fn call<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (
        function_name().skip(spaces()),
        optional(between(
            char('[').skip(spaces()),
            char(']').skip(spaces()),
            generic_type_parameter().skip(spaces()),
        )),
        between(
            char('(').skip(spaces()),
            char(')').skip(spaces()),
            sep_by(rib_expr(), char(',').skip(spaces())),
        ),
    )
        .map(|(name, tp, args)| Expr::call_worker_function(name, tp, None, args))
}

pub fn function_name<Input>() -> impl Parser<Input, Output = DynamicParsedFunctionName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    let identifier = || many1(alpha_num().or(token('-'))).map(|string: String| string);
    let namespace = many1(identifier());
    let package = many1(identifier());
    let ns_pkg = (namespace, token(':'), package).map(|(ns, _, pkg)| (ns, pkg));
    let interface = many1(identifier());

    let capture_resource_params = || {
        parser(|input| {
            let _: &mut Input = input;
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
                } else if next_char == '(' || next_char == '{' || next_char == '[' {
                    nesting += 1;
                    current_param.push(next_char);
                } else if next_char == '}' || next_char == ']' {
                    nesting -= 1;
                    current_param.push(next_char);
                } else if next_char == ',' && nesting == 1 {
                    let expr = Expr::from_text(current_param.trim());
                    match expr {
                        Ok(expr) => {
                            result.push(expr);
                            current_param.clear();
                        }
                        Err(err) => {
                            return ParseResult::CommitErr(ParseError::from_error(
                                input.position(),
                                StreamError::message_format(format!(
                                    "Failed to parse resource parameter {current_param}: {err}"
                                )),
                            ))
                            .into_result();
                        }
                    }
                } else {
                    current_param.push(next_char);
                }

                result_committed = match result_committed {
                    Some(c) => Some(c.merge(committed)),
                    None => Some(committed),
                };
            }

            if !current_param.is_empty() {
                let expr = Expr::from_text(current_param.trim());
                match expr {
                    Ok(expr) => {
                        result.push(expr);
                    }
                    Err(err) => {
                        return ParseResult::CommitErr(ParseError::from_error(
                            input.position(),
                            StreamError::message_format(format!(
                                "Failed to parse resource parameter {}",
                                err
                            )),
                        ))
                        .into_result();
                    }
                }
            }

            Ok((result, result_committed.unwrap()))
        })
    };

    let version = attempt(token('@'))
        .with(take_until(attempt(string(".{"))))
        .and_then(|v: String| {
            let stripped = v.strip_suffix('.').unwrap_or(&v);
            match semver::Version::parse(stripped) {
                Ok(version) => Ok(version),
                Err(_) => Err(RibParseError::Message("Invalid version".to_string()).into()),
            }
        })
        .message("version");

    let single_function =
        identifier().map(|id| DynamicParsedFunctionReference::Function { function: id });

    let indexed_resource_syntax = || (identifier(), token('(').with(capture_resource_params()));
    let indexed_constructor_syntax = (indexed_resource_syntax(), token('.'), string("new")).map(
        |((resource, resource_params), _, _)| {
            DynamicParsedFunctionReference::IndexedResourceConstructor {
                resource,
                resource_params,
            }
        },
    );
    let indexed_drop_syntax = (indexed_resource_syntax(), token('.'), string("drop")).map(
        |((resource, resource_params), _, _)| DynamicParsedFunctionReference::IndexedResourceDrop {
            resource,
            resource_params,
        },
    );
    let indexed_method_syntax = (indexed_resource_syntax(), token('.'), identifier()).map(
        |((resource, resource_params), _, method)| {
            DynamicParsedFunctionReference::IndexedResourceMethod {
                resource,
                resource_params,
                method,
            }
        },
    );
    let indexed_static_method_syntax = (
        string("[static]"),
        indexed_resource_syntax(),
        token('.'),
        identifier(),
    )
        .map(|(_, (resource, resource_params), _, method)| {
            DynamicParsedFunctionReference::IndexedResourceStaticMethod {
                resource,
                resource_params,
                method,
            }
        });

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
        attempt(indexed_constructor_syntax),
        attempt(indexed_drop_syntax),
        attempt(indexed_method_syntax),
        attempt(indexed_static_method_syntax),
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
#[cfg(test)]
mod function_call_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::{DynamicParsedFunctionName, DynamicParsedFunctionReference};

    use crate::expr::Expr;
    use crate::function_name::{ParsedFunctionSite, SemVer};

    #[test]
    fn test_call() {
        let input = "foo()";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![],
        ));

        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_args() {
        let input = "foo(bar)";
        let result = Expr::from_text(input);

        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![Expr::identifier_global("bar", None)],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_args() {
        let input = "foo(bar, baz)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_args_and_spaces() {
        let input = "foo(bar, baz, qux)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None),
                Expr::identifier_global("qux", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_args_and_spaces_and_commas() {
        let input = "foo(bar, baz, qux, quux)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None),
                Expr::identifier_global("qux", None),
                Expr::identifier_global("quux", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_args_and_spaces_and_commas_and_spaces() {
        let input = "foo(bar, baz, qux, quux, quuz)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None),
                Expr::identifier_global("qux", None),
                Expr::identifier_global("quux", None),
                Expr::identifier_global("quuz", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_args_and_spaces_and_commas_and_spaces_and_commas() {
        let input = "foo(bar, baz, qux, quux, quuz, quuux)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None),
                Expr::identifier_global("qux", None),
                Expr::identifier_global("quux", None),
                Expr::identifier_global("quuz", None),
                Expr::identifier_global("quuux", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_record() {
        let input = "foo({bar: baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![Expr::record(vec![(
                "bar".to_string(),
                Expr::identifier_global("baz", None),
            )])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_record_and_multiple_args() {
        let input = "foo({bar: baz}, qux)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::record(vec![(
                    "bar".to_string(),
                    Expr::identifier_global("baz", None),
                )]),
                Expr::identifier_global("qux", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_records() {
        let input = "foo({bar: baz}, {qux: quux})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::record(vec![(
                    "bar".to_string(),
                    Expr::identifier_global("baz", None),
                )]),
                Expr::record(vec![(
                    "qux".to_string(),
                    Expr::identifier_global("quux", None),
                )]),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_records_and_args() {
        let input = "foo({bar: baz}, {qux: quux}, quuz)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::record(vec![(
                    "bar".to_string(),
                    Expr::identifier_global("baz", None),
                )]),
                Expr::record(vec![(
                    "qux".to_string(),
                    Expr::identifier_global("quux", None),
                )]),
                Expr::identifier_global("quuz", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_sequence() {
        let input = "foo([bar, baz])";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![Expr::sequence(
                vec![
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None),
                ],
                None,
            )],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_sequence_and_args() {
        let input = "foo([bar, baz], qux)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::sequence(
                    vec![
                        Expr::identifier_global("bar", None),
                        Expr::identifier_global("baz", None),
                    ],
                    None,
                ),
                Expr::identifier_global("qux", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_multiple_sequences() {
        let input = "foo([bar, baz], [qux, quux])";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::sequence(
                    vec![
                        Expr::identifier_global("bar", None),
                        Expr::identifier_global("baz", None),
                    ],
                    None,
                ),
                Expr::sequence(
                    vec![
                        Expr::identifier_global("qux", None),
                        Expr::identifier_global("quux", None),
                    ],
                    None,
                ),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_tuples() {
        let input = "foo((bar, baz))";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![Expr::tuple(vec![
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None),
            ])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_tuples_and_args() {
        let input = "foo((bar, baz), qux)";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![
                Expr::tuple(vec![
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None),
                ]),
                Expr::identifier_global("qux", None),
            ],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_flags() {
        let input = "foo({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_interface_names() {
        let input = "interface.{fn1}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Interface {
                    name: "interface".to_string(),
                },
                function: DynamicParsedFunctionReference::Function {
                    function: "fn1".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_exported_interface() {
        let input = "ns:name/interface.{fn1}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::Function {
                    function: "fn1".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_versioned_exported_interface() {
        let input = "wasi:cli/run@0.2.0.{run}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "wasi".to_string(),
                    package: "cli".to_string(),
                    interface: "run".to_string(),
                    version: Some(SemVer(semver::Version::new(0, 2, 0))),
                },
                function: DynamicParsedFunctionReference::Function {
                    function: "run".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_constructor_syntax_sugar() {
        let input = "ns:name/interface.{resource1.new}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceConstructor {
                    resource: "resource1".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_constructor() {
        let input = "ns:name/interface.{[constructor]resource1}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceConstructor {
                    resource: "resource1".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_indexed_constructor1() {
        let input = "ns:name/interface.{resource1().new}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![],
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    // TODO: The resource parameters can be identifiers, but currently function name parser parses all arguments to be just string
    #[test]
    fn test_call_with_function_name_indexed_constructor2() {
        let input = "ns:name/interface.{resource1(\"hello\", 1, true).new}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        Expr::literal("hello"),
                        Expr::number(BigDecimal::from(1)),
                        Expr::boolean(true),
                    ],
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_indexed_constructor3() {
        let input =
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).new}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::IndexedResourceConstructor {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        Expr::literal("hello"),
                        Expr::record(vec![(
                            "field-a".to_string(),
                            Expr::option(Some(Expr::number(BigDecimal::from(1)))),
                        )]),
                    ],
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_method_syntax_sugar() {
        let input = "ns:name/interface.{resource1.do-something}({bar, baz})";
        let result = Expr::from_text(input).unwrap();
        let expected = Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_method() {
        let input = "ns:name/interface.{[method]resource1.do-something}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    // TODO: Should have been RawResourceStaticMethod
    #[test]
    fn test_call_with_function_name_static_method_syntax_sugar() {
        let input = "ns:name/interface.{resource1.do-something-static}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceMethod {
                    resource: "resource1".to_string(),
                    method: "do-something-static".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_static() {
        let input = "ns:name/interface.{[static]resource1.do-something-static}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceStaticMethod {
                    resource: "resource1".to_string(),
                    method: "do-something-static".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_drop_syntax_sugar() {
        let input = "ns:name/interface.{resource1.drop}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceDrop {
                    resource: "resource1".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_indexed_drop_1() {
        let input = "ns:name/interface.{resource1().drop}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![],
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_indexed_drop_2() {
        let input = "ns:name/interface.{resource1(\"hello\", 1, true).drop}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        Expr::literal("hello"),
                        Expr::number(BigDecimal::from(1)),
                        Expr::boolean(true),
                    ],
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_indexed_drop_3() {
        let input =
            "ns:name/interface.{resource1(\"hello\", { field-a: some(1) }).drop}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::IndexedResourceDrop {
                    resource: "resource1".to_string(),
                    resource_params: vec![
                        Expr::literal("hello"),
                        Expr::record(vec![(
                            "field-a".to_string(),
                            Expr::option(Some(Expr::number(BigDecimal::from(1)))),
                        )]),
                    ],
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_call_with_function_name_drop() {
        let input = "ns:name/interface.{[drop]resource1}({bar, baz})";
        let result = Expr::from_text(input);
        let expected = Ok(Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface {
                    namespace: "ns".to_string(),
                    package: "name".to_string(),
                    interface: "interface".to_string(),
                    version: None,
                },
                function: DynamicParsedFunctionReference::RawResourceDrop {
                    resource: "resource1".to_string(),
                },
            },
            None,
            None,
            vec![Expr::flags(vec!["bar".to_string(), "baz".to_string()])],
        ));
        assert_eq!(result, expected);
    }
}
