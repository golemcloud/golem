// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use golem_common::model::agent::{AgentConfigSource, ParsedAgentId};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::worker::{AgentConfigEntryDto, TypedAgentConfigEntry};
use golem_common::schema::agent::typed_schema_value_with_projected_defs;
use golem_common::schema::render::from_json_value;
use golem_common::schema::schema_type::SecretSpec;
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
use golem_common::schema::{
    AgentTypeSchema, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::Component;
use std::collections::HashMap;

/// Resolve a chain of [`SchemaType::Ref`]s into a non-`Ref` type, with a bounded
/// loop guarding against reference cycles.
fn resolve_type<'a>(graph: &'a SchemaGraph, ty: &'a SchemaType) -> &'a SchemaType {
    let mut current = ty;
    for _ in 0..256 {
        match current {
            SchemaType::Ref { id, .. } => match graph.lookup(id) {
                Some(def) => current = &def.body,
                None => break,
            },
            _ => break,
        }
    }
    current
}

/// Whether `ty` (resolving refs in `graph`) is an `option<_>` type, i.e. a value
/// is not required to be present.
fn is_optional_type(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(resolve_type(graph, ty), SchemaType::Option { .. })
}

fn secret_config_payload_type<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Option<(bool, &'a SecretSpec)> {
    match resolve_type(graph, ty) {
        SchemaType::Option { inner, .. } => match resolve_type(graph, inner) {
            SchemaType::Secret { spec, .. } => Some((true, spec)),
            _ => None,
        },
        SchemaType::Secret { spec, .. } => Some((false, spec)),
        _ => None,
    }
}

pub fn ensure_required_agent_secrets_are_configured(
    agent_secrets: &HashMap<CanonicalAgentSecretPath, AgentSecret>,
    agent_id: Option<&ParsedAgentId>,
    component: &Component,
) -> Result<(), WorkerExecutorError> {
    let Some(agent_id) = agent_id else {
        return Ok(());
    };

    let agent_type = component
        .metadata
        .find_agent_type_by_name_ref(&agent_id.agent_type)
        .expect("Agent metadata for the parsed agent type was not part of component metadata");

    for config_entry in &agent_type.config {
        if config_entry.source != AgentConfigSource::Secret {
            continue;
        }

        let canonical_agent_secret_path =
            CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_entry.path);

        // The declared type's refs resolve against the agent's shared `defs`, so
        // pass the agent graph plus the borrowed `value_type` directly instead of
        // materializing a per-entry graph that clones the whole `defs` registry.
        let declared_graph = &agent_type.schema;
        let declared_type = &config_entry.value_type;
        let Some((declared_optional, declared_secret_spec)) =
            secret_config_payload_type(declared_graph, declared_type)
        else {
            return Err(WorkerExecutorError::invalid_request(format!(
                "Required agent secret {} has invalid declaration type",
                config_entry.path.join(".")
            )));
        };

        match agent_secrets.get(&canonical_agent_secret_path) {
            Some(agent_secret) => {
                let secret_graph = &agent_secret.secret_type;
                if !is_equivalent_cross_graph(
                    secret_graph,
                    &secret_graph.root,
                    declared_graph,
                    &declared_secret_spec.inner,
                ) {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has invalid type",
                        config_entry.path.join(".")
                    )));
                }
                if let Some(secret_value) = &agent_secret.secret_value {
                    validate_value(secret_graph, &secret_graph.root, secret_value).map_err(
                        |errors| {
                            WorkerExecutorError::invalid_request(format!(
                                "Required agent secret {} has invalid value: {}",
                                config_entry.path.join("."),
                                errors
                                    .into_iter()
                                    .map(|error| error.to_string())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            ))
                        },
                    )?;
                }
                if agent_secret.secret_value.is_none() && !declared_optional {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has no configured value",
                        config_entry.path.join(".")
                    )));
                }
            }
            None if declared_optional => {}
            None => {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Required agent secret {} does not exist",
                    config_entry.path.join(".")
                )));
            }
        }
    }

    Ok(())
}

pub fn parse_worker_creation_agent_config(
    worker_agent_config: Vec<AgentConfigEntryDto>,
    agent_id: Option<&ParsedAgentId>,
    component: &Component,
) -> Result<Vec<TypedAgentConfigEntry>, WorkerExecutorError> {
    let Some(agent_id) = agent_id else {
        return Ok(Vec::new());
    };

    let agent_type = component
        .metadata
        .find_agent_type_by_name_ref(&agent_id.agent_type)
        .expect("Agent metadata for the parsed agent type was not part of component metadata");

    let mut initial_agent_config = Vec::new();

    for entry in worker_agent_config {
        let config_declaration = agent_type
            .config
            .iter()
            .find(|c| c.source == AgentConfigSource::Local && c.path == entry.path)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type does not declare local config {}",
                    entry.path.join(".")
                ))
            })?;

        // Decode + validate against the agent's shared graph and the declared
        // `value_type` (refs resolve through the agent's `defs`).
        let declared_type = &config_declaration.value_type;

        let schema_value: SchemaValue =
            from_json_value(&agent_type.schema, declared_type, &entry.value.0).map_err(|err| {
                WorkerExecutorError::invalid_request(format!(
                    "config value for path {} is not a valid schema value: {err}",
                    entry.path.join(".")
                ))
            })?;

        validate_value(&agent_type.schema, declared_type, &schema_value).map_err(|errors| {
            WorkerExecutorError::invalid_request(format!(
                "config value for path {} does not match expected schema: [{}]",
                entry.path.join("."),
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;

        // The stored entry is a single-root carrier, so project the agent
        // graph's defs to exactly those reachable from `value_type` instead of
        // cloning the whole registry.
        let value = typed_schema_value_with_projected_defs(
            &agent_type.schema,
            config_declaration.value_type.clone(),
            schema_value,
        );

        initial_agent_config.push(TypedAgentConfigEntry {
            path: entry.path,
            value,
        });
    }

    {
        // The actual loading of the local agent config happens in the DurableWorkerCtx, but
        // we also compute it here during creation to allow failing a creation request before any
        // metadata has been written / oplog has been created.
        let component_config = component
            .metadata
            .agent_type_config(&agent_id.agent_type)
            .map(|s| s.to_vec())
            .unwrap_or_default();

        let config = effective_agent_config(initial_agent_config.clone(), component_config)?;

        validate_agent_config(&config, agent_type)?;
    }

    Ok(initial_agent_config)
}

/// Merges the component-level typed config (stored in `AgentTypeProvisionConfig`) with
/// the worker-creation config entries, with worker entries taking precedence.
///
/// The result is the schema-native [`TypedSchemaValue`] carried by
/// [`TypedAgentConfigEntry`] keyed by config path; it is what the executor's
/// guest-facing config plumbing (`wasi:config/store`, named retry-policy
/// parsing) consumes.
pub fn effective_agent_config(
    config: Vec<TypedAgentConfigEntry>,
    default_agent_config: Vec<TypedAgentConfigEntry>,
) -> Result<HashMap<Vec<String>, TypedSchemaValue>, WorkerExecutorError> {
    let mut result: HashMap<Vec<String>, TypedSchemaValue> = HashMap::new();

    for entry in default_agent_config {
        result.insert(entry.path, entry.value);
    }

    for entry in config {
        result.insert(entry.path, entry.value);
    }

    Ok(result)
}

pub fn validate_agent_config(
    config: &HashMap<Vec<String>, TypedSchemaValue>,
    agent_type: &AgentTypeSchema,
) -> Result<(), WorkerExecutorError> {
    for entry in &agent_type.config {
        if entry.source != AgentConfigSource::Local {
            continue;
        };

        // Refs in the declared `value_type` resolve against the agent's shared
        // `defs`; pass the agent graph plus the borrowed type directly instead of
        // cloning the whole `defs` registry into a per-entry graph.
        let declared_graph = &agent_type.schema;
        let declared_type = &entry.value_type;

        match config.get(&entry.path) {
            Some(config_value) => {
                validate_value(declared_graph, declared_type, config_value.value()).map_err(
                    |errors| {
                        WorkerExecutorError::invalid_request(format!(
                            "Type mismatch for config {}: [{}]",
                            entry.path.join("."),
                            errors
                                .iter()
                                .map(|e| e.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    },
                )?;
            }
            None if is_optional_type(declared_graph, declared_type) => {}
            None => {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Config {} was not provided a value",
                    entry.path.join(".")
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{AgentMode, AgentTypeName, ParsedAgentId, Snapshotting};
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::diff::Hash;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::schema::agent::{
        AgentConfigDeclarationSchema, AgentConstructorSchema, InputSchema,
    };
    use golem_common::schema::schema_type::SecretSpec;
    use golem_service_base::model::agent_secret::AgentSecret;
    use golem_service_base::model::component::Component;
    use std::collections::{BTreeMap, HashMap};
    use test_r::test;

    #[test]
    fn secret_backed_config_accepts_plaintext_payload_type_for_secret_declaration() {
        let agent_type_name = AgentTypeName("vault".to_string());
        let config_path = vec!["apiKey".to_string()];
        let secret_type = SchemaGraph::anonymous(SchemaType::string());
        let agent_type = AgentTypeSchema {
            type_name: agent_type_name.clone(),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
            },
            methods: Vec::new(),
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![AgentConfigDeclarationSchema {
                source: AgentConfigSource::Secret,
                path: config_path.clone(),
                value_type: SchemaType::secret(SecretSpec {
                    inner: Box::new(SchemaType::string()),
                    category: None,
                }),
            }],
        };

        let metadata = ComponentMetadata::from_parts(
            Default::default(),
            Vec::new(),
            None,
            None,
            vec![agent_type],
            BTreeMap::new(),
        );
        let component = Component {
            id: ComponentId::new(),
            revision: ComponentRevision::INITIAL,
            environment_id: EnvironmentId::new(),
            component_name: ComponentName("component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            application_name: ApplicationName("app".to_string()),
            environment_name: EnvironmentName::try_from("dev").unwrap(),
            component_size: 0,
            metadata,
            created_at: chrono::Utc::now(),
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        };
        let agent_id = ParsedAgentId::new(
            agent_type_name,
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(Vec::new())),
                SchemaValue::Record { fields: vec![] },
            ),
            None,
        );
        let mut agent_secrets = HashMap::new();
        agent_secrets.insert(
            CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_path),
            AgentSecret {
                id: AgentSecretId::new(),
                environment_id: EnvironmentId::new(),
                path: CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_path),
                revision: AgentSecretRevision::INITIAL,
                secret_type,
                secret_value: Some(SchemaValue::String("present".to_string())),
            },
        );

        ensure_required_agent_secrets_are_configured(&agent_secrets, Some(&agent_id), &component)
            .expect("secret<T> config should accept a matching stored plaintext T");
    }

    #[test]
    fn secret_backed_config_rejects_invalid_stored_secret_value() {
        let agent_type_name = AgentTypeName("vault".to_string());
        let config_path = vec!["apiKey".to_string()];
        let secret_type = SchemaGraph::anonymous(SchemaType::string());
        let agent_type = AgentTypeSchema {
            type_name: agent_type_name.clone(),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
            },
            methods: Vec::new(),
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![AgentConfigDeclarationSchema {
                source: AgentConfigSource::Secret,
                path: config_path.clone(),
                value_type: SchemaType::secret(SecretSpec {
                    inner: Box::new(SchemaType::string()),
                    category: None,
                }),
            }],
        };

        let metadata = ComponentMetadata::from_parts(
            Default::default(),
            Vec::new(),
            None,
            None,
            vec![agent_type],
            BTreeMap::new(),
        );
        let component = Component {
            id: ComponentId::new(),
            revision: ComponentRevision::INITIAL,
            environment_id: EnvironmentId::new(),
            component_name: ComponentName("component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            application_name: ApplicationName("app".to_string()),
            environment_name: EnvironmentName::try_from("dev").unwrap(),
            component_size: 0,
            metadata,
            created_at: chrono::Utc::now(),
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        };
        let agent_id = ParsedAgentId::new(
            agent_type_name,
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(Vec::new())),
                SchemaValue::Record { fields: vec![] },
            ),
            None,
        );
        let mut agent_secrets = HashMap::new();
        agent_secrets.insert(
            CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_path),
            AgentSecret {
                id: AgentSecretId::new(),
                environment_id: EnvironmentId::new(),
                path: CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_path),
                revision: AgentSecretRevision::INITIAL,
                secret_type,
                secret_value: Some(SchemaValue::Bool(true)),
            },
        );

        let result = ensure_required_agent_secrets_are_configured(
            &agent_secrets,
            Some(&agent_id),
            &component,
        );
        assert!(
            result
                .expect_err(
                    "invalid stored plaintext must be rejected before minting a secret handle"
                )
                .to_string()
                .contains("invalid value")
        );
    }
}
