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

use golem_wasm_rpc::{print_type_annotated_value, protobuf};
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use tracing::{debug, info};

use golem_client::model::InvokeResult;

use crate::model::component::{function_result_types, Component};
use crate::model::wave::type_wave_compatible;
use crate::model::GolemError;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum InvokeResultView {
    #[serde(rename = "wave")]
    Wave(Vec<String>),
    #[serde(rename = "json")]
    Json(Value),
}

impl InvokeResultView {
    pub fn try_parse_or_json(
        res: InvokeResult,
        component: &Component,
        function: &str,
    ) -> Result<InvokeResultView, GolemError> {
        Ok(
            Self::try_parse(&res.result, component, function).unwrap_or_else(|_| {
                let json = serde_json::to_value(&res.result).unwrap();
                InvokeResultView::Json(json)
            }),
        )
    }

    fn try_parse(
        res: &protobuf::type_annotated_value::TypeAnnotatedValue,
        component: &Component,
        function: &str,
    ) -> Result<InvokeResultView, GolemError> {
        let results = match res {
            protobuf::type_annotated_value::TypeAnnotatedValue::Tuple(tuple) => tuple
                .value
                .iter()
                .map(|t| t.clone().type_annotated_value.unwrap())
                .collect::<Vec<_>>(),
            // TODO: need to support multi-result case when it's a Record
            _ => {
                info!("Can't parse InvokeResult - tuple expected.");

                return Err(GolemError(
                    "Can't parse InvokeResult - tuple expected.".to_string(),
                ));
            }
        };

        // TODO: we don't need this, as the result is always a TypeAnnotatedValue
        let result_types = function_result_types(component, function)?;

        if results.len() != result_types.len() {
            info!("Unexpected number of results.");

            return Err(GolemError("Unexpected number of results.".to_string()));
        }

        if !result_types.iter().all(|typ| type_wave_compatible(typ)) {
            debug!("Result type is not supported by wave");

            return Err(GolemError(
                "Result type is not supported by wave".to_string(),
            ));
        }

        let wave = results
            .into_iter()
            .map(Self::try_wave_format)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(InvokeResultView::Wave(wave))
    }

    fn try_wave_format(
        parsed: protobuf::type_annotated_value::TypeAnnotatedValue,
    ) -> Result<String, GolemError> {
        match print_type_annotated_value(&parsed) {
            Ok(res) => Ok(res),
            Err(err) => {
                info!("Failed to format parsed value as wave: {err:?}");

                Err(GolemError(
                    "Failed to format parsed value as wave".to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::vec;

    use test_r::test;

    use chrono::Utc;
    use golem_wasm_ast::analysis::analysed_type::{bool, handle};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionResult, AnalysedResourceId,
        AnalysedResourceMode,
    };
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::TypeAnnotatedValue as RootTypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::TypedTuple;
    use golem_wasm_rpc::TypeAnnotatedValueConstructors;
    use uuid::Uuid;

    use golem_client::model::{
        AnalysedType, ComponentMetadata, ComponentType, InvokeResult, VersionedComponentId,
    };

    use crate::model::component::Component;
    use crate::model::invoke_result_view::InvokeResultView;

    fn parse(results: Vec<golem_wasm_rpc::Value>, types: Vec<AnalysedType>) -> InvokeResultView {
        let typed_results = results
            .iter()
            .zip(&types)
            .map(|(val, typ)| TypeAnnotatedValue::create(val, typ).unwrap())
            .map(|tv| RootTypeAnnotatedValue {
                type_annotated_value: Some(tv),
            })
            .collect::<Vec<_>>();

        let typed_result = TypeAnnotatedValue::Tuple(TypedTuple {
            typ: types.iter().map(|t| t.into()).collect(),
            value: typed_results,
        });

        let func_res = types
            .into_iter()
            .map(|typ| AnalysedFunctionResult { name: None, typ })
            .collect::<Vec<_>>();

        let component = Component {
            versioned_component_id: VersionedComponentId {
                component_id: Uuid::max(),
                version: 0,
            },
            component_name: String::new(),
            component_size: 0,
            component_type: ComponentType::Durable,
            metadata: ComponentMetadata {
                producers: Vec::new(),
                exports: vec![AnalysedExport::Function(AnalysedFunction {
                    name: "func-name".to_string(),
                    parameters: Vec::new(),
                    results: func_res,
                })],
                memories: vec![],
            },
            project_id: None,
            created_at: Some(Utc::now()),
            files: vec![],
        };

        InvokeResultView::try_parse_or_json(
            InvokeResult {
                result: typed_result,
            },
            &component,
            "func-name",
        )
        .unwrap()
    }

    #[test]
    fn represented_as_wave() {
        let res = parse(vec![golem_wasm_rpc::Value::Bool(true)], vec![bool()]);

        assert!(matches!(res, InvokeResultView::Wave(_)))
    }

    #[test]
    fn fallback_to_json() {
        let res = parse(
            vec![golem_wasm_rpc::Value::Handle {
                uri: "".to_string(),
                resource_id: 1,
            }],
            vec![handle(AnalysedResourceId(1), AnalysedResourceMode::Owned)],
        );

        assert!(matches!(res, InvokeResultView::Json(_)))
    }
}
