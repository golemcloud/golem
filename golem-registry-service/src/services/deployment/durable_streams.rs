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

use golem_common::model::agent::{
    DurableStreamRouteOptions, DurableStreamSlotSource, HttpEndpointDetails, HttpMountDetails,
    PathSegment,
};
use golem_common::schema::agent::contains_stream_in_graph;
use golem_common::schema::{
    AgentMethodSchema, AgentTypeSchema, FieldSource, InputSchema, OutputSchema, SchemaGraph,
    SchemaType,
};
use golem_service_base::custom_api::{
    DurableStreamRepresentation, DurableStreamRouteLoadPolicy, DurableStreamRoutePolicy,
    DurableStreamSlot, DurableStreamSlotDirection,
};
use std::collections::{HashMap, HashSet};

const RESERVED_PATH_NAMES: &[&str] = &["invocations", "streams", "forks"];
const MAX_CONCURRENT_READERS_PER_STREAM: u32 = 16;

struct DiscoveredSlot {
    canonical_name: String,
    direction: DurableStreamSlotDirection,
    representation: DurableStreamRepresentation,
}

/// Validates the static Durable Streams slots and returns the part of the
/// method input which is still bound by the ordinary HTTP parameter compiler,
/// together with the fully resolved public route policy.
pub(super) fn validate_route(
    agent: &AgentTypeSchema,
    method: &AgentMethodSchema,
    mount: &HttpMountDetails,
    endpoint: &HttpEndpointDetails,
) -> Result<(InputSchema, DurableStreamRoutePolicy), String> {
    validate_path(mount, endpoint)?;

    let graph = &agent.schema;
    let mut slot_names = HashSet::new();
    let mut slots = Vec::new();
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
                let representation = validate_stream_element(graph, inner.as_deref(), &field.name)?;
                reject_stream_binding(endpoint, &field.name)?;
                slots.push(DiscoveredSlot {
                    canonical_name: field.name.clone(),
                    direction: DurableStreamSlotDirection::Input,
                    representation,
                });
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
                let representation = validate_stream_element(graph, inner.as_deref(), "$result")?;
                slots.push(DiscoveredSlot {
                    canonical_name: "$result".into(),
                    direction: DurableStreamSlotDirection::Output,
                    representation,
                });
            }
            SchemaType::Record { fields, .. } if contains_stream_in_graph(graph, output) => {
                for field in fields {
                    match resolve(graph, &field.body)? {
                        SchemaType::Stream { inner, .. } => {
                            validate_slot_name(&field.name)?;
                            insert_slot(&mut slot_names, &field.name)?;
                            let representation =
                                validate_stream_element(graph, inner.as_deref(), &field.name)?;
                            slots.push(DiscoveredSlot {
                                canonical_name: field.name.clone(),
                                direction: DurableStreamSlotDirection::Output,
                                representation,
                            });
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
                slots.push(DiscoveredSlot {
                    canonical_name: "$result".into(),
                    direction: DurableStreamSlotDirection::Output,
                    representation: DurableStreamRepresentation::Json,
                });
            }
        },
    }

    let policy = resolve_policy(slots, endpoint.durable_streams.as_ref())?;
    Ok((InputSchema::parameters(non_stream_fields), policy))
}

fn resolve_policy(
    discovered: Vec<DiscoveredSlot>,
    options: Option<&DurableStreamRouteOptions>,
) -> Result<DurableStreamRoutePolicy, String> {
    let mut overrides = HashMap::new();
    if let Some(options) = options {
        for slot in &options.slots {
            let key = match &slot.source {
                DurableStreamSlotSource::Input(source) => {
                    (DurableStreamSlotDirection::Input, source.name.clone())
                }
                DurableStreamSlotSource::Output(source) => {
                    (DurableStreamSlotDirection::Output, source.name.clone())
                }
            };
            if overrides.contains_key(&key) {
                return Err(format!(
                    "Duplicate Durable Streams slot declaration '{}:{}'",
                    match key.0 {
                        DurableStreamSlotDirection::Input => "input",
                        DurableStreamSlotDirection::Output => "output",
                    },
                    key.1
                ));
            }
            overrides.insert(key, slot);
        }
    }

    let mut public_names = HashSet::new();
    let mut slots = Vec::with_capacity(discovered.len());
    for slot in discovered {
        let override_ = overrides.remove(&(slot.direction, slot.canonical_name.clone()));
        let public_name = override_
            .and_then(|slot| slot.name.as_deref())
            .unwrap_or(&slot.canonical_name)
            .to_string();
        validate_public_slot_name(
            &public_name,
            slot.canonical_name == "$result"
                && override_.and_then(|slot| slot.name.as_ref()).is_none(),
        )?;
        if !public_names.insert(public_name.clone()) {
            return Err(format!(
                "Duplicate Durable Streams public slot name '{public_name}'"
            ));
        }
        let content_type = resolve_content_type(
            slot.representation,
            override_.and_then(|slot| slot.content_type.as_deref()),
            &slot.canonical_name,
        )?;
        slots.push(DurableStreamSlot {
            canonical_name: slot.canonical_name,
            public_name,
            direction: slot.direction,
            content_type,
            representation: slot.representation,
        });
    }
    if let Some(((_, name), _)) = overrides.into_iter().next() {
        return Err(format!("Unknown Durable Streams slot selector '{name}'"));
    }

    let has_writable_slot = slots.iter().any(DurableStreamSlot::writable);
    let allow_external_writes = options
        .and_then(|options| options.allow_external_writes)
        .unwrap_or(true);
    if options.is_some_and(|options| options.allow_external_writes.is_some()) && !has_writable_slot
    {
        return Err(
            "allow_external_writes cannot be set on a route without a writable input slot".into(),
        );
    }

    let load = options
        .and_then(|options| options.load.as_ref())
        .map(|load| DurableStreamRouteLoadPolicy {
            max_concurrent_readers_per_stream: load.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: load
                .max_append_requests_per_second_per_stream,
        });
    if let Some(load) = &load {
        if !matches!(
            load.max_concurrent_readers_per_stream,
            None | Some(1..=MAX_CONCURRENT_READERS_PER_STREAM)
        ) {
            return Err(format!(
                "max_concurrent_readers_per_stream must be between 1 and {MAX_CONCURRENT_READERS_PER_STREAM}"
            ));
        }
        if load.max_append_requests_per_second_per_stream == Some(0) {
            return Err(
                "max_append_requests_per_second_per_stream must be greater than zero".into(),
            );
        }
        if !allow_external_writes && load.max_append_requests_per_second_per_stream.is_some() {
            return Err(
                "max_append_requests_per_second_per_stream cannot be set when external writes are disabled"
                    .into(),
            );
        }
    }

    Ok(DurableStreamRoutePolicy {
        slots,
        allow_external_writes,
        allow_stream_delete: options
            .and_then(|options| options.allow_stream_delete)
            .unwrap_or(true),
        allow_invocation_delete: options
            .and_then(|options| options.allow_invocation_delete)
            .unwrap_or(true),
        load,
    })
}

fn validate_public_slot_name(name: &str, built_in_result_default: bool) -> Result<(), String> {
    if (name == "$result" && !built_in_result_default)
        || RESERVED_PATH_NAMES.contains(&name)
        || name.starts_with("__ds")
    {
        return Err(format!(
            "Durable Streams public slot name '{name}' is reserved"
        ));
    }
    if name.is_empty()
        || name.len() > 64
        || matches!(name, "." | "..")
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'$')
        })
        || (name.contains('$') && name != "$result")
    {
        return Err(format!(
            "Durable Streams public slot name '{name}' is not URL-segment-safe"
        ));
    }
    Ok(())
}

fn resolve_content_type(
    representation: DurableStreamRepresentation,
    override_: Option<&str>,
    slot: &str,
) -> Result<String, String> {
    let default = match representation {
        DurableStreamRepresentation::Json => "application/json",
        DurableStreamRepresentation::Bytes => "application/octet-stream",
    };
    let Some(value) = override_ else {
        return Ok(default.into());
    };
    let parsed: mime::Mime = value
        .parse()
        .map_err(|_| format!("Durable Streams slot '{slot}' has invalid content type '{value}'"))?;
    if parsed.params().next().is_some()
        || parsed.type_() == mime::STAR
        || parsed.subtype() == mime::STAR
    {
        return Err(format!(
            "Durable Streams slot '{slot}' content type cannot contain parameters or wildcards"
        ));
    }
    let normalized = parsed.essence_str().to_ascii_lowercase();
    let is_json = (parsed.type_() == mime::APPLICATION && parsed.subtype() == mime::JSON)
        || parsed.suffix() == Some(mime::JSON);
    match representation {
        DurableStreamRepresentation::Json if parsed != mime::APPLICATION_JSON => Err(format!(
            "Durable Streams JSON slot '{slot}' must use application/json"
        )),
        DurableStreamRepresentation::Bytes if is_json || parsed.type_() == mime::TEXT => {
            Err(format!(
                "Durable Streams byte slot '{slot}' must use a non-JSON, non-text content type"
            ))
        }
        _ => Ok(normalized),
    }
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
) -> Result<DurableStreamRepresentation, String> {
    let element = element.ok_or_else(|| {
        format!("Durable Streams slot '{slot}' has no element type for public JSON")
    })?;
    if contains_stream_in_graph(graph, element) {
        return Err(format!(
            "Durable Streams slot '{slot}' has a nested stream element"
        ));
    }
    if matches!(resolve(graph, element)?, SchemaType::U8 { .. }) {
        Ok(DurableStreamRepresentation::Bytes)
    } else {
        validate_public_json(graph, element, &mut HashSet::new())
            .map_err(|error| format!("Durable Streams slot '{slot}': {error}"))?;
        Ok(DurableStreamRepresentation::Json)
    }
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
        AgentMode, AgentTypeName, CorsOptions, DurableStreamInputSlotSource,
        DurableStreamOutputSlotSource, DurableStreamRouteLoadOptions, DurableStreamSlotOptions,
        HttpMethod, LiteralSegment, PathVariable, Snapshotting,
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
            kind: golem_common::schema::AgentTypeKind::Regular,
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
                static_bindings: vec![],
                filesystem_bindings: vec![],
                openapi_provider_method: None,
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
                durable_streams: None,
            },
        )
    }

    fn options(slots: Vec<DurableStreamSlotOptions>) -> DurableStreamRouteOptions {
        DurableStreamRouteOptions {
            slots,
            allow_external_writes: None,
            allow_stream_delete: None,
            allow_invocation_delete: None,
            load: None,
        }
    }

    fn input_slot(
        source: &str,
        name: Option<&str>,
        content_type: Option<&str>,
    ) -> DurableStreamSlotOptions {
        DurableStreamSlotOptions {
            source: DurableStreamSlotSource::Input(DurableStreamInputSlotSource {
                name: source.into(),
            }),
            name: name.map(Into::into),
            content_type: content_type.map(Into::into),
        }
    }

    fn output_slot(
        source: &str,
        name: Option<&str>,
        content_type: Option<&str>,
    ) -> DurableStreamSlotOptions {
        DurableStreamSlotOptions {
            source: DurableStreamSlotSource::Output(DurableStreamOutputSlotSource {
                name: source.into(),
            }),
            name: name.map(Into::into),
            content_type: content_type.map(Into::into),
        }
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

        let (filtered, policy) = validate_route(&agent, &method, &mount, &endpoint).unwrap();
        assert_eq!(filtered.fields(), &[field("limit", SchemaType::u32())]);
        assert_eq!(policy.slots.len(), 2);
        assert_eq!(policy.slots[0].canonical_name, "events");
        assert_eq!(
            policy.slots[0].representation,
            DurableStreamRepresentation::Json
        );
        assert_eq!(policy.slots[1].canonical_name, "$result");
        assert_eq!(policy.slots[1].public_name, "$result");
        assert!(policy.allow_external_writes);
        assert!(policy.allow_stream_delete);
        assert!(policy.allow_invocation_delete);
        assert_eq!(policy.load, None);
    }

    #[test]
    fn resolves_aliases_representations_content_types_and_explicit_policy() {
        let (agent, method) = fixtures(
            vec![field(
                "requestBytes",
                SchemaType::stream(Some(SchemaType::u8())),
            )],
            SchemaType::stream(Some(SchemaType::string())),
        );
        let (mount, mut endpoint) = route();
        endpoint.durable_streams = Some(DurableStreamRouteOptions {
            slots: vec![
                input_slot(
                    "requestBytes",
                    Some("requests"),
                    Some("Application/Vnd.Golem.Binary"),
                ),
                output_slot("$result", Some("responses"), Some("application/json")),
            ],
            allow_external_writes: Some(false),
            allow_stream_delete: Some(false),
            allow_invocation_delete: Some(false),
            load: Some(DurableStreamRouteLoadOptions {
                max_concurrent_readers_per_stream: Some(16),
                max_append_requests_per_second_per_stream: None,
            }),
        });

        let (_, policy) = validate_route(&agent, &method, &mount, &endpoint).unwrap();
        assert_eq!(
            policy.slots,
            vec![
                DurableStreamSlot {
                    canonical_name: "requestBytes".into(),
                    public_name: "requests".into(),
                    direction: DurableStreamSlotDirection::Input,
                    content_type: "application/vnd.golem.binary".into(),
                    representation: DurableStreamRepresentation::Bytes,
                },
                DurableStreamSlot {
                    canonical_name: "$result".into(),
                    public_name: "responses".into(),
                    direction: DurableStreamSlotDirection::Output,
                    content_type: "application/json".into(),
                    representation: DurableStreamRepresentation::Json,
                },
            ]
        );
        assert!(!policy.allow_external_writes);
        assert!(!policy.allow_stream_delete);
        assert!(!policy.allow_invocation_delete);
        assert_eq!(
            policy.load,
            Some(DurableStreamRouteLoadPolicy {
                max_concurrent_readers_per_stream: Some(16),
                max_append_requests_per_second_per_stream: None,
            })
        );
    }

    #[test]
    fn rejects_unknown_wrong_direction_and_duplicate_selectors() {
        let (agent, method) = fixtures(
            vec![field("input", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::stream(Some(SchemaType::u8())),
        );
        let (mount, mut endpoint) = route();

        for slots in [
            vec![input_slot("missing", None, None)],
            vec![output_slot("input", None, None)],
            vec![
                input_slot("input", None, None),
                input_slot("input", Some("other"), None),
            ],
        ] {
            endpoint.durable_streams = Some(options(slots));
            assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
        }
    }

    #[test]
    fn rejects_invalid_or_colliding_public_names() {
        let (agent, method) = fixtures(
            vec![field("input", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::stream(Some(SchemaType::u8())),
        );
        let (mount, mut endpoint) = route();

        for name in [
            "",
            ".",
            "..",
            "$result",
            "streams",
            "__ds-private",
            "not/a-slot",
        ] {
            endpoint.durable_streams = Some(options(vec![input_slot("input", Some(name), None)]));
            assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
        }

        endpoint.durable_streams = Some(options(vec![
            input_slot("input", Some("same"), None),
            output_slot("$result", Some("same"), None),
        ]));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());

        endpoint.durable_streams = Some(options(vec![input_slot(
            "input",
            Some(&"a".repeat(64)),
            None,
        )]));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_ok());
        endpoint.durable_streams = Some(options(vec![input_slot(
            "input",
            Some(&"a".repeat(65)),
            None,
        )]));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
    }

    #[test]
    fn aliases_do_not_legalize_invalid_canonical_names() {
        let (agent, method) = fixtures(
            vec![field("streams", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::string(),
        );
        let (mount, mut endpoint) = route();
        endpoint.durable_streams = Some(options(vec![input_slot(
            "streams",
            Some("legal-alias"),
            None,
        )]));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
    }

    #[test]
    fn enforces_representation_specific_content_types() {
        let (agent, method) = fixtures(
            vec![
                field("bytes", SchemaType::stream(Some(SchemaType::u8()))),
                field("json", SchemaType::stream(Some(SchemaType::string()))),
            ],
            SchemaType::string(),
        );
        let (mount, mut endpoint) = route();

        for content_type in [
            "invalid",
            "*/*",
            "application/octet-stream; charset=utf-8",
            "application/problem+json",
            "model/gltf+json",
            "text/plain",
        ] {
            endpoint.durable_streams =
                Some(options(vec![input_slot("bytes", None, Some(content_type))]));
            assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
        }
        for content_type in [
            "application/octet-stream",
            "application/json; charset=utf-8",
            "text/plain",
        ] {
            endpoint.durable_streams =
                Some(options(vec![input_slot("json", None, Some(content_type))]));
            assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
        }
    }

    #[test]
    fn validates_write_options_and_load_boundaries() {
        let (agent, method) = fixtures(
            vec![field("input", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::string(),
        );
        let (mount, mut endpoint) = route();

        for (readers, appends) in [(Some(0), None), (Some(17), None), (None, Some(0))] {
            let mut route_options = options(vec![]);
            route_options.load = Some(DurableStreamRouteLoadOptions {
                max_concurrent_readers_per_stream: readers,
                max_append_requests_per_second_per_stream: appends,
            });
            endpoint.durable_streams = Some(route_options);
            assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
        }

        let mut route_options = options(vec![]);
        route_options.allow_external_writes = Some(false);
        route_options.load = Some(DurableStreamRouteLoadOptions {
            max_concurrent_readers_per_stream: Some(1),
            max_append_requests_per_second_per_stream: Some(1),
        });
        endpoint.durable_streams = Some(route_options);
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());

        let (_, output_only) = fixtures(vec![], SchemaType::stream(Some(SchemaType::string())));
        endpoint.durable_streams = Some(options(vec![]));
        assert!(validate_route(&agent, &output_only, &mount, &endpoint).is_ok());
        for allow_external_writes in [true, false] {
            let mut route_options = options(vec![]);
            route_options.allow_external_writes = Some(allow_external_writes);
            endpoint.durable_streams = Some(route_options);
            assert!(validate_route(&agent, &output_only, &mount, &endpoint).is_err());
        }

        let mut route_options = options(vec![]);
        route_options.load = Some(DurableStreamRouteLoadOptions {
            max_concurrent_readers_per_stream: Some(1),
            max_append_requests_per_second_per_stream: Some(1),
        });
        endpoint.durable_streams = Some(route_options);
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_ok());
    }

    #[test]
    fn preserves_only_the_implicit_result_name_exception_and_synthetic_json() {
        let (agent, method) = fixtures(
            vec![field("input", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::u8(),
        );
        let (mount, mut endpoint) = route();
        endpoint.durable_streams = Some(options(vec![output_slot(
            "$result",
            None,
            Some("application/json"),
        )]));

        let (_, policy) = validate_route(&agent, &method, &mount, &endpoint).unwrap();
        let result = policy
            .slots
            .iter()
            .find(|slot| slot.canonical_name == "$result")
            .unwrap();
        assert_eq!(result.public_name, "$result");
        assert_eq!(result.representation, DurableStreamRepresentation::Json);

        endpoint.durable_streams =
            Some(options(vec![output_slot("$result", Some("$result"), None)]));
        assert!(validate_route(&agent, &method, &mount, &endpoint).is_err());
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

        let (_, duplicate_across_directions) = fixtures(
            vec![field("events", SchemaType::stream(Some(SchemaType::u8())))],
            SchemaType::record(vec![named(
                "events",
                SchemaType::stream(Some(SchemaType::u8())),
            )]),
        );
        assert!(validate_route(&agent, &duplicate_across_directions, &mount, &endpoint).is_err());
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
