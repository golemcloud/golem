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

use crate::fuzzy::{Error, FuzzySearch, Match};
use crate::model::component::show_exported_functions;
use golem_wasm_ast::analysis::AnalysedExport;
use rib::{ParsedFunctionName, ParsedFunctionReference};

pub fn fuzzy_match_function_name(
    provided_function_name: &str,
    exports: &[AnalysedExport],
) -> crate::fuzzy::Result {
    let component_function_names =
        duplicate_names_with_syntax_sugar(show_exported_functions(exports, false));

    // First see if the function name is a valid function name
    let (normalized_function_name, parsed_function_name) =
        match ParsedFunctionName::parse(provided_function_name) {
            Ok(parsed_function_name) => {
                // If it is we render the normalized function name for fuzzy-searching that

                let normalized_function_name = normalize_function_name(&parsed_function_name);
                (
                    normalized_function_name.to_string(),
                    Some(parsed_function_name),
                )
            }
            Err(_) => {
                // If it is not, we try to fuzzy-match the original one
                (provided_function_name.to_string(), None)
            }
        };

    let fuzzy_search = FuzzySearch::new(component_function_names.iter().map(|s| s.as_str()));
    match fuzzy_search.find(&normalized_function_name) {
        Ok(matched) => {
            match parsed_function_name {
                // if we have a match, but it is non-exact _or_ we have a parsed function name, we customize the parsed function name and render it
                // because it may have resource parameters
                Some(mut parsed_function_name)
                    if !matched.exact_match
                        || parsed_function_name.function.is_indexed_resource() =>
                {
                    let parsed_matched_function_name = ParsedFunctionName::parse(&matched.option)
                        .expect("The rendered component export names should always be parseable");

                    adjust_parsed_function_name(
                        &mut parsed_function_name,
                        parsed_matched_function_name,
                        normalized_function_name,
                    )?;

                    let rendered_altered_function_name = parsed_function_name.to_string();
                    Ok(Match {
                        option: rendered_altered_function_name,
                        pattern: matched.pattern,
                        exact_match: false,
                    })
                }
                _ => Ok(normalized(matched)),
            }
        }
        Err(Error::Ambiguous {
            pattern,
            highlighted_options,
            raw_options,
        }) => try_disambiguate(pattern, highlighted_options, raw_options).map(normalized),
        Err(err) => {
            // No match, we just return the fuzzy-match result
            Err(err)
        }
    }
}

fn normalized(matched: Match) -> Match {
    let normalized_function_name = normalize_function_name(&static_as_method(
        ParsedFunctionName::parse(matched.option)
            .expect("The rendered component export names should always be parseable"),
    ));

    // This is not strictly necessary (as the server side would accept the syntax-sugared versions too)
    // but it makes the function's output more consistent
    Match {
        option: normalized_function_name.to_string(),
        ..matched
    }
}

fn static_as_method(name: ParsedFunctionName) -> ParsedFunctionName {
    ParsedFunctionName {
        site: name.site,
        function: match name.function {
            ParsedFunctionReference::RawResourceStaticMethod { resource, method } => {
                ParsedFunctionReference::RawResourceMethod { resource, method }
            }
            ParsedFunctionReference::IndexedResourceStaticMethod {
                resource,
                method,
                resource_params,
            } => ParsedFunctionReference::IndexedResourceMethod {
                resource,
                method,
                resource_params,
            },
            other => other,
        },
    }
}

fn try_disambiguate(
    pattern: String,
    highlighted_options: Vec<String>,
    raw_options: Vec<String>,
) -> crate::fuzzy::Result {
    let mut parsed_options = raw_options
        .iter()
        .map(|option| {
            static_as_method(
                ParsedFunctionName::parse(option)
                    .expect("The rendered component export names should always be parseable"),
            )
        })
        .collect::<Vec<_>>();
    parsed_options.dedup();
    if parsed_options.len() == 1 {
        let option = parsed_options[0].to_string();
        let exact_match = pattern == option;
        Ok(Match {
            option,
            pattern,
            exact_match,
        })
    } else {
        Err(Error::Ambiguous {
            pattern,
            highlighted_options,
            raw_options,
        })
    }
}

fn normalize_function_name(parsed_function_name: &ParsedFunctionName) -> ParsedFunctionName {
    let interface_name = parsed_function_name.site.interface_name();
    let raw_function_name = parsed_function_name.function.function_name();
    match interface_name {
        Some(interface_name) => ParsedFunctionName::on_interface(interface_name, raw_function_name),
        None => ParsedFunctionName::global(raw_function_name),
    }
}

fn adjust_parsed_function_name(
    original: &mut ParsedFunctionName,
    matched: ParsedFunctionName,
    matched_pattern: String,
) -> Result<(), Error> {
    original.site = matched.site;
    match &mut original.function {
        ParsedFunctionReference::Function { function } => {
            *function = matched.function.function_name();
        }
        ParsedFunctionReference::RawResourceConstructor { resource }
        | ParsedFunctionReference::IndexedResourceConstructor { resource, .. } => {
            match matched.function {
                ParsedFunctionReference::RawResourceConstructor {
                    resource: parsed_resource,
                }
                | ParsedFunctionReference::IndexedResourceConstructor {
                    resource: parsed_resource,
                    ..
                } => {
                    *resource = parsed_resource;
                }
                _ => {
                    // We were looking for a resource constructor but found something else
                    return Err(crate::fuzzy::Error::NotFound {
                        pattern: matched_pattern,
                    });
                }
            }
        }
        ParsedFunctionReference::RawResourceDrop { resource }
        | ParsedFunctionReference::IndexedResourceDrop { resource, .. } => {
            match matched.function {
                ParsedFunctionReference::RawResourceDrop {
                    resource: parsed_resource,
                }
                | ParsedFunctionReference::IndexedResourceDrop {
                    resource: parsed_resource,
                    ..
                } => {
                    *resource = parsed_resource;
                }
                _ => {
                    // We were looking for a resource drop but found something else
                    return Err(crate::fuzzy::Error::NotFound {
                        pattern: matched_pattern,
                    });
                }
            }
        }
        ParsedFunctionReference::RawResourceMethod { resource, method }
        | ParsedFunctionReference::RawResourceStaticMethod { resource, method }
        | ParsedFunctionReference::IndexedResourceMethod {
            resource, method, ..
        }
        | ParsedFunctionReference::IndexedResourceStaticMethod {
            resource, method, ..
        } => {
            match matched.function {
                ParsedFunctionReference::RawResourceMethod {
                    resource: parsed_resource,
                    method: parsed_method,
                }
                | ParsedFunctionReference::IndexedResourceMethod {
                    resource: parsed_resource,
                    method: parsed_method,
                    ..
                }
                | ParsedFunctionReference::RawResourceStaticMethod {
                    resource: parsed_resource,
                    method: parsed_method,
                }
                | ParsedFunctionReference::IndexedResourceStaticMethod {
                    resource: parsed_resource,
                    method: parsed_method,
                    ..
                } => {
                    *resource = parsed_resource;
                    *method = parsed_method;
                }
                _ => {
                    // We were looking for a resource method but found something else
                    return Err(crate::fuzzy::Error::NotFound {
                        pattern: matched_pattern,
                    });
                }
            }
        }
    }

    Ok(())
}

fn duplicate_names_with_syntax_sugar(names: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();

    for item in names {
        let parsed = ParsedFunctionName::parse(&item)
            .expect("The rendered component export names should always be parseable");
        match parsed.function {
            ParsedFunctionReference::RawResourceConstructor { .. }
            | ParsedFunctionReference::RawResourceDrop { .. }
            | ParsedFunctionReference::RawResourceMethod { .. }
            | ParsedFunctionReference::RawResourceStaticMethod { .. }
            | ParsedFunctionReference::IndexedResourceConstructor { .. }
            | ParsedFunctionReference::IndexedResourceMethod { .. }
            | ParsedFunctionReference::IndexedResourceStaticMethod { .. }
            | ParsedFunctionReference::IndexedResourceDrop { .. } => {
                // the Display instance is using the syntax sugared version, while the original list
                // contains the raw function names
                result.push(item);
                result.push(parsed.to_string());

                if let Some(static_method) = parsed.function.method_as_static() {
                    result.push(
                        ParsedFunctionName {
                            function: static_method,
                            site: parsed.site.clone(),
                        }
                        .to_string(),
                    );
                }
            }
            _ => {
                // If the function is not related to resources, there is no syntax sugar
                result.push(item);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::model::worker::fuzzy_match_function_name;
    use golem_wasm_ast::analysis::analysed_type::{
        case, f32, field, handle, list, record, str, u32, variant,
    };
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance, AnalysedResourceId, AnalysedResourceMode,
    };
    use test_r::test;

    #[test]
    fn test_fuzzy_match_simple_function_names() {
        assert_eq!(
            fuzzy_match_function_name("golem:it/api.{initialize-cart}", &example_exported_global())
                .unwrap()
                .option,
            "golem:it/api.{initialize-cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("initialize-cart", &example_exported_global())
                .unwrap()
                .option,
            "golem:it/api.{initialize-cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("initialize", &example_exported_global())
                .unwrap()
                .option,
            "golem:it/api.{initialize-cart}"
        );
    }

    #[test]
    fn test_fuzzy_match_constructor() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[constructor]cart}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{[constructor]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("golem:it/api.{cart.new}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[constructor]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("[constructor]cart", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[constructor]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.new", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[constructor]cart}"
        );
    }

    #[test]
    fn test_fuzzy_match_drop() {
        assert_eq!(
            fuzzy_match_function_name("golem:it/api.{[drop]cart}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("golem:it/api.{cart.drop}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.[drop]cart", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.cart.drop", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("[drop]cart", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.drop", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[drop]cart}"
        );
    }

    #[test]
    fn test_fuzzy_match_method() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[method]cart.add-item}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("golem:it/api.{cart.add-item}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.[method]cart.add-item", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.cart.add-item", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("[method]cart.add-item", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.add-item", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.add", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.add-item}"
        );
    }

    #[test]
    fn test_fuzzy_match_static_method() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[method]cart.merge-with}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{[static]cart.merge-with}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart.merge-with}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.[method]cart.merge-with", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.cart.merge-with", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("[method]cart.merge-with", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("[static]cart.merge-with", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.merge-with", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
        assert_eq!(
            fuzzy_match_function_name("cart.merge", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{[method]cart.merge-with}"
        );
    }

    #[test]
    fn test_fuzzy_match_indexed_constructor() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart(\"owner\").new}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{cart(\"owner\").new}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.{cart(\"owner\").new}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{cart(\"owner\").new}"
        );

        // Indexed resource syntax cannot be fuzzy-matched without being a somewhat parseable function name
        assert!(
            fuzzy_match_function_name("cart(\"owner\").new", &example_exported_resource()).is_err()
        );
    }

    #[test]
    fn test_fuzzy_match_indexed_drop() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart(\"owner\").drop}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{cart(\"owner\").drop}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.{cart(\"owner\").drop}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{cart(\"owner\").drop}"
        );

        // Indexed resource syntax cannot be fuzzy-matched without being a somewhat parseable function name
        assert!(
            fuzzy_match_function_name("cart(\"owner\").drop", &example_exported_resource())
                .is_err()
        );
    }

    #[test]
    fn test_fuzzy_match_indexed_method() {
        assert_eq!(
            fuzzy_match_function_name(
                "golem:it/api.{cart(\"owner\").add-item}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{cart(\"owner\").add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name(
                "api.{cart(\"owner\").add-item}",
                &example_exported_resource()
            )
            .unwrap()
            .option,
            "golem:it/api.{cart(\"owner\").add-item}"
        );
        assert_eq!(
            fuzzy_match_function_name("api.{cart(\"owner\").add}", &example_exported_resource())
                .unwrap()
                .option,
            "golem:it/api.{cart(\"owner\").add-item}"
        );

        // Indexed resource syntax cannot be fuzzy-matched without being a somewhat parseable function name
        assert!(fuzzy_match_function_name(
            "cart(\"owner\").add-item",
            &example_exported_resource()
        )
        .is_err());
    }

    fn example_exported_global() -> Vec<AnalysedExport> {
        vec![AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "initialize-cart".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "user-id".to_string(),
                        typ: str(),
                    }],
                    result: None,
                },
                AnalysedFunction {
                    name: "add-item".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "item".to_string(),
                        typ: record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ]),
                    }],
                    result: None,
                },
                AnalysedFunction {
                    name: "remove-item".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "product-id".to_string(),
                        typ: str(),
                    }],
                    result: None,
                },
                AnalysedFunction {
                    name: "update-item-quantity".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "product-id".to_string(),
                            typ: str(),
                        },
                        AnalysedFunctionParameter {
                            name: "quantity".to_string(),
                            typ: u32(),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "checkout".to_string(),
                    parameters: vec![],
                    result: Some(AnalysedFunctionResult {
                        typ: variant(vec![
                            case("error", str()),
                            case("success", record(vec![field("order-id", str())])),
                        ]),
                    }),
                },
                AnalysedFunction {
                    name: "get-cart-contents".to_string(),
                    parameters: vec![],
                    result: Some(AnalysedFunctionResult {
                        typ: list(record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])),
                    }),
                },
            ],
        })]
    }

    fn example_exported_resource() -> Vec<AnalysedExport> {
        vec![AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "[constructor]cart".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "user-id".to_string(),
                        typ: str(),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }),
                },
                AnalysedFunction {
                    name: "[method]cart.add-item".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "item".to_string(),
                            typ: record(vec![
                                field("product-id", str()),
                                field("name", str()),
                                field("price", f32()),
                                field("quantity", u32()),
                            ]),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[method]cart.remove-item".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "product-id".to_string(),
                            typ: str(),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[method]cart.update-item-quantity".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "product-id".to_string(),
                            typ: str(),
                        },
                        AnalysedFunctionParameter {
                            name: "quantity".to_string(),
                            typ: u32(),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[method]cart.checkout".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: variant(vec![
                            case("error", str()),
                            case("success", record(vec![field("order-id", str())])),
                        ]),
                    }),
                },
                AnalysedFunction {
                    name: "[method]cart.get-cart-contents".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                    }],
                    result: Some(AnalysedFunctionResult {
                        typ: list(record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ])),
                    }),
                },
                AnalysedFunction {
                    name: "[method]cart.merge-with".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "other-cart".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                    ],
                    result: None,
                },
                AnalysedFunction {
                    name: "[drop]cart".to_string(),
                    parameters: vec![AnalysedFunctionParameter {
                        name: "self".to_string(),
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }],
                    result: None,
                },
            ],
        })]
    }
}
