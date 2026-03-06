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
    AgentId, AgentType, AgentTypeName, ConfigKeyValueType, ConfigValueType,
};
use golem_common::model::worker::{
    ParsedWorkerCreationLocalAgentConfigEntry, WorkerCreationLocalAgentConfigEntry,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::{Component, LocalAgentConfigEntry};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::ValueAndType;
use std::collections::HashMap;

pub fn ensure_required_agent_secrets_are_configured(
    agent_secrets: &HashMap<Vec<String>, AgentSecret>,
    agent_id: Option<&AgentId>,
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
        let ConfigKeyValueType {
            key,
            value: ConfigValueType::Shared(shared_config_declaration),
        } = config_entry
        else {
            continue;
        };

        match agent_secrets.get(&key) {
            Some(agent_secret) => {
                if agent_secret.secret_type != shared_config_declaration.value {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has invalid type. found: {:?}, expected: {:?}",
                        key.join("."),
                        agent_secret.secret_type,
                        shared_config_declaration.value
                    )));
                }
                if agent_secret.secret_value.is_none()
                    && !matches!(agent_secret.secret_type, AnalysedType::Option(_))
                {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Required agent secret {} has no configured value",
                        key.join(".")
                    )));
                }
            }
            None if matches!(shared_config_declaration.value, AnalysedType::Option(_)) => {}
            None => {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Required agent secret {} does not exist",
                    key.join(".")
                )))
            }
        }
    }

    Ok(())
}

pub fn parse_worker_creation_local_agent_config(
    worker_local_agent_config: Vec<WorkerCreationLocalAgentConfigEntry>,
    agent_id: Option<&AgentId>,
    component: &Component,
) -> Result<Vec<ParsedWorkerCreationLocalAgentConfigEntry>, WorkerExecutorError> {
    let Some(agent_id) = agent_id else {
        return Ok(Vec::new());
    };

    let agent_type = component
        .metadata
        .find_agent_type_by_name(&agent_id.agent_type)
        .expect("Agent metadata for the parsed agent type was not part of component metadata");

    let mut initial_local_agent_config = Vec::new();

    for entry in worker_local_agent_config {
        let config_key_type = agent_type
            .config
            .iter()
            .find_map(|c| match c {
                ConfigKeyValueType {
                    key,
                    value: ConfigValueType::Local(inner),
                } if *key == *entry.key => Some(&inner.value),
                _ => None,
            })
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type does not declare config key {}",
                    entry.key.join(".")
                ))
            })?;

        let parsed_value =
            ValueAndType::parse_with_type(&entry.value, config_key_type).map_err(|err| {
                WorkerExecutorError::invalid_request(format!(
                    "config value for key {} does not match expected schema: [{}]",
                    entry.key.join("."),
                    err.join(", ")
                ))
            })?;

        initial_local_agent_config.push(ParsedWorkerCreationLocalAgentConfigEntry {
            key: entry.key,
            value: parsed_value,
        });
    }

    // The actual loading of the local agent config happens in the DurableWorkerCtx, but
    // we also compute it here during creation to allow failing a creation request before any metdata has
    // been written / oplog has been created.
    let local_agent_config = effective_local_agent_config(
        initial_local_agent_config.clone(),
        component.local_agent_config.clone(),
        &agent_type.type_name,
    );

    validate_local_agent_config(&local_agent_config, &agent_type)?;

    Ok(initial_local_agent_config)
}

pub fn effective_local_agent_config(
    worker_local_agent_config: Vec<ParsedWorkerCreationLocalAgentConfigEntry>,
    component_local_agent_config: Vec<LocalAgentConfigEntry>,
    agent_type: &AgentTypeName,
) -> HashMap<Vec<String>, golem_wasm::ValueAndType> {
    let mut result = HashMap::new();

    let applicable_component_local_agent_config = component_local_agent_config
        .into_iter()
        .filter(|lac| lac.agent == *agent_type);

    for entry in applicable_component_local_agent_config {
        result.insert(entry.key, entry.value);
    }

    for entry in worker_local_agent_config {
        result.insert(entry.key, entry.value);
    }

    result
}

pub fn validate_local_agent_config(
    local_agent_config: &HashMap<Vec<String>, golem_wasm::ValueAndType>,
    agent_type: &AgentType,
) -> Result<(), WorkerExecutorError> {
    for entry in &agent_type.config {
        if let ConfigValueType::Local(config_declaration) = &entry.value {
            match local_agent_config.get(&entry.key) {
                Some(config_value) => {
                    if config_value.typ != config_declaration.value {
                        // TODO: better rendering of analysed type.
                        return Err(WorkerExecutorError::invalid_request(format!(
                            "Type mismatch for config key {}. expected: {:?}; found: {:?}",
                            entry.key.join("."),
                            config_declaration.value,
                            config_value.typ
                        )));
                    }
                }
                None if matches!(config_declaration.value, AnalysedType::Option(_)) => {}
                None => {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "Config key {} was not provided a value",
                        entry.key.join(".")
                    )))
                }
            }
        }
    }

    Ok(())
}
