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
use crate::model::cli_output::StructuredOutput;
use anyhow::{anyhow, bail};
use golem_client::model::AgentInvocationResult;
use golem_common::model::IdempotencyKey;
use golem_common::model::agent::{AgentType, DataSchema, DataValue, ElementValue};
use golem_common::schema::adapters::value_and_type_to_typed_schema_value;
use golem_wasm::analysis::{AnalysedType, TypeTuple};
use golem_wasm::{Value, ValueAndType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeResultView {
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_json: Option<ValueAndType>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub results_json: Option<Vec<ValueAndType>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_format: Option<String>,
    #[serde(skip)]
    pub is_void_result: bool,
}

impl StructuredOutput for InvokeResultView {
    const KIND: &'static str = "agent.invoke";
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

        let (is_void_result, result_values) =
            match Self::try_get_agent_results(&result, agent_type, method_name) {
                Ok(r) => r,
                Err(err) => {
                    log_error(format!("{err}"));
                    (false, vec![])
                }
            };

        let (result_json, results_json, rendered_result, result_format) = match result_values.len()
        {
            0 => (None, None, None, None),
            1 => {
                let result_json = result_values.into_iter().next().expect("checked length");
                let rendered_result = match value_and_type_to_typed_schema_value(&result_json) {
                    Ok(typed) => render_typed_schema_value(&typed, &source_language),
                    Err(err) => format!("<rendering error: {err}>"),
                };
                (
                    Some(result_json),
                    None,
                    Some(rendered_result),
                    Some(result_format),
                )
            }
            _ => {
                let rendered_result =
                    Self::render_multiple_results(&result_values, &source_language);
                (
                    None,
                    Some(result_values),
                    Some(rendered_result),
                    Some(result_format),
                )
            }
        };

        Self {
            idempotency_key: idempotency_key.value,
            result_json,
            results_json,
            result: rendered_result,
            result_format,
            is_void_result,
        }
    }

    pub fn new_trigger(idempotency_key: IdempotencyKey) -> Self {
        Self {
            idempotency_key: idempotency_key.value,
            result_json: None,
            results_json: None,
            result: None,
            result_format: None,
            is_void_result: false,
        }
    }

    fn try_get_agent_results(
        result: &AgentInvocationResult,
        agent_type: &AgentType,
        method_name: &str,
    ) -> anyhow::Result<(bool, Vec<ValueAndType>)> {
        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == method_name)
            .ok_or_else(|| anyhow!("Method '{method_name}' not found in agent type"))?;

        let output_schemas = match &method.output_schema {
            DataSchema::Tuple(schemas) => &schemas.elements,
            _ => bail!("Non-tuple output schema not supported for result rendering"),
        };

        if output_schemas.is_empty() {
            return Ok((true, vec![]));
        }

        let Some(ref untyped) = result.result else {
            return Ok((false, vec![]));
        };

        let data_value =
            DataValue::try_from_untyped_json(untyped.clone(), method.output_schema.clone())
                .map_err(|e| anyhow!("Failed to parse agent result: {e}"))?;

        let DataValue::Tuple(elements) = data_value else {
            bail!("Non-tuple agent result not supported for result rendering");
        };

        let values = elements
            .elements
            .into_iter()
            .map(|element| match element {
                ElementValue::ComponentModel(component_model) => Ok(component_model.value),
                _ => bail!("Non-ComponentModel output schema not supported for result rendering"),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok((false, values))
    }

    fn render_multiple_results(
        results: &[ValueAndType],
        source_language: &SourceLanguage,
    ) -> String {
        let value = Value::Tuple(results.iter().map(|result| result.value.clone()).collect());
        let typ = AnalysedType::Tuple(TypeTuple {
            name: None,
            owner: None,
            items: results.iter().map(|result| result.typ.clone()).collect(),
        });
        let value_and_type = ValueAndType::new(value, typ);
        match value_and_type_to_typed_schema_value(&value_and_type) {
            Ok(typed) => render_typed_schema_value(&typed, source_language),
            Err(err) => format!("<rendering error: {err}>"),
        }
    }
}
