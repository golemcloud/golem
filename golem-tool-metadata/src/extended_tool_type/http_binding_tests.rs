// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (http://www.apache.org/licenses/LICENSE-2.0).
use super::*;
use golem_schema::Empty;
use golem_schema::http::{CustomHttpMethod, HeaderVariable, HttpMethod, QueryVariable};
use test_r::test;

#[test]
fn implicit_query_collision_is_rejected_for_custom_and_standard_methods() {
    for method in [
        HttpMethod::Get(Empty {}),
        HttpMethod::Custom(CustomHttpMethod {
            value: "GET".into(),
        }),
        HttpMethod::Head(Empty {}),
        HttpMethod::Custom(CustomHttpMethod {
            value: "HEAD".into(),
        }),
        HttpMethod::Delete(Empty {}),
        HttpMethod::Custom(CustomHttpMethod {
            value: "DELETE".into(),
        }),
    ] {
        let mut tool = report_http_tool();
        let body = tool.commands[1].body.as_mut().unwrap();
        body.positionals.fixed.clear();
        body.options = vec![scalar_opt("first", None), scalar_opt("second", None)];
        tool.commands[1].http = Some(http_endpoint(method, "/generate"));
        let Some(CommandHttp::Endpoint(endpoint)) = &mut tool.commands[1].http else {
            unreachable!()
        };
        endpoint.query_vars.push(QueryVariable {
            query_param_name: "second".into(),
            variable_name: "first".into(),
        });
        assert!(
            validate_tool(&tool).is_err(),
            "two inputs share the second query key"
        );
        let Some(CommandHttp::Endpoint(endpoint)) = &mut tool.commands[1].http else {
            unreachable!()
        };
        endpoint.query_vars[0].query_param_name = "renamed".into();
        validate_tool(&tool).expect("distinct explicit and implicit query keys are valid");
    }
}

#[test]
fn nested_collections_are_not_scalar_http_bindings() {
    for ty in [
        SchemaType::list(SchemaType::list(SchemaType::string())),
        SchemaType::map(SchemaType::string(), SchemaType::list(SchemaType::u32())),
        SchemaType::list(SchemaType::record(vec![])),
    ] {
        for query in [false, true] {
            let mut tool = report_http_tool();
            let body = tool.commands[1].body.as_mut().unwrap();
            body.positionals.fixed.clear();
            body.options = vec![scalar_opt("value", None)];
            body.options[0].shape = ExtendedOptionShape::Scalar(SchemaGraph::anonymous(ty.clone()));
            tool.commands[1].http = Some(http_endpoint(HttpMethod::Post(Empty {}), "/generate"));
            // A JSON-body control must remain valid: only the HTTP source shape is rejected.
            validate_tool(&tool).expect("nested collections are valid JSON-body inputs");
            let Some(CommandHttp::Endpoint(endpoint)) = &mut tool.commands[1].http else {
                unreachable!()
            };
            if query {
                endpoint.query_vars.push(QueryVariable {
                    query_param_name: "v".into(),
                    variable_name: "value".into(),
                });
            } else {
                endpoint.header_vars.push(HeaderVariable {
                    header_name: "X-Value".into(),
                    variable_name: "value".into(),
                });
            }
            assert!(
                validate_tool(&tool).is_err(),
                "nested shape {ty:?}, query={query}"
            );
        }
    }
}

#[test]
fn duplicate_header_names_are_case_insensitive_across_distinct_inputs() {
    let mut tool = report_http_tool();
    let body = tool.commands[1].body.as_mut().unwrap();
    body.positionals.fixed.clear();
    body.options = vec![scalar_opt("first", None), scalar_opt("second", None)];
    let Some(CommandHttp::Endpoint(endpoint)) = &mut tool.commands[1].http else {
        unreachable!()
    };
    endpoint.header_vars = vec![
        HeaderVariable {
            header_name: "X-Value".into(),
            variable_name: "first".into(),
        },
        HeaderVariable {
            header_name: "x-value".into(),
            variable_name: "second".into(),
        },
    ];
    assert!(validate_tool(&tool).is_err());
    let Some(CommandHttp::Endpoint(endpoint)) = &mut tool.commands[1].http else {
        unreachable!()
    };
    endpoint.header_vars[1].header_name = "X-Other".into();
    validate_tool(&tool).expect("distinct header bindings remain valid");
}
