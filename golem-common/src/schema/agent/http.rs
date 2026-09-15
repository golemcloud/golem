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
use crate::schema::schema_type::NamedFieldType;
use crate::schema::validation::is_equivalent_cross_graph;
use crate::schema::{SchemaGraph, SchemaType};
use std::collections::HashSet;

fn record(fields: impl IntoIterator<Item = (&'static str, SchemaType)>) -> SchemaType {
    SchemaType::record(
        fields
            .into_iter()
            .map(|(name, body)| NamedFieldType {
                name: name.into(),
                body,
                metadata: Default::default(),
            })
            .collect(),
    )
}

fn headers_schema() -> SchemaType {
    SchemaType::list(record([
        ("name", SchemaType::string()),
        ("value", SchemaType::list(SchemaType::u8())),
    ]))
}

pub fn request_schema() -> SchemaType {
    record([
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
    record([
        ("status", SchemaType::u16()),
        ("headers", headers_schema()),
        (
            "body",
            SchemaType::stream(Some(SchemaType::list(SchemaType::u8()))),
        ),
    ])
}

pub(super) fn validate(agent: &AgentTypeSchema) -> Result<(), String> {
    let mut names = HashSet::new();
    if agent
        .methods
        .iter()
        .any(|method| !names.insert(&method.name))
    {
        return Err("duplicate-method".into());
    }
    if let Some(mount) = &agent.http_mount {
        FileMapping::validate_list(&mount.static_bindings)?;
        FileMapping::validate_list(&mount.filesystem_bindings)?;
    }
    match agent.kind {
        AgentTypeKind::HttpRouter => validate_router(agent),
        AgentTypeKind::Regular => {
            if agent
                .methods
                .iter()
                .flat_map(|method| &method.http_endpoint)
                .any(|endpoint| matches!(endpoint.http_method, HttpMethod::Any(_)))
            {
                return Err("router-method-role".into());
            }
            if let Some(mount) = &agent.http_mount {
                if !mount.static_bindings.is_empty() {
                    return Err("static-owner".into());
                }
                if mount.openapi_provider.is_some() {
                    return Err("provider-owner".into());
                }
                if !mount.filesystem_bindings.is_empty() {
                    validate_filesystem_owner(agent, mount)?;
                }
            }
            Ok(())
        }
    }
}

fn validate_router(agent: &AgentTypeSchema) -> Result<(), String> {
    if agent.mode != AgentMode::Ephemeral {
        return Err("router-mode".into());
    }
    if !agent.constructor.input_schema.fields().is_empty() {
        return Err("router-constructor".into());
    }
    if !matches!(agent.snapshotting, Snapshotting::Disabled(_)) {
        return Err("router-snapshot".into());
    }
    let mount = agent.http_mount.as_ref().ok_or("router-mount")?;
    if !mount.path_prefix.iter().all(|segment| matches!(segment, PathSegment::Literal(literal) if valid_decoded_segment(&literal.value))) {
        return Err("router-mount".into());
    }
    if !mount.filesystem_bindings.is_empty() {
        return Err("filesystem-owner".into());
    }
    if let Some(provider) = &mount.openapi_provider {
        let method = agent
            .methods
            .iter()
            .find(|method| &method.name == provider)
            .ok_or("router-method-role")?;
        if !method.http_endpoint.is_empty() {
            return Err("router-method-role".into());
        }
        if !method.input_schema.fields().is_empty()
            || !output_matches(agent, method, &SchemaType::string())
        {
            return Err("provider-schema".into());
        }
    }
    let mut handler_seen = false;
    for method in &agent.methods {
        if mount.openapi_provider.as_ref() == Some(&method.name) {
            continue;
        }
        if handler_seen || method.http_endpoint.len() != 1 {
            return Err("router-method-role".into());
        }
        handler_seen = true;
        let endpoint = &method.http_endpoint[0];
        if !matches!(endpoint.http_method, HttpMethod::Any(_)) || !endpoint.path_suffix.is_empty() {
            return Err("router-method-role".into());
        }
        if endpoint.auth_details.is_some()
            || !endpoint.cors_options.allowed_patterns.is_empty()
            || !endpoint.header_vars.is_empty()
            || !endpoint.query_vars.is_empty()
        {
            return Err("handler-endpoint-policy".into());
        }
        let mut inputs = method
            .input_schema
            .fields()
            .iter()
            .filter(|field| field.source == FieldSource::UserSupplied);
        let request = inputs.next().ok_or("handler-schema")?;
        if request.name != "request"
            || inputs.next().is_some()
            || !schema_matches(agent, &request.schema, &request_schema())
            || !output_matches(agent, method, &response_schema())
        {
            return Err("handler-schema".into());
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
) -> Result<(), String> {
    if agent.mode != AgentMode::Durable || mount.phantom_agent {
        return Err("filesystem-owner".into());
    }
    let mut captures = HashSet::new();
    for segment in &mount.path_prefix {
        match segment {
            PathSegment::PathVariable(variable)
                if captures.insert(variable.variable_name.as_str()) => {}
            PathSegment::Literal(literal) if valid_decoded_segment(&literal.value) => {}
            PathSegment::SystemVariable(_) => {}
            _ => return Err("unbound-constructor".into()),
        }
    }
    for field in agent.constructor.input_schema.fields() {
        if field.source != FieldSource::UserSupplied || !captures.remove(field.name.as_str()) {
            return Err("unbound-constructor".into());
        }
        let ty = agent
            .schema
            .resolve_ref(&field.schema)
            .map_err(|_| "unbound-constructor")?;
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
            return Err("unbound-constructor".into());
        }
    }
    if !captures.is_empty() {
        return Err("unbound-constructor".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
