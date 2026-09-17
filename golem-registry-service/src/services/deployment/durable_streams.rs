// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_common::model::agent::{HttpEndpointDetails, HttpMountDetails, PathSegment};
use golem_common::schema::agent::contains_stream_in_graph;
use golem_common::schema::{
    AgentMethodSchema, AgentTypeSchema, FieldSource, InputSchema, OutputSchema, SchemaGraph,
    SchemaType,
};
use std::collections::HashSet;

const RESERVED_PATH_NAMES: &[&str] = &["invocations", "streams", "forks"];

/// Validates the static Durable Streams slots and returns the part of the
/// method input which is still bound by the ordinary HTTP parameter compiler.
pub(super) fn validate_route(
    agent: &AgentTypeSchema,
    method: &AgentMethodSchema,
    mount: &HttpMountDetails,
    endpoint: &HttpEndpointDetails,
) -> Result<InputSchema, String> {
    validate_path(mount, endpoint)?;

    let graph = &agent.schema;
    let mut slot_names = HashSet::new();
    let mut non_stream_fields = Vec::new();

    for field in method.input_schema.fields() {
        if !matches!(field.source, FieldSource::UserSupplied) {
            non_stream_fields.push(field.clone());
            continue;
        }

        match resolve(graph, &field.schema)? {
            SchemaType::Stream { inner, .. } => {
                validate_slot_name(&field.name)?;
                insert_slot(&mut slot_names, &field.name)?;
                validate_stream_element(graph, inner.as_deref(), &field.name)?;
                reject_stream_binding(endpoint, &field.name)?;
            }
            _ if contains_stream_in_graph(graph, &field.schema) => {
                return Err(format!(
                    "Durable Streams input '{}' must be a top-level stream parameter",
                    field.name
                ));
            }
            _ => non_stream_fields.push(field.clone()),
        }
    }

    match &method.output_schema {
        OutputSchema::Unit => {}
        OutputSchema::Single(output) => match resolve(graph, output)? {
            SchemaType::Stream { inner, .. } => {
                insert_slot(&mut slot_names, "$result")?;
                validate_stream_element(graph, inner.as_deref(), "$result")?;
            }
            SchemaType::Record { fields, .. } if contains_stream_in_graph(graph, output) => {
                for field in fields {
                    match resolve(graph, &field.body)? {
                        SchemaType::Stream { inner, .. } => {
                            validate_slot_name(&field.name)?;
                            insert_slot(&mut slot_names, &field.name)?;
                            validate_stream_element(graph, inner.as_deref(), &field.name)?;
                        }
                        _ => {
                            return Err(format!(
                                "Durable Streams output record field '{}' must be a direct stream",
                                field.name
                            ));
                        }
                    }
                }
            }
            _ if contains_stream_in_graph(graph, output) => {
                return Err(
                    "Durable Streams output streams must be direct or direct record fields".into(),
                );
            }
            _ => {
                insert_slot(&mut slot_names, "$result")?;
                validate_public_json(graph, output, &mut HashSet::new())?;
            }
        },
    }

    Ok(InputSchema::parameters(non_stream_fields))
}

fn validate_path(mount: &HttpMountDetails, endpoint: &HttpEndpointDetails) -> Result<(), String> {
    for segment in mount.path_prefix.iter().chain(&endpoint.path_suffix) {
        match segment {
            PathSegment::RemainingPathVariable(variable) => {
                return Err(format!(
                    "Durable Streams base path cannot contain greedy variable '{}'",
                    variable.variable_name
                ));
            }
            PathSegment::PathVariable(variable)
                if RESERVED_PATH_NAMES.contains(&variable.variable_name.as_str()) =>
            {
                return Err(format!(
                    "Durable Streams path variable '{}' shadows a reserved path segment",
                    variable.variable_name
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn reject_stream_binding(endpoint: &HttpEndpointDetails, name: &str) -> Result<(), String> {
    let path_bound = endpoint.path_suffix.iter().any(|segment| match segment {
        PathSegment::PathVariable(variable) | PathSegment::RemainingPathVariable(variable) => {
            variable.variable_name == name
        }
        _ => false,
    });
    let query_bound = endpoint.query_vars.iter().any(|v| v.variable_name == name);
    let header_bound = endpoint.header_vars.iter().any(|v| v.variable_name == name);
    if path_bound || query_bound || header_bound {
        Err(format!(
            "Durable Streams parameter '{name}' cannot be bound to path, query or header"
        ))
    } else {
        Ok(())
    }
}

fn validate_slot_name(name: &str) -> Result<(), String> {
    if name == "$result" || RESERVED_PATH_NAMES.contains(&name) || name.starts_with("__ds") {
        return Err(format!("Durable Streams slot name '{name}' is reserved"));
    }
    if name.is_empty()
        || name.len() > 64
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(format!(
            "Durable Streams slot name '{name}' is not URL-segment-safe"
        ));
    }
    Ok(())
}

fn insert_slot(slots: &mut HashSet<String>, name: &str) -> Result<(), String> {
    if slots.insert(name.to_string()) {
        Ok(())
    } else {
        Err(format!("Duplicate Durable Streams slot name '{name}'"))
    }
}

fn validate_stream_element(
    graph: &SchemaGraph,
    element: Option<&SchemaType>,
    slot: &str,
) -> Result<(), String> {
    let element = element.ok_or_else(|| {
        format!("Durable Streams slot '{slot}' has no element type for public JSON")
    })?;
    if contains_stream_in_graph(graph, element) {
        return Err(format!(
            "Durable Streams slot '{slot}' has a nested stream element"
        ));
    }
    validate_public_json(graph, element, &mut HashSet::new())
        .map_err(|error| format!("Durable Streams slot '{slot}': {error}"))
}

fn resolve<'a>(graph: &'a SchemaGraph, ty: &'a SchemaType) -> Result<&'a SchemaType, String> {
    graph.resolve_ref(ty).map_err(|error| error.to_string())
}

fn validate_public_json(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visiting: &mut HashSet<String>,
) -> Result<(), String> {
    if let SchemaType::Ref { id, .. } = ty {
        if !visiting.insert(id.to_string()) {
            return Ok(());
        }
        let body = &graph
            .lookup(id)
            .ok_or_else(|| format!("schema graph contains dangling reference: {id}"))?
            .body;
        let result = validate_public_json(graph, body, visiting);
        visiting.remove(id.as_str());
        return result;
    }
    match ty {
        SchemaType::Secret { .. }
        | SchemaType::QuotaToken { .. }
        | SchemaType::PermissionCard { .. }
        | SchemaType::Future { .. }
        | SchemaType::Stream { .. } => Err("type has no public JSON representation".into()),
        SchemaType::Record { fields, .. } => {
            for field in fields {
                validate_public_json(graph, &field.body, visiting)?;
            }
            Ok(())
        }
        SchemaType::Variant { cases, .. } => {
            for payload in cases.iter().filter_map(|case| case.payload.as_ref()) {
                validate_public_json(graph, payload, visiting)?;
            }
            Ok(())
        }
        SchemaType::Tuple { elements, .. } => {
            for element in elements {
                validate_public_json(graph, element, visiting)?;
            }
            Ok(())
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            validate_public_json(graph, element, visiting)
        }
        SchemaType::Map { key, value, .. } => {
            validate_public_json(graph, key, visiting)?;
            validate_public_json(graph, value, visiting)
        }
        SchemaType::Option { inner, .. } => validate_public_json(graph, inner, visiting),
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = spec.ok.as_deref() {
                validate_public_json(graph, ok, visiting)?;
            }
            if let Some(err) = spec.err.as_deref() {
                validate_public_json(graph, err, visiting)?;
            }
            Ok(())
        }
        SchemaType::Union { spec, .. } => {
            for branch in &spec.branches {
                validate_public_json(graph, &branch.body, visiting)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, CorsOptions, HttpMethod, LiteralSegment, PathVariable,
        Snapshotting,
    };
    use golem_common::schema::{AgentConstructorSchema, NamedField, NamedFieldType, SchemaTypeDef};
    use test_r::test;

    fn field(name: &str, schema: SchemaType) -> NamedField {
        NamedField::user_supplied(name, schema)
    }

    fn named(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.into(),
            body,
            metadata: Default::default(),
        }
    }

    fn fixtures(
        input: Vec<NamedField>,
        output: SchemaType,
    ) -> (AgentTypeSchema, AgentMethodSchema) {
        let method = AgentMethodSchema {
            name: "run".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(input),
            output_schema: OutputSchema::Single(Box::new(output)),
            http_endpoint: vec![],
            read_only: None,
        };
        let agent = AgentTypeSchema {
            type_name: AgentTypeName("test".into()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
            },
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        };
        (agent, method)
    }

    fn route() -> (HttpMountDetails, HttpEndpointDetails) {
        (
            HttpMountDetails {
                path_prefix: vec![PathSegment::Literal(LiteralSegment {
                    value: "api".into(),
                })],
                auth_details: None,
                phantom_agent: false,
                cors_options: CorsOptions {
                    allowed_patterns: vec![],
                },
                webhook_suffix: vec![],
            },
            HttpEndpointDetails {
                http_method: HttpMethod::Post(Empty {}),
                path_suffix: vec![],
                header_vars: vec![],
                query_vars: vec![],
                auth_details: None,
                cors_options: CorsOptions {
                    allowed_patterns: vec![],
                },
            },
        )
    }

    #[test]
    fn resolves_stream_refs_and_preserves_non_stream_inputs() {
        let (mut agent, method) = fixtures(
            vec![
                field("events", SchemaType::ref_to("events".into())),
                field("limit", SchemaType::u32()),
            ],
            SchemaType::string(),
        );
        agent.schema.defs.push(SchemaTypeDef {
            id: "events".into(),
            name: None,
            body: SchemaType::stream(Some(SchemaType::string())),
        });
        let (mount, endpoint) = route();

        let filtered = validate_route(&agent, &method, &mount, &endpoint).unwrap();
        assert_eq!(filtered.fields(), &[field("limit", SchemaType::u32())]);
    }

    #[test]
    fn rejects_asymmetric_nested_stream_shapes() {
        let (agent, input_nested) = fixtures(
            vec![field(
                "events",
                SchemaType::list(SchemaType::stream(Some(SchemaType::u8()))),
            )],
            SchemaType::string(),
        );
        let (mount, endpoint) = route();
        assert!(validate_route(&agent, &input_nested, &mount, &endpoint).is_err());

        let (_, output_nested) = fixtures(
            vec![],
            SchemaType::record(vec![named(
                "events",
                SchemaType::list(SchemaType::stream(Some(SchemaType::u8()))),
            )]),
        );
        assert!(validate_route(&agent, &output_nested, &mount, &endpoint).is_err());
    }

    #[test]
    fn rejects_reserved_and_duplicate_output_slots() {
        let (agent, reserved) = fixtures(
            vec![field(
                "__ds-control",
                SchemaType::stream(Some(SchemaType::u8())),
            )],
            SchemaType::string(),
        );
        let (mount, endpoint) = route();
        assert!(validate_route(&agent, &reserved, &mount, &endpoint).is_err());

        let (_, duplicate) = fixtures(
            vec![],
            SchemaType::record(vec![
                named("events", SchemaType::stream(Some(SchemaType::u8()))),
                named("events", SchemaType::stream(Some(SchemaType::u8()))),
            ]),
        );
        assert!(validate_route(&agent, &duplicate, &mount, &endpoint).is_err());
    }

    #[test]
    fn rejects_slot_names_longer_than_protocol_limit() {
        assert!(validate_slot_name(&"a".repeat(64)).is_ok());
        let slot = "a".repeat(65);
        let (agent, method) = fixtures(
            vec![field(&slot, SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::string(),
        );
        let (mount, endpoint) = route();

        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
    }

    #[test]
    fn stream_input_can_share_constructor_path_name_but_not_method_bindings() {
        use golem_common::model::agent::{HeaderVariable, QueryVariable};
        let (agent, method) = fixtures(
            vec![field("id", SchemaType::stream(Some(SchemaType::string())))],
            SchemaType::string(),
        );
        let (mut mount, endpoint) = route();
        mount
            .path_prefix
            .push(PathSegment::PathVariable(PathVariable {
                variable_name: "id".into(),
            }));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_ok());
        let mut path_endpoint = endpoint.clone();
        path_endpoint
            .path_suffix
            .push(PathSegment::PathVariable(PathVariable {
                variable_name: "id".into(),
            }));
        assert!(validate_route(&agent, &method, &mount, &path_endpoint).is_err());
        let mut query_endpoint = endpoint.clone();
        query_endpoint.query_vars.push(QueryVariable {
            variable_name: "id".into(),
            query_param_name: "events".into(),
        });
        assert!(validate_route(&agent, &method, &mount, &query_endpoint).is_err());
        let mut header_endpoint = endpoint;
        header_endpoint.header_vars.push(HeaderVariable {
            variable_name: "id".into(),
            header_name: "events".into(),
        });
        assert!(validate_route(&agent, &method, &mount, &header_endpoint).is_err());
    }

    #[test]
    fn rejects_greedy_and_reserved_path_variables() {
        let (agent, method) = fixtures(
            vec![field("events", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::string(),
        );
        let (mut mount, mut endpoint) = route();
        mount
            .path_prefix
            .push(PathSegment::RemainingPathVariable(PathVariable {
                variable_name: "tail".into(),
            }));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());

        let (mount, _) = route();
        endpoint
            .path_suffix
            .push(PathSegment::PathVariable(PathVariable {
                variable_name: "streams".into(),
            }));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
    }
}
