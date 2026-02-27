// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::log::log_error;
use anyhow::{anyhow, bail};
use golem_client::model::AgentInvocationResult;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{AgentType, DataSchema, DataValue, ElementSchema};
use golem_common::model::IdempotencyKey;
use golem_wasm::{print_value_and_type, ValueAndType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvokeResultView {
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_json: Option<ValueAndType>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_wave: Option<Vec<String>>,
}

impl InvokeResultView {
    pub fn new_agent_invoke(
        idempotency_key: IdempotencyKey,
        result: AgentInvocationResult,
        agent_type: &AgentType,
        method_name: &str,
    ) -> Self {
        let wave = match Self::try_parse_agent_wave(&result, agent_type, method_name) {
            Ok(wave) => Some(wave),
            Err(err) => {
                log_error(format!("{err}"));
                None
            }
        };

        let result_json = result.result.and_then(|untyped| {
            let method = agent_type.methods.iter().find(|m| m.name == method_name)?;
            let data_value =
                match DataValue::try_from_untyped_json(untyped, method.output_schema.clone()) {
                    Ok(dv) => dv,
                    Err(err) => {
                        log_error(format!("Failed to parse agent result: {err}"));
                        return None;
                    }
                };
            let value = match data_value.into_return_value() {
                Some(v) => v,
                None => {
                    log_error("Agent result is not a single return value");
                    return None;
                }
            };
            let output_schemas = match &method.output_schema {
                DataSchema::Tuple(schemas) => &schemas.elements,
                _ => {
                    log_error("Non-tuple output schema not supported for result display");
                    return None;
                }
            };
            let first_schema = match output_schemas.first() {
                Some(s) => s,
                None => {
                    log_error("Empty output schema");
                    return None;
                }
            };
            let analysed_type = match &first_schema.schema {
                ElementSchema::ComponentModel(cm) => cm.element_type.to_wit_naming(),
                _ => {
                    log_error("Non-ComponentModel output schema not supported for result display");
                    return None;
                }
            };
            Some(ValueAndType::new(value, analysed_type))
        });

        Self {
            idempotency_key: idempotency_key.value,
            result_json,
            result_wave: wave,
        }
    }

    pub fn new_trigger(idempotency_key: IdempotencyKey) -> Self {
        Self {
            idempotency_key: idempotency_key.value,
            result_json: None,
            result_wave: None,
        }
    }

    fn try_parse_agent_wave(
        result: &AgentInvocationResult,
        agent_type: &AgentType,
        method_name: &str,
    ) -> anyhow::Result<Vec<String>> {
        let Some(ref untyped) = result.result else {
            return Ok(vec![]);
        };

        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == method_name)
            .ok_or_else(|| anyhow!("Method '{method_name}' not found in agent type"))?;

        let data_value =
            DataValue::try_from_untyped_json(untyped.clone(), method.output_schema.clone())
                .map_err(|e| anyhow!("Failed to parse agent result: {e}"))?;

        let Some(value) = data_value.into_return_value() else {
            return Ok(vec![]);
        };

        let output_schemas = match &method.output_schema {
            DataSchema::Tuple(schemas) => &schemas.elements,
            _ => bail!("Non-tuple output schema not supported for WAVE formatting"),
        };

        let first_schema = output_schemas
            .first()
            .ok_or_else(|| anyhow!("Empty output schema"))?;

        let analysed_type = match &first_schema.schema {
            ElementSchema::ComponentModel(cm) => cm.element_type.to_wit_naming(),
            _ => bail!("Non-ComponentModel output schema not supported for WAVE formatting"),
        };

        let vt = ValueAndType::new(value, analysed_type);
        Ok(vec![Self::try_wave_format(vt)?])
    }

    fn try_wave_format(parsed: ValueAndType) -> anyhow::Result<String> {
        match print_value_and_type(&parsed) {
            Ok(res) => Ok(res),
            Err(err) => Err(anyhow!("Failed to format parsed value as wave: {err}")),
        }
    }
}
