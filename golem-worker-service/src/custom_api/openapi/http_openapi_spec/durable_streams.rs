// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may obtain a copy at http://license.golem.cloud/LICENSE.

use super::super::route_schema::StreamSlotSchema;
use super::super::schema_mapping::render_output_schema;
use super::*;
use golem_service_base::custom_api::MethodParameter;

const OFFSET: &str = "GolemDSOffset";
const READ_OFFSET: &str = "GolemDSReadOffset";
const SESSION: &str = "GolemDSSessionId";
const MANIFEST: &str = "GolemDSManifest";
const PROBLEM: &str = "GolemDSProblem";

pub(super) fn emit(
    route: &RichCompiledRoute,
    schema: &RouteSchema,
    graph: &SchemaGraph,
    components: &mut Map<String, Value>,
    paths: &mut BTreeMap<String, Map<String, Value>>,
) -> Result<(), String> {
    let RichRouteBehaviour::CallAgent(behaviour) = &route.behavior else {
        unreachable!()
    };
    let call = schema
        .call_agent
        .as_ref()
        .ok_or("DS route schema missing")?;
    let slots = call.stream_slots.as_ref().ok_or("DS slots missing")?;
    add_components(components)?;
    let base = render_full_path(&route.path);
    let mut session_name = "session".to_string();
    while call.path_params.iter().any(|p| p.name == session_name) {
        session_name.insert_str(0, "ds_");
    }
    let session_path = format!(
        "{}/invocations/{{{session_name}}}",
        base.trim_end_matches('/')
    );
    let url_bound = behaviour.method_parameters.iter().all(|p| {
        matches!(
            p,
            MethodParameter::Path { .. } | MethodParameter::Query { .. }
        )
    });
    let mut creation_parameters = Vec::new();
    add_route_parameters(route, schema, graph, components, &mut creation_parameters)?;
    let creation_body = build_request_body(
        &schema.request_body,
        graph,
        components,
        &mut creation_parameters,
    )?;
    let path_parameters: Vec<_> = creation_parameters
        .iter()
        .filter(|p| p["in"] == "path")
        .cloned()
        .collect();
    let mut session_parameters = path_parameters.clone();
    session_parameters.push(path_parameter(&session_name, reference(SESSION)));
    let mut operation = |path: &str,
                         method: &str,
                         label: &str,
                         parameters: Vec<Value>,
                         body: Option<Value>,
                         responses: Value,
                         slot: Option<&StreamSlotSchema>|
     -> Result<(), String> {
        let mut op = json!({
            "operationId": format!("{}-{}-{}-{}", behaviour.agent_type.0, behaviour.method_name, method, hex_path(path)),
            "summary": label,
            "parameters": parameters,
            "responses": responses,
        });
        if let Some(description) = &behaviour.method_description {
            op["description"] = json!(description);
        }
        if let Some(slot) = slot.filter(|_| method == "put") {
            let media = if slot.binary {
                "application/octet-stream"
            } else {
                "application/json"
            };
            let contract = if body.is_some() {
                format!(
                    "Stream-Forked-From must be a same-server path in this route family. The optional initial body must be {media}."
                )
            } else {
                format!(
                    "Body must be empty. Content-Type is optional; when supplied it must be {media}."
                )
            };
            op["description"] = json!(
                format!(
                    "{}\n\n{contract}",
                    behaviour.method_description.as_deref().unwrap_or_default()
                )
                .trim()
            );
        }
        if let Some(security) = build_security(route) {
            op["security"] = security;
        }
        if let Some(body) = body {
            op["requestBody"] = body;
        }
        let item = paths.entry(path.into()).or_default();
        item.insert("x-golem-route-mode".into(), json!("durable-streams"));
        if let Some(slot) = slot {
            item.insert("x-golem-stream-slot".into(), json!({
                "name": slot.name, "direction": if slot.writable { "input" } else { "output" },
                "element-schema-ref": format!("#/components/schemas/{}", slot_component(&base, &slot.name)),
            }));
        }
        if item.insert(method.into(), op).is_some() {
            return Err(format!("duplicate OpenAPI operation {method} {path}"));
        }
        Ok(())
    };

    let mut create_responses = responses(
        &[
            ("201", "Session created"),
            ("200", "Existing invocation"),
            ("400", "Invalid arguments"),
            ("409", "Invocation arguments conflict"),
        ],
        false,
    );
    for code in ["201", "200"] {
        create_responses[code]["headers"] =
            json!({"Location": header("Created session URL", string_schema())});
    }
    operation(
        &base,
        "put",
        "create-session",
        creation_parameters.clone(),
        creation_body.clone(),
        create_responses,
        None,
    )?;
    let mut parameters = creation_parameters.clone();
    parameters.push(path_parameter(&session_name, reference(SESSION)));
    operation(
        &session_path,
        "put",
        "ensure-session",
        parameters,
        creation_body,
        responses(
            &[
                ("201", "Session created"),
                ("200", "Existing invocation"),
                ("400", "Invalid arguments"),
                ("409", "Invocation arguments conflict"),
            ],
            false,
        ),
        None,
    )?;
    for method in ["get", "head"] {
        let mut r = responses(
            &[(
                "200",
                "Session metadata with application/json Content-Type; HEAD has no body",
            )],
            false,
        );
        r["200"]["headers"] = json!({"Stream-Closed": header("All slots closed or deleted", json!({"type":"boolean"})), "Cache-Control": header("Session metadata is not cached", string_schema())});
        if method == "get" {
            r["200"]["content"] = json!({"application/json":{"schema":reference(MANIFEST)}});
        }
        operation(
            &session_path,
            method,
            &format!("{method}-session"),
            session_parameters.clone(),
            None,
            r,
            None,
        )?;
    }
    let mut cancel = responses(
        &[(
            "204",
            "Open streams cooperatively cancelled; history retained. Does not interrupt agent execution.",
        )],
        false,
    );
    cancel["204"]["description"] = json!(
        "Open streams cooperatively cancelled; repeated cancellation succeeds. Does not interrupt agent execution."
    );
    operation(
        &session_path,
        "delete",
        "cancel-session",
        session_parameters.clone(),
        None,
        cancel,
        None,
    )?;

    let mut fork_name = "fork".to_string();
    while call.path_params.iter().any(|p| p.name == fork_name) {
        fork_name.insert_str(0, "ds_");
    }
    let fork_session_path = format!(
        "{}/forks/{{{fork_name}}}/invocations/{{{session_name}}}",
        base.trim_end_matches('/')
    );
    let mut fork_parameters = session_parameters.clone();
    fork_parameters.push(path_parameter(&fork_name, reference(SESSION)));
    for method in ["get", "head"] {
        let mut r = responses(&[("200", "Fork manifest and immutable fork point")], false);
        r["200"]["headers"] = json!({"Stream-Closed": header("All slots closed or deleted", json!({"type":"boolean"})), "Cache-Control": header("Session metadata is not cached", string_schema())});
        if method == "get" {
            r["200"]["content"] = json!({"application/json":{"schema":reference(MANIFEST)}});
        }
        operation(
            &fork_session_path,
            method,
            &format!("{method}-fork"),
            fork_parameters.clone(),
            None,
            r,
            None,
        )?;
    }

    for slot in slots {
        let element = render_output_schema(graph, &slot.element, components)?;
        insert_component(
            components,
            &slot_component(&base, &slot.name),
            element.clone(),
        )?;
        let path = format!("{session_path}/streams/{}", slot.name.replace('$', "%24"));
        let fork_path = format!(
            "{fork_session_path}/streams/{}",
            slot.name.replace('$', "%24")
        );
        let media = if slot.binary {
            "application/octet-stream"
        } else {
            "application/json"
        };
        let data_schema = if slot.binary {
            arbitrary_binary_schema()
        } else {
            json!({"type":"array", "items": element})
        };
        let mut parameters = session_parameters.clone();
        if url_bound {
            parameters.extend(
                creation_parameters
                    .iter()
                    .filter(|p| p["in"] != "path")
                    .cloned(),
            );
        }
        let mut r = responses(
            &[
                ("201", "Session and slot created"),
                ("200", "Existing slot"),
                (
                    "400",
                    "Body must be empty; optional Content-Type must match the slot. Lazy creation requires all method arguments in the URL",
                ),
                ("409", "Content type mismatch or tombstoned slot"),
            ],
            false,
        );
        for code in ["201", "200"] {
            r[code]["headers"] = metadata_headers(false);
        }
        operation(
            &path,
            "put",
            &format!("ensure-{}", slot.name),
            parameters,
            None,
            r,
            Some(slot),
        )?;
        let mut r = responses(
            &[("200", "Stream metadata"), ("410", "Slot deleted")],
            false,
        );
        r["200"]["headers"] = metadata_headers(false);
        for (path, parameters) in [(&path, &session_parameters), (&fork_path, &fork_parameters)] {
            operation(
                path,
                "head",
                &format!("head-{}", slot.name),
                parameters.clone(),
                None,
                r.clone(),
                Some(slot),
            )?;
        }
        let mut parameters = session_parameters.clone();
        parameters.push(query_parameter("offset", false, reference(READ_OFFSET)));
        parameters.push(query_parameter(
            "live",
            false,
            json!({"type":"string","enum":["long-poll","sse"]}),
        ));
        parameters.push(query_parameter(
            "cursor",
            false,
            json!({"type":"integer","minimum":0,"maximum":u64::MAX - 180}),
        ));
        parameters.push(header_parameter("If-None-Match", false, string_schema()));
        let mut r = responses(
            &[
                (
                    "200",
                    "Catch-up or live data; SSE emits data arrays or base64 byte batches, followed by control events with streamNextOffset and upToDate. Control has streamClosed: true at EOF, otherwise string streamCursor.",
                ),
                ("204", "Long-poll reached EOF or timed out without items"),
                ("304", "Closed stream matches If-None-Match"),
                ("400", "Invalid offset, live mode or cursor"),
                ("410", "Slot deleted"),
                ("429", "Catch-up rate limit exceeded"),
            ],
            false,
        );
        r["200"]["content"] = json!({media: {"schema":data_schema}, "text/event-stream":{"schema":{"type":"string"}}});
        for code in ["200", "204", "304"] {
            r[code]["headers"] = metadata_headers(true);
        }
        for path in [&path, &fork_path] {
            let mut parameters = parameters.clone();
            if path == &fork_path {
                parameters.push(path_parameter(&fork_name, reference(SESSION)));
            }
            operation(
                path,
                "get",
                &format!("read-{}", slot.name),
                parameters,
                None,
                r.clone(),
                Some(slot),
            )?;
        }
        for (path, parameters) in [(&path, &session_parameters), (&fork_path, &fork_parameters)] {
            operation(
                path,
                "delete",
                &format!("delete-{}", slot.name),
                parameters.clone(),
                None,
                responses(
                    &[
                        ("204", "Slot cancelled and tombstoned"),
                        ("410", "Slot already deleted"),
                    ],
                    false,
                ),
                Some(slot),
            )?;
        }
        if slot.writable {
            let mut parameters = session_parameters.clone();
            if url_bound {
                parameters.extend(creation_parameters.iter().filter(|p| p["in"] != "path").cloned().map(|mut p| {
                    p["required"] = json!(false);
                    p["description"] = json!("Required according to the method schema only when this POST creates the session; ignored for an existing session.");
                    p
                }));
            }
            for name in [
                "Producer-Id",
                "Producer-Epoch",
                "Producer-Seq",
                "Stream-Closed",
            ] {
                let schema = match name {
                    "Producer-Id" => json!({"type":"string","minLength":1}),
                    "Stream-Closed" => {
                        json!({"type":"string","description":"Case-insensitive true closes atomically; other values are ignored."})
                    }
                    _ => json!({"type":"integer","minimum":0,"maximum":9007199254740991u64}),
                };
                let mut p = header_parameter(name, false, schema);
                p["description"] = json!(
                    "Supply all three Producer-* headers together or omit all. Retry with the same tuple after a lost response."
                );
                parameters.push(p);
            }
            let input = render_input_schema(graph, &slot.element, components)?;
            let append_schema = if slot.binary {
                arbitrary_binary_schema()
            } else {
                json!({"anyOf":[{"allOf":[input.clone(), {"not":{"type":"array"}}]}, {"type":"array","minItems":1,"maxItems":4096,"items":input}]})
            };
            let body = json!({"required":false,"description":"Nonempty typed body requires its content type. JSON arrays flatten one level; array-valued messages require an outer batch. Empty body is valid only with Stream-Closed: true and ignores Content-Type. Create the session first if method arguments cannot all be supplied in the URL.","content":{media:{"schema":append_schema}}});
            let mut r = responses(
                &[
                    ("200", "Producer append accepted"),
                    (
                        "204",
                        "Ordinary append, duplicate producer append, or close accepted",
                    ),
                    ("400", "Invalid body or producer headers"),
                    ("403", "Producer epoch fenced"),
                    (
                        "409",
                        "Closed stream, content type mismatch or producer sequence gap",
                    ),
                    ("410", "Slot deleted"),
                    ("413", "Append body or message count exceeds limit"),
                    ("429", "Append rate limit exceeded"),
                ],
                true,
            );
            for code in ["200", "204", "403", "409"] {
                let mut headers = json!({});
                if code != "403" {
                    headers["Stream-Next-Offset"] = header(
                        "Next offset on success or closed-stream conflict",
                        reference(OFFSET),
                    );
                    headers["Stream-Closed"] = header(
                        "Closure on success or closed-stream conflict",
                        json!({"type":"boolean"}),
                    );
                }
                let producer_headers: &[&str] = match code {
                    "403" => &["Producer-Epoch"],
                    "409" => &["Producer-Expected-Seq", "Producer-Received-Seq"],
                    _ => &["Producer-Epoch", "Producer-Seq"],
                };
                for name in producer_headers {
                    headers[*name] = header(
                        "Producer outcome; present when applicable",
                        json!({"type":"integer","minimum":0,"maximum":9007199254740991u64}),
                    );
                }
                r[code]["headers"] = headers;
            }
            for path in [&path, &fork_path] {
                let mut parameters = parameters.clone();
                if path == &fork_path {
                    parameters.retain(|p| p["in"] == "path" || p["in"] == "header");
                    parameters.push(path_parameter(&fork_name, reference(SESSION)));
                }
                operation(
                    path,
                    "post",
                    &format!("append-{}", slot.name),
                    parameters,
                    Some(body.clone()),
                    r.clone(),
                    Some(slot),
                )?;
            }
        }
        let mut create_fork_parameters = fork_parameters.clone();
        create_fork_parameters.extend([
            header_parameter("Stream-Forked-From", true, string_schema()),
            header_parameter("Stream-Fork-Offset", false, reference(OFFSET)),
            header_parameter(
                "Stream-Fork-Sub-Offset",
                false,
                json!({"type":"integer","minimum":0}),
            ),
            header_parameter("Stream-Closed", false, json!({"type":"boolean"})),
        ]);
        let fork_body = json!({"required":false,"content":{media:{"schema":if slot.binary { arbitrary_binary_schema() } else { json!({"type":"array","items":element}) }}}});
        let mut fork_responses = responses(
            &[
                ("201", "Fork created"),
                ("200", "Matching fork already exists"),
                (
                    "403",
                    "Initial body or Stream-Closed supplied for a read-only slot",
                ),
                (
                    "409",
                    "Fork configuration conflicts, target is tombstoned, or source is soft-deleted",
                ),
                ("413", "Fork copy or initial body exceeds its limit"),
                ("429", "Fork rate limited"),
            ],
            false,
        );
        for code in ["201", "200"] {
            fork_responses[code]["headers"] = metadata_headers(false);
            fork_responses[code]["headers"]["Location"] = header("Fork slot URL", string_schema());
        }
        fork_responses["429"]["headers"]["Retry-After"] =
            header("Retry delay in seconds, when available", string_schema());
        operation(
            &fork_path,
            "put",
            &format!("fork-{}", slot.name),
            create_fork_parameters,
            Some(fork_body),
            fork_responses,
            Some(slot),
        )?;
    }
    Ok(())
}

fn hex_path(path: &str) -> String {
    path.bytes().map(|b| format!("{b:02x}")).collect()
}

fn slot_component(base: &str, slot: &str) -> String {
    format!("GolemDSSlot_{}_{}", hex_path(base), hex_path(slot))
}

fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn header(description: &str, schema: Value) -> Value {
    json!({"description":description,"schema":schema})
}

fn metadata_headers(read: bool) -> Value {
    let mut headers = json!({
        "Stream-Next-Offset": header("Next read cursor", reference(OFFSET)),
        "Stream-Closed": header("Terminal state; on reads true only at EOF", json!({"type":"boolean"})),
        "Stream-Cancelled": header("Cancellation terminal", json!({"type":"boolean"})),
        "Stream-Up-To-Date": header("Read reached current head", json!({"type":"boolean"})),
        "Cache-Control": header("Caching policy", string_schema()),
        "ETag": header("Stream entity tag", string_schema()),
    });
    if read {
        headers["Stream-Cursor"] = header(
            "Long-poll collapsing cursor; SSE carries streamCursor in control events instead",
            string_schema(),
        );
        headers["Stream-SSE-Data-Encoding"] = header(
            "Present for byte-stream SSE data events",
            json!({"type":"string","enum":["base64"]}),
        );
    }
    headers
}

fn responses(entries: &[(&str, &str)], problem: bool) -> Value {
    let mut result = json!({
        "400":{"description":"Invalid request or unsupported TTL/expiry header"},
        "404":{"description":"Session or slot not found"},
        "503":{"description":"Executor unavailable or live-reader limit exceeded","headers":{"Retry-After":header("Retry delay in seconds", string_schema())}},
    });
    for (code, description) in entries {
        result[*code] = json!({"description":description});
    }
    if problem {
        for code in ["400", "404", "413"] {
            if !result[code].is_null() {
                let description = result[code]["description"].as_str().unwrap();
                result[code]["description"] =
                    json!(format!("{description}. Error responses may be bodyless."));
                result[code]["content"] =
                    json!({"application/problem+json":{"schema":reference(PROBLEM)}});
            }
        }
    }
    result
}

fn insert_component(
    components: &mut Map<String, Value>,
    name: &str,
    schema: Value,
) -> Result<(), String> {
    if let Some(existing) = components.get(name) {
        if existing != &schema {
            return Err(format!("conflicting component schema {name}"));
        }
    } else {
        components.insert(name.into(), schema);
    }
    Ok(())
}

fn add_components(components: &mut Map<String, Value>) -> Result<(), String> {
    for (name, schema) in [
        (OFFSET, json!({"type":"string","pattern":"^[0-9a-f]{48}$"})),
        (
            READ_OFFSET,
            json!({"type":"string","pattern":"^([0-9a-f]{48}|-1|now)$","default":"-1"}),
        ),
        (
            SESSION,
            json!({"type":"string","pattern":"^[A-Za-z0-9._-]{1,128}$"}),
        ),
        (
            PROBLEM,
            json!({"type":"object","required":["path","detail"],"properties":{"path":{"type":"string"},"detail":{"type":"string"}}}),
        ),
        (
            MANIFEST,
            json!({"type":"object","required":["session","streams","closed","fork"],"properties":{
                "session":reference(SESSION),"closed":{"type":"boolean"},"streams":{"type":"array","items":{
                    "type":"object","required":["name","contentType","nextOffset","closed","cancelled","deleted"],"properties":{
                        "name":{"type":"string"},"contentType":{"type":"string"},"nextOffset":reference(OFFSET),
                        "closed":{"type":"boolean"},"cancelled":{"type":"boolean"},"deleted":{"type":"boolean"}
                    }
                }},"fork":{"anyOf":[{"type":"object","required":["sourcePath","forkOffset","subOffset"],"properties":{
                    "sourcePath":{"type":"string"},"forkOffset":reference(OFFSET),"subOffset":{"type":"integer","minimum":0}
                }},{"type":"null"}]}
            }}),
        ),
    ] {
        insert_component(components, name, schema)?;
    }
    Ok(())
}
