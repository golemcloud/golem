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

use crate::agent_id_display::{SourceLanguage, render_typed_schema_value};
use crate::log::log_error;
use anyhow::anyhow;
use golem_client::model::AgentInvocationResult;
use golem_common::model::IdempotencyKey;
use golem_common::model::agent::{AgentType, DataSchema};
use golem_common::schema::TypedSchemaValue;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvokeResultView {
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_json: Option<TypedSchemaValue>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_format: Option<String>,
    #[serde(skip)]
    pub is_void_result: bool,
}

impl InvokeResultView {
    pub fn new_agent_invoke(
        idempotency_key: IdempotencyKey,
        result: AgentInvocationResult,
        agent_type: &AgentType,
        method_name: &str,
    ) -> Self {
        let source_language = SourceLanguage::from(agent_type.source_language.as_str());

        let result_format = match &source_language {
            SourceLanguage::Rust => "Rust syntax",
            SourceLanguage::TypeScript => "TypeScript syntax",
            SourceLanguage::Scala => "Scala syntax",
            SourceLanguage::MoonBit => "MoonBit syntax",
            SourceLanguage::Other(_) => "fallback TypeScript syntax",
        }
        .to_string();

        let (is_void_result, result_value) =
            match Self::try_get_agent_results(&result, agent_type, method_name) {
                Ok(r) => r,
                Err(err) => {
                    log_error(format!("{err}"));
                    (false, None)
                }
            };

        let (result_json, rendered_result, result_format) = match result_value {
            None => (None, None, None),
            Some(typed) => {
                let rendered_result = render_typed_schema_value(&typed, &source_language);
                (Some(typed), Some(rendered_result), Some(result_format))
            }
        };

        Self {
            idempotency_key: idempotency_key.value,
            result_json,
            result: rendered_result,
            result_format,
            is_void_result,
        }
    }

    pub fn new_trigger(idempotency_key: IdempotencyKey) -> Self {
        Self {
            idempotency_key: idempotency_key.value,
            result_json: None,
            result: None,
            result_format: None,
            is_void_result: false,
        }
    }

    fn try_get_agent_results(
        result: &AgentInvocationResult,
        agent_type: &AgentType,
        method_name: &str,
    ) -> anyhow::Result<(bool, Option<TypedSchemaValue>)> {
        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == method_name)
            .ok_or_else(|| anyhow!("Method '{method_name}' not found in agent type"))?;

        if matches!(&method.output_schema, DataSchema::Tuple(schemas) if schemas.elements.is_empty())
        {
            return Ok((true, None));
        }

        let Some(ref typed) = result.result else {
            return Ok((false, None));
        };

        let typed = serde_json::from_value(typed.0.clone())
            .map_err(|e| anyhow!("Failed to parse typed agent result: {e}"))?;

        Ok((false, Some(typed)))
    }
}
