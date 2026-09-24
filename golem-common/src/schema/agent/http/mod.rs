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

use super::{AgentMethodSchema, AgentTypeKind, AgentTypeSchema, FieldSource};
use crate::base_model::agent::http_files::valid_decoded_segment;
use crate::base_model::agent::{
    AgentMode, FileMapping, HttpMethod, HttpMountDetails, PathSegment, Snapshotting,
};
use crate::schema::validation::is_equivalent_cross_graph;
use crate::schema::{SchemaGraph, SchemaType};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpAgentValidationError {
    DuplicateMethod(String),
    InvalidFileMapping(String),
    RouterMethodOnRegularAgent(String),
    StaticBindingsOnRegularAgent,
    OpenApiProviderOnRegularAgent,
    RouterMode,
    RouterConstructor,
    RouterSnapshot,
    RouterMount,
    FilesystemOwner,
    RouterMethodRole(String),
    ProviderSchema(String),
    HandlerEndpointPolicy(String),
    HandlerSchema(String),
    UnboundConstructor(String),
}

impl Display for HttpAgentValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        use HttpAgentValidationError::*;
        match self {
            DuplicateMethod(name) => write!(f, "HTTP agent has duplicate method '{name}'"),
            InvalidFileMapping(error) => write!(f, "invalid HTTP file mapping: {error}"),
            RouterMethodOnRegularAgent(name) => write!(
                f,
                "regular agent method '{name}' uses the router-only ANY HTTP method"
            ),
            StaticBindingsOnRegularAgent => {
                write!(f, "static bindings are only valid on HTTP router agents")
            }
            OpenApiProviderOnRegularAgent => write!(
                f,
                "an OpenAPI provider is only valid on an HTTP router agent"
            ),
            RouterMode => write!(f, "HTTP router agents must be ephemeral"),
            RouterConstructor => write!(f, "HTTP router constructors cannot have input parameters"),
            RouterSnapshot => write!(f, "HTTP router agents cannot enable snapshotting"),
            RouterMount => write!(
                f,
                "HTTP router agents require a mount containing only valid literal path segments"
            ),
            FilesystemOwner => write!(
                f,
                "filesystem bindings require a non-phantom durable regular agent"
            ),
            RouterMethodRole(name) => write!(
                f,
                "HTTP router method '{name}' does not have a valid handler or provider role"
            ),
            ProviderSchema(name) => write!(
                f,
                "OpenAPI provider method '{name}' must take no input and return a string"
            ),
            HandlerEndpointPolicy(name) => write!(
                f,
                "HTTP router handler '{name}' has unsupported endpoint policy"
            ),
            HandlerSchema(name) => write!(
                f,
                "HTTP router handler '{name}' must accept request: HttpRequest and return HttpResponse"
            ),
            UnboundConstructor(name) => write!(
                f,
                "HTTP filesystem owner constructor parameter or path capture '{name}' is invalid or unbound"
            ),
        }
    }
}

impl std::error::Error for HttpAgentValidationError {}

fn headers_schema() -> SchemaType {
    SchemaType::list(SchemaType::record_from_fields([
        ("name", SchemaType::string()),
        ("value", SchemaType::list(SchemaType::u8())),
    ]))
}

pub fn request_schema() -> SchemaType {
    SchemaType::record_from_fields([
        ("method", SchemaType::string()),
        ("scheme", SchemaType::string()),
        ("authority", SchemaType::string()),
        ("path", SchemaType::string()),
        ("query", SchemaType::option(SchemaType::string())),
        ("headers", headers_schema()),
        (
            "body",
            SchemaType::stream(Some(SchemaType::list(SchemaType::u8()))),
        ),
    ])
}

pub fn response_schema() -> SchemaType {
    SchemaType::record_from_fields([
        ("status", SchemaType::u16()),
        ("headers", headers_schema()),
        (
            "body",
            SchemaType::stream(Some(SchemaType::list(SchemaType::u8()))),
        ),
    ])
}

pub(super) fn validate(agent: &AgentTypeSchema) -> Result<(), HttpAgentValidationError> {
    let mut names = HashSet::new();
    for method in &agent.methods {
        if !names.insert(&method.name) {
            return Err(HttpAgentValidationError::DuplicateMethod(
                method.name.clone(),
            ));
        }
    }
    if let Some(mount) = &agent.http_mount {
        FileMapping::validate_list(&mount.static_bindings)
            .map_err(HttpAgentValidationError::InvalidFileMapping)?;
        FileMapping::validate_list(&mount.filesystem_bindings)
            .map_err(HttpAgentValidationError::InvalidFileMapping)?;
    }
    match agent.kind {
        AgentTypeKind::HttpRouter => validate_router(agent),
        AgentTypeKind::Regular => {
            if let Some(method) = agent.methods.iter().find(|method| {
                method
                    .http_endpoint
                    .iter()
                    .any(|endpoint| matches!(endpoint.http_method, HttpMethod::Any(_)))
            }) {
                return Err(HttpAgentValidationError::RouterMethodOnRegularAgent(
                    method.name.clone(),
                ));
            }
            if let Some(mount) = &agent.http_mount {
                if !mount.static_bindings.is_empty() {
                    return Err(HttpAgentValidationError::StaticBindingsOnRegularAgent);
                }
                if mount.openapi_provider_method.is_some() {
                    return Err(HttpAgentValidationError::OpenApiProviderOnRegularAgent);
                }
                if !mount.filesystem_bindings.is_empty() {
                    validate_filesystem_owner(agent, mount)?;
                }
            }
            Ok(())
        }
    }
}

fn validate_router(agent: &AgentTypeSchema) -> Result<(), HttpAgentValidationError> {
    if agent.mode != AgentMode::Ephemeral {
        return Err(HttpAgentValidationError::RouterMode);
    }
    if !agent.constructor.input_schema.fields().is_empty() {
        return Err(HttpAgentValidationError::RouterConstructor);
    }
    if !matches!(agent.snapshotting, Snapshotting::Disabled(_)) {
        return Err(HttpAgentValidationError::RouterSnapshot);
    }
    let mount = agent
        .http_mount
        .as_ref()
        .ok_or(HttpAgentValidationError::RouterMount)?;
    if !mount.path_prefix.iter().all(|segment| matches!(segment, PathSegment::Literal(literal) if valid_decoded_segment(&literal.value))) {
        return Err(HttpAgentValidationError::RouterMount);
    }
    if !mount.filesystem_bindings.is_empty() {
        return Err(HttpAgentValidationError::FilesystemOwner);
    }
    if let Some(provider) = &mount.openapi_provider_method {
        let method = agent
            .methods
            .iter()
            .find(|method| &method.name == provider)
            .ok_or_else(|| HttpAgentValidationError::RouterMethodRole(provider.clone()))?;
        if !method.http_endpoint.is_empty() {
            return Err(HttpAgentValidationError::RouterMethodRole(
                method.name.clone(),
            ));
        }
        if !method.input_schema.fields().is_empty()
            || !output_matches(agent, method, &SchemaType::string())
        {
            return Err(HttpAgentValidationError::ProviderSchema(
                method.name.clone(),
            ));
        }
    }
    let mut handler_seen = false;
    for method in &agent.methods {
        if mount.openapi_provider_method.as_ref() == Some(&method.name) {
            continue;
        }
        if handler_seen || method.http_endpoint.len() != 1 {
            return Err(HttpAgentValidationError::RouterMethodRole(
                method.name.clone(),
            ));
        }
        handler_seen = true;
        let endpoint = &method.http_endpoint[0];
        if !matches!(endpoint.http_method, HttpMethod::Any(_)) || !endpoint.path_suffix.is_empty() {
            return Err(HttpAgentValidationError::RouterMethodRole(
                method.name.clone(),
            ));
        }
        if endpoint.auth_details.is_some()
            || !endpoint.cors_options.allowed_patterns.is_empty()
            || !endpoint.header_vars.is_empty()
            || !endpoint.query_vars.is_empty()
        {
            return Err(HttpAgentValidationError::HandlerEndpointPolicy(
                method.name.clone(),
            ));
        }
        let mut inputs = method
            .input_schema
            .fields()
            .iter()
            .filter(|field| field.source == FieldSource::UserSupplied);
        let request = inputs
            .next()
            .ok_or_else(|| HttpAgentValidationError::HandlerSchema(method.name.clone()))?;
        if request.name != "request"
            || inputs.next().is_some()
            || !schema_matches(agent, &request.schema, &request_schema())
            || !output_matches(agent, method, &response_schema())
        {
            return Err(HttpAgentValidationError::HandlerSchema(method.name.clone()));
        }
    }
    Ok(())
}

fn schema_matches(agent: &AgentTypeSchema, actual: &SchemaType, expected: &SchemaType) -> bool {
    is_equivalent_cross_graph(&agent.schema, actual, &SchemaGraph::empty(), expected)
}

fn output_matches(
    agent: &AgentTypeSchema,
    method: &AgentMethodSchema,
    expected: &SchemaType,
) -> bool {
    method
        .output_schema
        .schema()
        .is_some_and(|actual| schema_matches(agent, actual, expected))
}

fn validate_filesystem_owner(
    agent: &AgentTypeSchema,
    mount: &HttpMountDetails,
) -> Result<(), HttpAgentValidationError> {
    if agent.mode != AgentMode::Durable || mount.phantom_agent {
        return Err(HttpAgentValidationError::FilesystemOwner);
    }
    let mut captures = HashSet::new();
    for segment in &mount.path_prefix {
        match segment {
            PathSegment::PathVariable(variable)
                if captures.insert(variable.variable_name.as_str()) => {}
            PathSegment::Literal(literal) if valid_decoded_segment(&literal.value) => {}
            PathSegment::SystemVariable(_) => {}
            _ => {
                return Err(HttpAgentValidationError::UnboundConstructor(
                    "mount path".into(),
                ));
            }
        }
    }
    for field in agent.constructor.input_schema.fields() {
        if field.source != FieldSource::UserSupplied || !captures.remove(field.name.as_str()) {
            return Err(HttpAgentValidationError::UnboundConstructor(
                field.name.clone(),
            ));
        }
        let ty = agent
            .schema
            .resolve_ref(&field.schema)
            .map_err(|_| HttpAgentValidationError::UnboundConstructor(field.name.clone()))?;
        if !matches!(
            ty,
            SchemaType::String { .. }
                | SchemaType::Char { .. }
                | SchemaType::Bool { .. }
                | SchemaType::Enum { .. }
                | SchemaType::U8 { .. }
                | SchemaType::U16 { .. }
                | SchemaType::U32 { .. }
                | SchemaType::U64 { .. }
                | SchemaType::S8 { .. }
                | SchemaType::S16 { .. }
                | SchemaType::S32 { .. }
                | SchemaType::S64 { .. }
                | SchemaType::F32 { .. }
                | SchemaType::F64 { .. }
        ) {
            return Err(HttpAgentValidationError::UnboundConstructor(
                field.name.clone(),
            ));
        }
    }
    if !captures.is_empty() {
        return Err(HttpAgentValidationError::UnboundConstructor(
            captures.into_iter().next().unwrap().into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
