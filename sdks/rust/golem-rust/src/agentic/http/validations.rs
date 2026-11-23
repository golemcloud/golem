// Copyright 2024-2026 Golem Cloud
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

use crate::agentic::{EnrichedAgentMethod, EnrichedParameterSchema, ExtendedAgentConstructor};
use crate::golem_agentic::golem::agent::common::{
    AgentIdBinding, HttpEndpointDetails, HttpMountDetails, PathSegment,
};
use golem_schema::http::wit::wire as http;
use std::collections::HashSet;

#[doc(hidden)]
pub fn validate_wire_agent_http(
    agent: &crate::golem_agentic::golem::agent::common::AgentType,
) -> Result<(), String> {
    use crate::golem_agentic::golem::agent::common::{FieldSource, InputSchema, OutputSchema};
    use crate::schema::wit::wire::Role;
    let names = |input: &InputSchema| {
        let InputSchema::Parameters(fields) = input;
        fields
            .iter()
            .map(|field| field.name.clone())
            .collect::<HashSet<_>>()
    };
    let roles = |input: &InputSchema, role: Role| {
        let InputSchema::Parameters(fields) = input;
        fields
            .iter()
            .filter(|field| {
                matches!(field.source, FieldSource::UserSupplied)
                    && agent.schema.type_nodes[field.schema as usize]
                        .metadata
                        .role
                        .as_ref()
                        .is_some_and(|actual| {
                            std::mem::discriminant(actual) == std::mem::discriminant(&role)
                        })
            })
            .map(|field| field.name.clone())
            .collect::<HashSet<_>>()
    };
    if let Some(mount) = &agent.http_mount {
        let input = &agent.constructor.input_schema;
        let parameters = names(input);
        validate_agent_id_binding(mount)?;
        if mount.agent_id_binding.is_some()
            && (agent.mode == crate::golem_agentic::golem::agent::common::AgentMode::Ephemeral
                || agent.kind
                    == crate::golem_agentic::golem::agent::common::AgentTypeKind::HttpRouter)
        {
            return Err("Agent ID bindings require a regular durable agent".into());
        }
        if !mount.filesystem_bindings.is_empty()
            && (agent.mode != crate::golem_agentic::golem::agent::common::AgentMode::Durable
                || agent.kind != crate::golem_agentic::golem::agent::common::AgentTypeKind::Regular
                || mount.phantom_agent)
        {
            return Err(
                "Filesystem bindings require a regular durable owner without phantom_agent".into(),
            );
        }
        validate_no_catch_all_in_http_mount(&agent.type_name, mount)?;
        for (role, kind) in [
            (Role::UnstructuredText, "UnstructuredText"),
            (Role::UnstructuredBinary, "UnstructuredBinary"),
            (Role::Multimodal, "Multimodal"),
        ] {
            if mount.agent_id_binding.is_none()
                && let Some(name) = roles(input, role).iter().next()
            {
                return Err(format!(
                    "Agent '{}' constructor parameter '{}' cannot be of type '{}' when used with HTTP mount",
                    agent.type_name, name, kind
                ));
            }
        }
        validate_mount_variables_exist_in_constructor(mount, &parameters)?;
        validate_constructor_vars_are_satisfied(mount, &parameters)?;
    }
    for method in &agent.methods {
        if method.http_endpoint.is_empty() {
            continue;
        }
        if agent.http_mount.is_none() {
            return Err(format!(
                "Agent method '{}' of '{}' defines HTTP endpoints but the agent is not mounted over HTTP. Please specify mount details in 'agent_definition'",
                method.name, agent.type_name
            ));
        }
        let input = &method.input_schema;
        let decoder = crate::schema::wit::GraphDecoder::new(&agent.schema)
            .map_err(|error| error.to_string())?;
        let defs = decoder.decode_defs().map_err(|error| error.to_string())?;
        let InputSchema::Parameters(fields) = input;
        let mut implicit_query_vars = names(input);
        let mut uses_streams = false;
        for field in fields {
            let graph = crate::schema::SchemaGraph {
                root: decoder
                    .decode_type_at(field.schema)
                    .map_err(|error| error.to_string())?,
                defs: defs.clone(),
            };
            if matches!(field.source, FieldSource::UserSupplied) {
                uses_streams |=
                    crate::agentic::schema_type_contains_stream(&graph.root, &graph.defs);
            }
            if matches!(
                graph.resolve_ref(&graph.root),
                Ok(crate::schema::SchemaType::Stream { .. })
            ) {
                implicit_query_vars.remove(&field.name);
            }
        }
        if let OutputSchema::Single(index) = method.output_schema {
            let root = decoder
                .decode_type_at(index)
                .map_err(|error| error.to_string())?;
            uses_streams |= crate::agentic::schema_type_contains_stream(&root, &defs);
        }
        for endpoint in &method.http_endpoint {
            validate_agent_id_endpoint(
                agent.http_mount.as_ref().unwrap(),
                endpoint,
                &implicit_query_vars,
                uses_streams,
            )?;
            validate_endpoint_variables(
                endpoint,
                &names(input),
                &roles(input, Role::UnstructuredBinary),
                &roles(input, Role::UnstructuredText),
                &roles(input, Role::Multimodal),
            )?;
        }
    }
    Ok(())
}

// This validation is applied along with other details in the definition
// unlike basic parsing of the HTTP mount which is done earlier.
pub fn validate_http_mount(
    agent_class_name: &str,
    agent_mount: &HttpMountDetails,
    agent_constructor: &ExtendedAgentConstructor,
) -> Result<(), String> {
    let constructor_input_params = collect_constructor_input_parameter_names(agent_constructor);

    validate_agent_id_binding(agent_mount)?;
    validate_no_catch_all_in_http_mount(agent_class_name, agent_mount)?;
    if agent_mount.agent_id_binding.is_none() {
        validate_constructor_params_are_http_safe(agent_class_name, agent_constructor)?;
    }
    validate_mount_variables_exist_in_constructor(agent_mount, &constructor_input_params)?;
    validate_constructor_vars_are_satisfied(agent_mount, &constructor_input_params)?;

    Ok(())
}

pub fn validate_http_endpoint(
    agent_class_name: &str,
    agent_method: &EnrichedAgentMethod,
    http_mount_details: Option<&HttpMountDetails>,
) -> Result<(), String> {
    if agent_method.http_endpoint.is_empty() {
        return Ok(());
    }

    validate_mount_is_defined_for_http_endpoint(
        agent_class_name,
        agent_method,
        http_mount_details,
    )?;

    let method_vars = collect_method_input_vars(&agent_method.input_schema);

    let unstructured_binary_params = collect_unstructured_binary_params(&agent_method.input_schema);
    let unstructured_text_params = collect_unstructured_text_params(&agent_method.input_schema);
    let multimodal_params = collect_multimodal_params(&agent_method.input_schema);

    let implicit_query_vars = agent_method
        .input_schema
        .iter()
        .filter_map(|(name, param)| match param {
            EnrichedParameterSchema::Value(graph)
                if !matches!(
                    graph.resolve_ref(&graph.root),
                    Ok(crate::schema::SchemaType::Stream { .. })
                ) =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    let uses_streams = agent_method
        .input_schema
        .iter()
        .map(|(_, param)| match param {
            EnrichedParameterSchema::Value(graph) => graph,
        })
        .chain(agent_method.output_schema.iter().map(|(_, graph)| graph))
        .any(|graph| crate::agentic::schema_type_contains_stream(&graph.root, &graph.defs));
    for endpoint in &agent_method.http_endpoint {
        validate_agent_id_endpoint(
            http_mount_details.unwrap(),
            endpoint,
            &implicit_query_vars,
            uses_streams,
        )?;
        validate_endpoint_variables(
            endpoint,
            &method_vars,
            &unstructured_binary_params,
            &unstructured_text_params,
            &multimodal_params,
        )?;
    }

    Ok(())
}

fn collect_unstructured_binary_params(
    input_schema: &[(String, EnrichedParameterSchema)],
) -> HashSet<String> {
    let mut unstructured_binary_params = HashSet::new();

    for (param_name, param_schema) in input_schema {
        if let EnrichedParameterSchema::Value(schema) = param_schema
            && is_unstructured_binary_schema(schema)
        {
            unstructured_binary_params.insert(param_name.clone());
        }
    }

    unstructured_binary_params
}

fn collect_unstructured_text_params(
    input_schema: &[(String, EnrichedParameterSchema)],
) -> HashSet<String> {
    let mut unstructured_text_params = HashSet::new();

    for (param_name, param_schema) in input_schema {
        if let EnrichedParameterSchema::Value(schema) = param_schema
            && is_unstructured_text_schema(schema)
        {
            unstructured_text_params.insert(param_name.clone());
        }
    }

    unstructured_text_params
}

fn collect_multimodal_params(
    input_schema: &[(String, EnrichedParameterSchema)],
) -> HashSet<String> {
    let mut multimodal_params = HashSet::new();

    for (param_name, param_schema) in input_schema {
        if let EnrichedParameterSchema::Value(schema) = param_schema
            && is_multimodal_schema(schema)
        {
            multimodal_params.insert(param_name.clone());
        }
    }

    multimodal_params
}

// Collects method input variable names.
fn collect_method_input_vars(
    input_schema: &[(String, EnrichedParameterSchema)],
) -> HashSet<String> {
    input_schema.iter().map(|(name, _)| name.clone()).collect()
}

fn is_unstructured_text_schema(schema: &crate::schema::SchemaGraph) -> bool {
    schema.root.metadata().role.as_ref() == Some(&crate::schema::Role::UnstructuredText)
}

fn is_unstructured_binary_schema(schema: &crate::schema::SchemaGraph) -> bool {
    schema.root.metadata().role.as_ref() == Some(&crate::schema::Role::UnstructuredBinary)
}

fn is_multimodal_schema(schema: &crate::schema::SchemaGraph) -> bool {
    schema.root.metadata().role.as_ref() == Some(&crate::schema::Role::Multimodal)
}

fn validate_endpoint_variables(
    endpoint: &HttpEndpointDetails,
    method_vars: &HashSet<String>,
    unstructured_binary_params: &HashSet<String>,
    unstructured_text_params: &HashSet<String>,
    multimodal_params: &HashSet<String>,
) -> Result<(), String> {
    fn validate_variable(
        variable_name: &str,
        location: &str,
        unstructured_binary_params: &HashSet<String>,
        unstructured_text_params: &HashSet<String>,
        multimodal_params: &HashSet<String>,
        method_vars: &HashSet<String>,
        binary_error: &str,
        text_error: &str,
        multimodal_error: &str,
    ) -> Result<(), String> {
        if unstructured_binary_params.contains(variable_name) {
            return Err(binary_error.to_string());
        }

        if unstructured_text_params.contains(variable_name) {
            return Err(text_error.to_string());
        }

        if multimodal_params.contains(variable_name) {
            return Err(multimodal_error.to_string());
        }

        if !method_vars.contains(variable_name) {
            return Err(format!(
                "HTTP endpoint {} variable '{}' is not defined in method input parameters.",
                location, variable_name
            ));
        }

        Ok(())
    }

    for var in &endpoint.header_vars {
        validate_variable(
            &var.variable_name,
            "header",
            unstructured_binary_params,
            unstructured_text_params,
            multimodal_params,
            method_vars,
            &format!(
                "HTTP endpoint header variable '{}' cannot be used for method parameters of type 'UnstructuredBinary'",
                var.variable_name
            ),
            &format!(
                "HTTP endpoint header variable '{}' cannot be used for method parameters of type 'UnstructuredText'",
                var.variable_name
            ),
            &format!(
                "HTTP endpoint header variable '{}' cannot be used for method parameters of type 'Multimodal'",
                var.variable_name
            ),
        )?;
    }

    for var in &endpoint.query_vars {
        validate_variable(
            &var.variable_name,
            "query",
            unstructured_binary_params,
            unstructured_text_params,
            multimodal_params,
            method_vars,
            &format!(
                "HTTP endpoint query variable '{}' cannot be used when the method has a single 'UnstructuredBinary' parameter.",
                var.variable_name
            ),
            &format!(
                "HTTP endpoint query variable '{}' cannot be used when the method has a single 'UnstructuredText' parameter.",
                var.variable_name
            ),
            &format!(
                "HTTP endpoint query variable '{}' cannot be used when the method has a single 'Multimodal' parameter.",
                var.variable_name
            ),
        )?;
    }

    for segment in &endpoint.path_suffix {
        match segment {
            PathSegment::Segment(
                http::PathSegment::RemainingPathVariable(path_variable)
                | http::PathSegment::PathVariable(path_variable),
            ) => {
                let name = &path_variable.variable_name;
                validate_variable(
                    name,
                    "path",
                    unstructured_binary_params,
                    unstructured_text_params,
                    multimodal_params,
                    method_vars,
                    &format!(
                        "HTTP endpoint path variable '{}' cannot be used when the method has a single 'UnstructuredBinary' parameter.",
                        name
                    ),
                    &format!(
                        "HTTP endpoint path variable '{}' cannot be used when the method has a single 'UnstructuredText' parameter.",
                        name
                    ),
                    &format!(
                        "HTTP endpoint path variable '{}' cannot be used when the method has a single 'Multimodal' parameter.",
                        name
                    ),
                )?;
            }
            PathSegment::Segment(http::PathSegment::Literal(_)) => {}
            PathSegment::SystemVariable(_) => {}
        }
    }

    Ok(())
}

fn validate_mount_is_defined_for_http_endpoint(
    agent_class_name: &str,
    agent_method: &EnrichedAgentMethod,
    http_mount_details: Option<&HttpMountDetails>,
) -> Result<(), String> {
    if http_mount_details.is_none() && !agent_method.http_endpoint.is_empty() {
        return Err(format!(
            "Agent method '{}' of '{}' defines HTTP endpoints but the agent is not mounted over HTTP. \
            Please specify mount details in 'agent_definition'",
            agent_method.name, agent_class_name
        ));
    }

    Ok(())
}

pub(crate) fn reject_query_param_in_string(path: &str, entity_name: &str) -> Result<(), String> {
    if path.contains('?') {
        return Err(format!("{} cannot contain query parameters", entity_name));
    }

    Ok(())
}

pub(crate) fn reject_empty_string(name: &str, entity_name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{} cannot be empty", entity_name));
    }
    Ok(())
}

fn validate_no_catch_all_in_http_mount(
    agent_class_name: &str,
    agent_mount: &HttpMountDetails,
) -> Result<(), String> {
    if let Some(catch_all_variable) =
        agent_mount
            .path_prefix
            .iter()
            .find_map(|segment| match segment {
                PathSegment::Segment(http::PathSegment::RemainingPathVariable(variable)) => {
                    Some(&variable.variable_name)
                }
                _ => None,
            })
    {
        return Err(format!(
            "HTTP mount for agent '{}' cannot contain catch-all path variable '{}'",
            agent_class_name, catch_all_variable
        ));
    }

    Ok(())
}

fn collect_http_mount_variables(agent_mount: &HttpMountDetails) -> HashSet<String> {
    let mut vars = HashSet::new();

    for segment in &agent_mount.path_prefix {
        if let PathSegment::Segment(http::PathSegment::PathVariable(path_variable)) = segment {
            vars.insert(path_variable.variable_name.clone());
        }
    }

    vars
}

fn collect_constructor_input_parameter_names(
    agent_constructor: &ExtendedAgentConstructor,
) -> HashSet<String> {
    let mut param_names = HashSet::new();

    for (name, _) in &agent_constructor.input_schema {
        param_names.insert(name.clone());
    }

    param_names
}

fn validate_constructor_params_are_http_safe(
    agent_class_name: &str,
    agent_constructor: &ExtendedAgentConstructor,
) -> Result<(), String> {
    for (name, schema) in &agent_constructor.input_schema {
        let EnrichedParameterSchema::Value(schema) = schema;
        if is_unstructured_text_schema(schema) {
            return Err(format!(
                "Agent '{}' constructor parameter '{}' cannot be of type 'UnstructuredText' when used with HTTP mount",
                agent_class_name, name
            ));
        }
        if is_unstructured_binary_schema(schema) {
            return Err(format!(
                "Agent '{}' constructor parameter '{}' cannot be of type 'UnstructuredBinary' when used with HTTP mount",
                agent_class_name, name
            ));
        }
        if is_multimodal_schema(schema) {
            return Err(format!(
                "Agent '{}' constructor parameter '{}' cannot be of type 'Multimodal' when used with HTTP mount",
                agent_class_name, name
            ));
        }
    }

    Ok(())
}

fn validate_mount_variables_exist_in_constructor(
    agent_mount: &HttpMountDetails,
    constructor_vars: &std::collections::HashSet<String>,
) -> Result<(), String> {
    for (segment_index, segment) in agent_mount.path_prefix.iter().enumerate() {
        if let PathSegment::Segment(http::PathSegment::PathVariable(path_variable)) = segment {
            let variable_name = &path_variable.variable_name;

            if matches!(&agent_mount.agent_id_binding, Some(AgentIdBinding::Path(binding)) if &binding.name == variable_name)
            {
                continue;
            }
            if !constructor_vars.contains(variable_name) {
                return Err(format!(
                    "HTTP mount path variable '{}' (in path segment {}) is not defined in the agent constructor.",
                    variable_name, segment_index,
                ));
            }
        }
    }

    Ok(())
}

fn validate_agent_id_binding(mount: &HttpMountDetails) -> Result<(), String> {
    if let Some(path) = &mount.create_route {
        if mount.agent_id_binding.is_none() {
            return Err("HTTP create route requires an agent ID binding".into());
        }
        if path.iter().any(|segment| {
            !matches!(
                segment,
                PathSegment::SystemVariable(_)
                    | PathSegment::Segment(http::PathSegment::Literal(_))
            )
        }) {
            return Err("HTTP create route can contain only literals and system variables".into());
        }
    }
    let Some(selector) = &mount.agent_id_binding else {
        return Ok(());
    };
    let (AgentIdBinding::Path(binding) | AgentIdBinding::Query(binding)) = selector;
    if binding.name.trim().is_empty() {
        return Err("Agent ID binding requires a nonempty name".into());
    }
    if mount.phantom_agent {
        return Err("Agent ID binding cannot be combined with phantom_agent".into());
    }
    if matches!(selector, AgentIdBinding::Path(_)) &&
        mount.path_prefix.iter().filter(|s| matches!(s, PathSegment::Segment(http::PathSegment::PathVariable(v)) if v.variable_name == binding.name)).count() != 1 {
        return Err("Agent ID path binding must name exactly one ordinary mount capture".into());
    }
    for segment in &mount.path_prefix {
        if let PathSegment::Segment(
            http::PathSegment::PathVariable(variable)
            | http::PathSegment::RemainingPathVariable(variable),
        ) = segment
            && !matches!(selector, AgentIdBinding::Path(binding) if binding.name == variable.variable_name)
        {
            return Err(format!(
                "Agent ID binding cannot be combined with constructor URL binding '{}'",
                variable.variable_name
            ));
        }
    }
    Ok(())
}

fn validate_agent_id_endpoint(
    mount: &HttpMountDetails,
    endpoint: &HttpEndpointDetails,
    method_vars: &HashSet<String>,
    uses_streams: bool,
) -> Result<(), String> {
    let durable_streams = match endpoint.mode {
        Some(http::HttpRouteMode::DurableStreams) => true,
        None => uses_streams || endpoint.durable_streams.is_some(),
        _ => false,
    };
    let conflict = match &mount.agent_id_binding {
        Some(AgentIdBinding::Path(binding)) => endpoint.path_suffix.iter().any(|s| matches!(s, PathSegment::Segment(http::PathSegment::PathVariable(v) | http::PathSegment::RemainingPathVariable(v)) if v.variable_name == binding.name)),
        Some(AgentIdBinding::Query(binding)) => {
            let implicit_query = matches!(endpoint.http_method, crate::golem_agentic::golem::agent::common::HttpMethod::Method(http::HttpMethod::Get | http::HttpMethod::Head | http::HttpMethod::Delete));
            let explicitly_bound = endpoint.header_vars.iter().any(|v| v.variable_name == binding.name)
                || endpoint.query_vars.iter().any(|v| v.variable_name == binding.name)
                || endpoint.path_suffix.iter().any(|s| matches!(s, PathSegment::Segment(http::PathSegment::PathVariable(v) | http::PathSegment::RemainingPathVariable(v)) if v.variable_name == binding.name));
            endpoint.query_vars.iter().any(|q| q.query_param_name == binding.name)
                || (implicit_query && method_vars.contains(&binding.name) && !explicitly_bound)
                || (durable_streams && ["offset", "cursor", "live"].contains(&binding.name.as_str()))
        },
        None => false,
    };
    if conflict {
        return Err(
            "Agent ID binding conflicts with an endpoint input or durable-stream control".into(),
        );
    }
    Ok(())
}

fn validate_constructor_vars_are_satisfied(
    agent_mount: &HttpMountDetails,
    constructor_vars: &std::collections::HashSet<String>,
) -> Result<(), String> {
    if agent_mount.agent_id_binding.is_some() {
        return Ok(());
    }
    let provided_vars = collect_http_mount_variables(agent_mount);

    for constructor_var in constructor_vars {
        if !provided_vars.contains(constructor_var) {
            return Err(format!(
                "Agent constructor variable '{}' is not provided by the HTTP mount path.",
                constructor_var,
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use test_r::test;

    use super::*;
    use crate::agentic::{Schema, StructuredSchema, UnstructuredBinary, UnstructuredText};
    use crate::golem_agentic::golem::agent::common::{AuthDetails, CorsOptions, HttpMethod};
    use golem_rust_macro::{AllowedLanguages, AllowedMimeTypes};
    use golem_schema::http::wit::wire::{
        self as http, HeaderVariable, PathVariable, QueryVariable,
    };

    fn constructor_with_params(params: Vec<(&str, StructuredSchema)>) -> ExtendedAgentConstructor {
        let input_schema = params
            .into_iter()
            .map(|(name, schema)| {
                let StructuredSchema::Default(schema) = schema;

                (name.to_string(), EnrichedParameterSchema::Value(schema))
            })
            .collect();

        ExtendedAgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema,
        }
    }

    fn mount_with_segments(segments: Vec<PathSegment>) -> HttpMountDetails {
        HttpMountDetails {
            path_prefix: segments,
            auth_details: Some(AuthDetails { required: true }),
            phantom_agent: false,
            agent_id_binding: None,
            create_route: None,
            cors_options: CorsOptions {
                allowed_patterns: vec![],
            },
            webhook_suffix: vec![],
            static_bindings: vec![],
            filesystem_bindings: vec![],
            file_response_headers: vec![],
            openapi_provider_method: None,
        }
    }

    fn path_var(name: &str) -> PathSegment {
        PathSegment::Segment(http::PathSegment::PathVariable(PathVariable {
            variable_name: name.to_string(),
        }))
    }

    fn catch_all(name: &str) -> PathSegment {
        PathSegment::Segment(http::PathSegment::RemainingPathVariable(PathVariable {
            variable_name: name.to_string(),
        }))
    }

    fn literal() -> PathSegment {
        PathSegment::Segment(http::PathSegment::Literal("literal".to_string()))
    }

    #[test]
    fn agent_id_capture_is_exclusive_and_does_not_require_constructor_values() {
        use crate::golem_agentic::golem::agent::common::AgentIdBindingDetails;
        let constructor = constructor_with_params(vec![("customer", String::get_type())]);
        let mut mount = mount_with_segments(vec![literal(), path_var("instance")]);
        mount.agent_id_binding = Some(AgentIdBinding::Path(AgentIdBindingDetails {
            name: "instance".into(),
        }));
        assert!(validate_http_mount("Selected", &mount, &constructor).is_ok());
        mount.path_prefix.push(path_var("customer"));
        assert!(
            validate_http_mount("Selected", &mount, &constructor)
                .unwrap_err()
                .contains("constructor URL binding")
        );
        mount.path_prefix.pop();
        let same_name = constructor_with_params(vec![
            ("customer", String::get_type()),
            ("instance", String::get_type()),
        ]);
        assert!(validate_http_mount("Selected", &mount, &same_name).is_ok());
        mount.path_prefix.pop();
        assert!(
            validate_http_mount("Selected", &mount, &constructor)
                .unwrap_err()
                .contains("exactly one")
        );
        mount.agent_id_binding = Some(AgentIdBinding::Query(AgentIdBindingDetails {
            name: "instance".into(),
        }));
        assert!(validate_http_mount("Selected", &mount, &constructor).is_ok());
        mount.path_prefix.push(path_var("instance"));
        assert!(validate_http_mount("Selected", &mount, &constructor).is_err());
    }

    #[test]
    fn create_route_requires_id_and_literal_or_system_path() {
        use crate::golem_agentic::golem::agent::common::{AgentIdBindingDetails, SystemVariable};
        let constructor = constructor_with_params(vec![("customer", String::get_type())]);
        let mut mount = mount_with_segments(vec![literal()]);
        mount.create_route = Some(vec![literal()]);
        assert!(
            validate_http_mount("Selected", &mount, &constructor)
                .unwrap_err()
                .contains("requires an agent ID")
        );
        mount.agent_id_binding = Some(AgentIdBinding::Query(AgentIdBindingDetails {
            name: "id".into(),
        }));
        mount.create_route = Some(vec![
            literal(),
            PathSegment::SystemVariable(SystemVariable::AgentType),
        ]);
        assert!(validate_http_mount("Selected", &mount, &constructor).is_ok());
        mount.create_route = Some(vec![]);
        assert!(validate_http_mount("Selected", &mount, &constructor).is_ok());
        for capture in [path_var("id"), path_var("customer"), catch_all("rest")] {
            mount.create_route = Some(vec![capture]);
            assert!(
                validate_http_mount("Selected", &mount, &constructor)
                    .unwrap_err()
                    .contains("only literals")
            );
        }
        mount.create_route = None;
        mount.phantom_agent = true;
        assert!(
            validate_http_mount("Selected", &mount, &constructor)
                .unwrap_err()
                .contains("phantom_agent")
        );
    }

    #[test]
    fn query_id_is_reserved_from_implicit_and_explicit_method_inputs() {
        use crate::golem_agentic::golem::agent::common::AgentIdBindingDetails;
        let mut mount = mount_with_segments(vec![literal()]);
        mount.agent_id_binding = Some(AgentIdBinding::Query(AgentIdBindingDetails {
            name: "id".into(),
        }));
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec![],
            vec![],
            vec![],
            None,
            vec![],
        );
        let method = make_agent_method(
            "read",
            make_schema(vec!["id"], vec![]),
            vec![endpoint.clone()],
        );
        assert!(
            validate_http_endpoint("Selected", &method, Some(&mount))
                .unwrap_err()
                .contains("conflicts")
        );
        let mut method =
            make_agent_method("read", make_schema(vec!["filter"], vec![]), vec![endpoint]);
        assert!(validate_http_endpoint("Selected", &method, Some(&mount)).is_ok());
        method.http_endpoint[0].query_vars.push(QueryVariable {
            query_param_name: "id".into(),
            variable_name: "filter".into(),
        });
        assert!(
            validate_http_endpoint("Selected", &method, Some(&mount))
                .unwrap_err()
                .contains("conflicts")
        );
        for http_method in [
            http::HttpMethod::Get,
            http::HttpMethod::Head,
            http::HttpMethod::Delete,
        ] {
            let mut method = make_agent_method(
                "read",
                make_schema(vec!["id"], vec![]),
                vec![make_endpoint(
                    HttpMethod::Method(http_method),
                    vec![],
                    vec![],
                    vec![],
                    None,
                    vec![],
                )],
            );
            assert!(validate_http_endpoint("Selected", &method, Some(&mount)).is_err());
            method.http_endpoint[0].header_vars.push(HeaderVariable {
                header_name: "x-filter".into(),
                variable_name: "id".into(),
            });
            assert!(validate_http_endpoint("Selected", &method, Some(&mount)).is_ok());
            method.http_endpoint[0].header_vars.clear();
            method.http_endpoint[0].path_suffix.push(path_var("id"));
            assert!(validate_http_endpoint("Selected", &method, Some(&mount)).is_ok());
            method.http_endpoint[0].path_suffix.clear();
            method.http_endpoint[0].query_vars.push(QueryVariable {
                query_param_name: "filter".into(),
                variable_name: "id".into(),
            });
            assert!(validate_http_endpoint("Selected", &method, Some(&mount)).is_ok());
        }
        let method = make_agent_method(
            "write",
            make_schema(vec!["id"], vec![]),
            vec![make_endpoint(
                HttpMethod::Method(http::HttpMethod::Post),
                vec![],
                vec![],
                vec![],
                None,
                vec![],
            )],
        );
        assert!(validate_http_endpoint("Selected", &method, Some(&mount)).is_ok());
    }

    #[test]
    fn wire_id_mounts_allow_files_and_preserve_metadata() {
        use crate::agentic::ExtendedAgentType;
        use crate::golem_agentic::golem::agent::common::{
            AgentIdBindingDetails, AgentMode, AgentTypeKind, Snapshotting,
        };
        use http::{ExactFileMapping, FileMapping};
        let mut mount = mount_with_segments(vec![literal(), path_var("id")]);
        mount.agent_id_binding = Some(AgentIdBinding::Path(AgentIdBindingDetails {
            name: "id".into(),
        }));
        mount.create_route = Some(vec![literal()]);
        mount.filesystem_bindings = vec![FileMapping::Exact(ExactFileMapping {
            public_path: vec!["state".into()],
            file_path: "/state".into(),
        })];
        let mut extended = ExtendedAgentType {
            type_name: "Selected".into(),
            kind: AgentTypeKind::Regular,
            description: String::new(),
            source_language: "rust".into(),
            constructor: constructor_with_params(vec![("customer", String::get_type())]),
            methods: vec![],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: Some(mount),
            snapshotting: Snapshotting::Disabled,
            config: vec![],
            sorted_method_indices: vec![],
        };
        for binding in [
            AgentIdBinding::Path(AgentIdBindingDetails { name: "id".into() }),
            AgentIdBinding::Query(AgentIdBindingDetails { name: "id".into() }),
        ] {
            let mount = extended.http_mount.as_mut().unwrap();
            mount.path_prefix = if matches!(binding, AgentIdBinding::Path(_)) {
                vec![literal(), path_var("id")]
            } else {
                vec![literal()]
            };
            mount.agent_id_binding = Some(binding);
            let wire = extended.to_agent_type();
            assert!(validate_wire_agent_http(&wire).is_ok());
            let decoded = ExtendedAgentType::from_agent_type(wire).unwrap();
            let mount = decoded.http_mount.unwrap();
            assert!(mount.agent_id_binding.is_some());
            assert_eq!(mount.create_route.unwrap().len(), 1);
            assert_eq!(mount.filesystem_bindings.len(), 1);
            for (kind, mode) in [
                (AgentTypeKind::HttpRouter, AgentMode::Durable),
                (AgentTypeKind::Regular, AgentMode::Ephemeral),
            ] {
                let mut wire = extended.to_agent_type();
                wire.kind = kind;
                wire.mode = mode;
                assert!(
                    validate_wire_agent_http(&wire)
                        .unwrap_err()
                        .contains("regular durable")
                );
            }
        }
        for name in ["offset", "cursor", "live"] {
            extended.http_mount.as_mut().unwrap().agent_id_binding =
                Some(AgentIdBinding::Query(AgentIdBindingDetails {
                    name: name.into(),
                }));
            for output in [false, true] {
                let stream =
                    crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::stream(None));
                let mut method = make_agent_method(
                    "events",
                    vec![],
                    vec![make_endpoint(
                        HttpMethod::Method(http::HttpMethod::Post),
                        vec![],
                        vec![],
                        vec![],
                        None,
                        vec![],
                    )],
                );
                if output {
                    method.output_schema.push(("events".into(), stream));
                } else {
                    method
                        .input_schema
                        .push(("events".into(), EnrichedParameterSchema::Value(stream)));
                }
                for (mode, conflict) in [
                    (None, true),
                    (Some(http::HttpRouteMode::DurableStreams), true),
                    (Some(http::HttpRouteMode::Sse), false),
                    (Some(http::HttpRouteMode::WebSocket), false),
                ] {
                    method.http_endpoint[0].mode = mode;
                    assert_eq!(
                        validate_http_endpoint("Selected", &method, extended.http_mount.as_ref())
                            .is_err(),
                        conflict,
                        "{name}, output={output}, mode={mode:?}"
                    );
                    extended.methods = vec![method.clone()];
                    assert_eq!(
                        validate_wire_agent_http(&extended.to_agent_type()).is_err(),
                        conflict,
                        "wire {name}, output={output}, mode={mode:?}"
                    );
                }
            }
        }
        extended.methods.clear();
        let mount = extended.http_mount.as_mut().unwrap();
        mount.agent_id_binding = None;
        mount.create_route = None;
        mount.path_prefix.push(path_var("customer"));
        mount.phantom_agent = true;
        assert!(
            validate_wire_agent_http(&extended.to_agent_type())
                .unwrap_err()
                .contains("durable owner")
        );
        extended.http_mount.as_mut().unwrap().phantom_agent = false;
        extended.mode = AgentMode::Ephemeral;
        assert!(
            validate_wire_agent_http(&extended.to_agent_type())
                .unwrap_err()
                .contains("durable owner")
        );
        extended.mode = AgentMode::Durable;
        assert!(validate_wire_agent_http(&extended.to_agent_type()).is_ok());
        let mount = extended.http_mount.as_mut().unwrap();
        mount.filesystem_bindings.clear();
        mount.phantom_agent = true;
        assert!(validate_wire_agent_http(&extended.to_agent_type()).is_ok());
        extended.mode = AgentMode::Ephemeral;
        assert!(validate_wire_agent_http(&extended.to_agent_type()).is_ok());
    }

    #[test]
    fn validate_http_mount_success() {
        let constructor = constructor_with_params(vec![
            ("user_id", String::get_type()),
            ("org_id", String::get_type()),
        ]);

        let mount = mount_with_segments(vec![literal(), path_var("user_id"), path_var("org_id")]);

        let result = validate_http_mount("MyAgent", &mount, &constructor);

        assert!(result.is_ok());
    }

    #[test]
    fn fails_on_catch_all_segment() {
        let constructor = constructor_with_params(vec![("id", String::get_type())]);

        let mount = mount_with_segments(vec![path_var("id"), catch_all("rest")]);

        let err = validate_http_mount("MyAgent", &mount, &constructor).unwrap_err();

        assert_eq!(
            err,
            "HTTP mount for agent 'MyAgent' cannot contain catch-all path variable 'rest'"
        );
    }

    #[derive(AllowedMimeTypes, Clone, Debug)]
    enum MimeTypes {
        #[mime_type("application/json")]
        ApplicationJson,
        Fr,
    }

    #[test]
    fn fails_on_unstructured_binary_constructor_param() {
        let constructor =
            constructor_with_params(vec![("blob", UnstructuredBinary::<MimeTypes>::get_type())]);

        let mount = mount_with_segments(vec![path_var("blob")]);

        let err = validate_http_mount("MyAgent", &mount, &constructor).unwrap_err();

        assert_eq!(
            err,
            "Agent 'MyAgent' constructor parameter 'blob' cannot be of type 'UnstructuredBinary' when used with HTTP mount"
        );
    }

    #[test]
    fn fails_when_mount_variable_not_in_constructor() {
        let constructor = constructor_with_params(vec![("id", String::get_type())]);

        let mount = mount_with_segments(vec![path_var("missing")]);

        let err = validate_http_mount("MyAgent", &mount, &constructor).unwrap_err();

        assert_eq!(
            err,
            "HTTP mount path variable 'missing' (in path segment 0) is not defined in the agent constructor."
        );
    }

    #[test]
    fn fails_when_constructor_var_not_satisfied() {
        let constructor = constructor_with_params(vec![
            ("id", String::get_type()),
            ("org", String::get_type()),
        ]);

        let mount = mount_with_segments(vec![path_var("id")]);

        let err = validate_http_mount("MyAgent", &mount, &constructor).unwrap_err();

        assert_eq!(
            err,
            "Agent constructor variable 'org' is not provided by the HTTP mount path."
        );
    }

    fn make_schema(
        normal_vars: Vec<&str>,
        unstructured_vars: Vec<&str>,
    ) -> Vec<(String, EnrichedParameterSchema)> {
        make_schema_with_text(normal_vars, unstructured_vars, vec![])
    }

    fn make_schema_with_text(
        normal_vars: Vec<&str>,
        unstructured_binary_vars: Vec<&str>,
        unstructured_text_vars: Vec<&str>,
    ) -> Vec<(String, EnrichedParameterSchema)> {
        let mut fields = vec![];

        for name in normal_vars {
            fields.push((
                name.to_string(),
                EnrichedParameterSchema::Value(String::get_type().get_schema_graph().unwrap()),
            ));
        }

        for name in unstructured_binary_vars {
            fields.push((
                name.to_string(),
                EnrichedParameterSchema::Value(crate::agentic::unstructured_binary_schema_graph(
                    None,
                )),
            ));
        }

        for name in unstructured_text_vars {
            fields.push((
                name.to_string(),
                EnrichedParameterSchema::Value(crate::agentic::unstructured_text_schema_graph(
                    None,
                )),
            ));
        }

        fields
    }

    fn make_endpoint(
        http_method: HttpMethod,
        path_vars: Vec<&str>,
        header_vars: Vec<(&str, &str)>,
        query_vars: Vec<&str>,
        auth: Option<bool>,
        cors: Vec<&str>,
    ) -> HttpEndpointDetails {
        HttpEndpointDetails {
            http_method,
            path_suffix: path_vars
                .into_iter()
                .map(|name| {
                    PathSegment::Segment(http::PathSegment::PathVariable(PathVariable {
                        variable_name: name.to_string(),
                    }))
                })
                .collect(),
            header_vars: header_vars
                .into_iter()
                .map(|(header_name, variable_name)| HeaderVariable {
                    header_name: header_name.to_string(),
                    variable_name: variable_name.to_string(),
                })
                .collect(),
            query_vars: query_vars
                .into_iter()
                .map(|var| QueryVariable {
                    query_param_name: var.to_string(),
                    variable_name: var.to_string(),
                })
                .collect(),
            auth_details: auth.map(|b| AuthDetails { required: b }),
            cors_options: CorsOptions {
                allowed_patterns: cors.into_iter().map(|s| s.to_string()).collect(),
            },
            mode: None,
            durable_streams: None,
        }
    }

    fn make_agent_method(
        name: &str,
        input_schema: Vec<(String, EnrichedParameterSchema)>,
        endpoints: Vec<HttpEndpointDetails>,
    ) -> EnrichedAgentMethod {
        EnrichedAgentMethod {
            name: name.to_string(),
            description: "".to_string(),
            prompt_hint: None,
            input_schema,
            output_schema: vec![],
            http_endpoint: endpoints,
            read_only: None,
        }
    }

    #[test]
    fn test_no_http_endpoints() {
        let agent_method = make_agent_method("foo", make_schema(vec!["x"], vec![]), vec![]);
        assert!(validate_http_endpoint("AgentA", &agent_method, None).is_ok());
    }

    #[test]
    fn test_http_endpoint_without_mount() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec!["x"],
            vec![],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method("foo", make_schema(vec!["x"], vec![]), vec![endpoint]);
        let err = validate_http_endpoint("AgentA", &agent_method, None).unwrap_err();
        assert!(err.contains("defines HTTP endpoints but the agent is not mounted over HTTP"));
    }

    #[test]
    fn test_valid_endpoint() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec!["x"],
            vec![("X-Test", "x")],
            vec!["x"],
            Some(true),
            vec!["*"],
        );
        let agent_method = make_agent_method("foo", make_schema(vec!["x"], vec![]), vec![endpoint]);
        let mount = mount_with_segments(vec![path_var("foo")]);
        assert!(validate_http_endpoint("AgentA", &agent_method, Some(&mount)).is_ok());
    }

    #[test]
    fn test_header_unstructured_binary_error() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec![],
            vec![("X-Test", "b")],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method("foo", make_schema(vec![], vec!["b"]), vec![endpoint]);
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("cannot be used for method parameters of type 'UnstructuredBinary'"));
    }

    #[test]
    fn test_path_variable_not_in_method_params() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec!["y"],
            vec![],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method("foo", make_schema(vec!["x"], vec![]), vec![endpoint]);
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("is not defined in method input parameters"));
    }

    #[derive(AllowedLanguages, Clone, Debug)]
    enum Languages {
        #[code("en")]
        En,
        #[code("de")]
        De,
    }

    #[test]
    fn fails_on_unstructured_text_constructor_param() {
        let constructor =
            constructor_with_params(vec![("body", UnstructuredText::<Languages>::get_type())]);

        let mount = mount_with_segments(vec![path_var("body")]);

        let err = validate_http_mount("MyAgent", &mount, &constructor).unwrap_err();

        assert_eq!(
            err,
            "Agent 'MyAgent' constructor parameter 'body' cannot be of type 'UnstructuredText' when used with HTTP mount"
        );
    }

    #[test]
    fn test_header_unstructured_text_error() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec![],
            vec![("X-Test", "t")],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema_with_text(vec![], vec![], vec!["t"]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("cannot be used for method parameters of type 'UnstructuredText'"));
    }

    #[test]
    fn test_query_unstructured_text_error() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec![],
            vec![],
            vec!["t"],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema_with_text(vec![], vec![], vec!["t"]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("'UnstructuredText'"));
    }

    #[test]
    fn test_path_unstructured_text_error() {
        let endpoint = make_endpoint(
            HttpMethod::Method(http::HttpMethod::Get),
            vec!["t"],
            vec![],
            vec![],
            Some(true),
            vec![],
        );
        let agent_method = make_agent_method(
            "foo",
            make_schema_with_text(vec![], vec![], vec!["t"]),
            vec![endpoint],
        );
        let mount = mount_with_segments(vec![path_var("foo")]);
        let err = validate_http_endpoint("AgentA", &agent_method, Some(&mount)).unwrap_err();
        assert!(err.contains("'UnstructuredText'"));
    }
}
