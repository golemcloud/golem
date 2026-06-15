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

use golem_common::model::agent::{AgentConfigSource, LegacyParsedAgentId};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::worker::{AgentConfigEntryDto, TypedAgentConfigEntry};
use golem_common::schema::AgentTypeSchema;
use golem_common::schema::adapters::analysed_type::{
    schema_graph_to_analysed_type, schema_type_to_analysed_type,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::Component;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use std::collections::HashMap;

pub fn ensure_required_agent_secrets_are_configured(
    agent_secrets: &HashMap<CanonicalAgentSecretPath, AgentSecret>,
    agent_id: Option<&LegacyParsedAgentId>,
    component: &Component,
) -> Result<(), WorkerExecutorError> {
    let Some(agent_id) = agent_id else {
        return Ok(());
    };

    let agent_type = component
        .metadata
        .find_agent_type_by_name(&agent_id.agent_type)
        .expect("Agent metadata for the parsed agent type was not part of component metadata");

    for config_entry in agent_type.config {
        if config_entry.source != AgentConfigSource::Secret {
            continue;
        }

        let canonical_agent_secret_path =
            CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_entry.path);

        let expected_secret_type = schema_type_to_analysed_type(
            &agent_type.schema,
            &config_entry.value_type,
        )
        .map_err(|e| {
            WorkerExecutorError::runtime(format!(
                "Declared secret config type for {} is not representable as AnalysedType: {e}",
                config_entry.path.join(".")
            ))
        })?;

        match agent_secrets.get(&canonical_agent_secret_path) {
            Some(agent_secret) => {
                let secret_type_legacy = schema_graph_to_analysed_type(&agent_secret.secret_type)
                    .map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Required agent secret {} has a type that is not representable as AnalysedType: {e}",
                        config_entry.path.join(".")
                    ))
                })?;
                if secret_type_legacy != expected_secret_type {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has invalid type. found: {:?}, expected: {:?}",
                        config_entry.path.join("."),
                        secret_type_legacy,
                        expected_secret_type
                    )));
                }
                if agent_secret.secret_value.is_none()
                    && !matches!(secret_type_legacy, AnalysedType::Option(_))
                {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has no configured value",
                        config_entry.path.join(".")
                    )));
                }
            }
            None if matches!(expected_secret_type, AnalysedType::Option(_)) => {}
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
    agent_id: Option<&LegacyParsedAgentId>,
    component: &Component,
) -> Result<Vec<TypedAgentConfigEntry>, WorkerExecutorError> {
    let Some(agent_id) = agent_id else {
        return Ok(Vec::new());
    };

    let agent_type = component
        .metadata
        .find_agent_type_by_name(&agent_id.agent_type)
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

        let config_declaration_type =
            schema_type_to_analysed_type(&agent_type.schema, &config_declaration.value_type)
                .map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Declared config type for {} is not representable as AnalysedType: {e}",
                        entry.path.join(".")
                    ))
                })?;

        let parsed_value = ValueAndType::parse_with_type(&entry.value.0, &config_declaration_type)
            .map_err(|err| {
                WorkerExecutorError::invalid_request(format!(
                    "config value for path {} does not match expected schema: [{}]",
                    entry.path.join("."),
                    err.join(", ")
                ))
            })?;

        initial_agent_config.push(TypedAgentConfigEntry {
            path: entry.path,
            value: parsed_value,
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

        let config = effective_agent_config(initial_agent_config.clone(), component_config);

        validate_agent_config(&config, &agent_type)?;
    }

    Ok(initial_agent_config)
}

/// Merges the component-level typed config (stored in `AgentTypeProvisionConfig`) with
/// the worker-creation config entries, with worker entries taking precedence.
/// Returns a map from config path to `ValueAndType`.
pub fn effective_agent_config(
    config: Vec<TypedAgentConfigEntry>,
    default_agent_config: Vec<TypedAgentConfigEntry>,
) -> HashMap<Vec<String>, ValueAndType> {
    let mut result = HashMap::new();

    for entry in default_agent_config {
        result.insert(entry.path, entry.value);
    }

    for entry in config {
        result.insert(entry.path, entry.value);
    }

    result
}

pub fn validate_agent_config(
    config: &HashMap<Vec<String>, ValueAndType>,
    agent_type: &AgentTypeSchema,
) -> Result<(), WorkerExecutorError> {
    for entry in &agent_type.config {
        if entry.source != AgentConfigSource::Local {
            continue;
        };

        let entry_value_type = schema_type_to_analysed_type(&agent_type.schema, &entry.value_type)
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Declared config type for {} is not representable as AnalysedType: {e}",
                    entry.path.join(".")
                ))
            })?;

        match config.get(&entry.path) {
            Some(config_value) => {
                if config_value.typ != entry_value_type {
                    // TODO: better rendering of analysed type.
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Type mismatch for config {}. expected: {:?}; found: {:?}",
                        entry.path.join("."),
                        entry_value_type,
                        config_value.typ
                    )));
                }
            }
            None if matches!(entry_value_type, AnalysedType::Option(_)) => {}
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
