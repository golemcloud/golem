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

use golem_common::model::agent::{AgentConfigSource, AgentType, AgentTypeName, ParsedAgentId};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::worker::{ParsedWorkerAgentConfigEntry, WorkerAgentConfigEntry};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::{AgentConfigEntry, Component};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::ValueAndType;
use std::collections::HashMap;

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
        .find_agent_type_by_name(&agent_id.agent_type)
        .expect("Agent metadata for the parsed agent type was not part of component metadata");

    for config_entry in agent_type.config {
        if config_entry.source != AgentConfigSource::Secret {
            continue;
        }

        let canonical_agent_secret_path =
            CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_entry.path);

        match agent_secrets.get(&canonical_agent_secret_path) {
            Some(agent_secret) => {
                if agent_secret.secret_type != config_entry.value_type {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has invalid type. found: {:?}, expected: {:?}",
                        config_entry.path.join("."),
                        agent_secret.secret_type,
                        config_entry.value_type
                    )));
                }
                if agent_secret.secret_value.is_none()
                    && !matches!(agent_secret.secret_type, AnalysedType::Option(_))
                {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has no configured value",
                        config_entry.path.join(".")
                    )));
                }
            }
            None if matches!(config_entry.value_type, AnalysedType::Option(_)) => {}
            None => {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Required agent secret {} does not exist",
                    config_entry.path.join(".")
                )))
            }
        }
    }

    Ok(())
}

pub fn parse_worker_creation_agent_config(
    worker_agent_config: Vec<WorkerAgentConfigEntry>,
    agent_id: Option<&ParsedAgentId>,
    component: &Component,
) -> Result<Vec<ParsedWorkerAgentConfigEntry>, WorkerExecutorError> {
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

        let parsed_value =
            ValueAndType::parse_with_type(&entry.value, &config_declaration.value_type).map_err(
                |err| {
                    WorkerExecutorError::invalid_request(format!(
                        "config value for path {} does not match expected schema: [{}]",
                        entry.path.join("."),
                        err.join(", ")
                    ))
                },
            )?;

        initial_agent_config.push(ParsedWorkerAgentConfigEntry {
            path: entry.path,
            value: parsed_value,
        });
    }

    {
        // The actual loading of the local agent config happens in the DurableWorkerCtx, but
        // we also compute it here during creation to allow failing a creation request before any metdata has
        // been written / oplog has been created.
        let agent_config = effective_agent_config(
            initial_agent_config.clone(),
            component.agent_config.clone(),
            &agent_type.type_name,
        );

        validate_agent_config(&agent_config, &agent_type)?;
    }

    Ok(initial_agent_config)
}

pub fn effective_agent_config(
    worker_agent_config: Vec<ParsedWorkerAgentConfigEntry>,
    component_agent_config: Vec<AgentConfigEntry>,
    agent_type: &AgentTypeName,
) -> HashMap<Vec<String>, golem_wasm::ValueAndType> {
    let mut result = HashMap::new();

    let applicable_component_agent_config = component_agent_config
        .into_iter()
        .filter(|lac| lac.agent == *agent_type);

    for entry in applicable_component_agent_config {
        result.insert(entry.path, entry.value);
    }

    for entry in worker_agent_config {
        result.insert(entry.path, entry.value);
    }

    result
}

pub fn validate_agent_config(
    agent_config: &HashMap<Vec<String>, golem_wasm::ValueAndType>,
    agent_type: &AgentType,
) -> Result<(), WorkerExecutorError> {
    for entry in &agent_type.config {
        if entry.source != AgentConfigSource::Local {
            continue;
        };

        match agent_config.get(&entry.path) {
            Some(config_value) => {
                if config_value.typ != entry.value_type {
                    // TODO: better rendering of analysed type.
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Type mismatch for config {}. expected: {:?}; found: {:?}",
                        entry.path.join("."),
                        entry.value_type,
                        config_value.typ
                    )));
                }
            }
            None if matches!(entry.value_type, AnalysedType::Option(_)) => {}
            None => {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Config {} was not provided a value",
                    entry.path.join(".")
                )))
            }
        }
    }

    Ok(())
}
