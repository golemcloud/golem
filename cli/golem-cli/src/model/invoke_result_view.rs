// Copyright 2024-2025 Golem Cloud
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

use crate::model::component::{function_result_types, Component};
use crate::model::text::fmt::log_error;
use crate::model::wave::type_wave_compatible;
use crate::model::IdempotencyKey;
use anyhow::{anyhow, bail};
use golem_client::model::{InvokeResult, TypeAnnotatedValue};
use golem_wasm_rpc::{print_type_annotated_value, protobuf};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvokeResultView {
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_json: Option<TypeAnnotatedValue>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result_wave: Option<Vec<String>>,
}

impl InvokeResultView {
    pub fn new_invoke(
        idempotency_key: IdempotencyKey,
        result: InvokeResult,
        component: &Component,
        function: &str,
    ) -> Self {
        let wave = match Self::try_parse_wave(&result.result, component, function) {
            Ok(wave) => Some(wave),
            Err(err) => {
                log_error(format!("{}", err));
                None
            }
        };

        Self {
            idempotency_key: idempotency_key.0,
            result_json: Some(result.result),
            result_wave: wave,
        }
    }

    pub fn new_enqueue(idempotency_key: IdempotencyKey) -> Self {
        Self {
            idempotency_key: idempotency_key.0,
            result_json: None,
            result_wave: None,
        }
    }

    fn try_parse_wave(
        result: &TypeAnnotatedValue,
        component: &Component,
        function: &str,
    ) -> anyhow::Result<Vec<String>> {
        let results = match result {
            TypeAnnotatedValue::Tuple(tuple) => tuple
                .value
                .iter()
                .map(|t| t.clone().type_annotated_value.unwrap())
                .collect::<Vec<_>>(),
            // TODO: need to support multi-result case when it's a Record
            _ => {
                bail!("Can't parse InvokeResult - tuple expected.");
            }
        };

        // TODO: we don't need this, as the result is always a TypeAnnotatedValue
        let result_types = function_result_types(component, function)?;

        if results.len() != result_types.len() {
            bail!("Unexpected number of results.".to_string());
        }

        if !result_types.iter().all(|typ| type_wave_compatible(typ)) {
            bail!("Result type is not supported by wave".to_string(),);
        }

        let wave = results
            .into_iter()
            .map(Self::try_wave_format)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(wave)
    }

    fn try_wave_format(
        parsed: protobuf::type_annotated_value::TypeAnnotatedValue,
    ) -> anyhow::Result<String> {
        match print_type_annotated_value(&parsed) {
            Ok(res) => Ok(res),
            Err(err) => Err(anyhow!("Failed to format parsed value as wave: {err}")),
        }
    }
}
