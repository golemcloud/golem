use crate::model::template::function_result_types;
use crate::model::wave::{type_to_analysed, type_wave_compatible};
use crate::model::GolemError;
use golem_client::model::{InvokeResult, Template, Type};
use golem_wasm_rpc::TypeAnnotatedValue;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use tracing::{debug, info};

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
        template: &Template,
        function: &str,
    ) -> InvokeResultView {
        Self::try_parse(&res, template, function).unwrap_or(InvokeResultView::Json(res.result))
    }

    fn try_parse(
        res: &InvokeResult,
        template: &Template,
        function: &str,
    ) -> Result<InvokeResultView, GolemError> {
        let results = match res.result.as_array() {
            None => {
                info!("Can't parse InvokeResult - array expected.");

                return Err(GolemError(
                    "Can't parse InvokeResult - array expected.".to_string(),
                ));
            }
            Some(results) => results,
        };

        let result_types = function_result_types(template, function)?;

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
            .iter()
            .zip(result_types.iter())
            .map(|(json, typ)| Self::try_wave_format(json, typ))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(InvokeResultView::Wave(wave))
    }

    fn try_wave_format(json: &Value, typ: &Type) -> Result<String, GolemError> {
        let parsed = Self::try_parse_single(json, typ)?;

        match wasm_wave::to_string(&parsed) {
            Ok(res) => Ok(res),
            Err(err) => {
                info!("Failed to format parsed value as wave: {err:?}");

                Err(GolemError(
                    "Failed to format parsed value as wave".to_string(),
                ))
            }
        }
    }

    fn try_parse_single(json: &Value, typ: &Type) -> Result<TypeAnnotatedValue, GolemError> {
        match golem_wasm_rpc::json::get_typed_value_from_json(json, &type_to_analysed(typ)) {
            Ok(res) => Ok(res),
            Err(errs) => {
                info!("Failed to parse typed value: {errs:?}");

                Err(GolemError("Failed to parse typed value".to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::invoke_result_view::InvokeResultView;
    use crate::model::wave::type_to_analysed;
    use golem_client::model::{
        Export, ExportFunction, FunctionResult, InvokeResult, ProtectedTemplateId, ResourceMode,
        Template, TemplateMetadata, Type, TypeBool, TypeHandle, UserTemplateId,
        VersionedTemplateId,
    };
    use golem_wasm_ast::analysis::AnalysedFunctionResult;
    use golem_wasm_rpc::Uri;
    use uuid::Uuid;

    fn parse(results: Vec<golem_wasm_rpc::Value>, types: Vec<Type>) -> InvokeResultView {
        let analyzed_res = types
            .iter()
            .map(|t| AnalysedFunctionResult {
                name: None,
                typ: type_to_analysed(t),
            })
            .collect::<Vec<_>>();
        let json = golem_wasm_rpc::json::function_result(results, &analyzed_res).unwrap();

        let func_res = types
            .into_iter()
            .map(|typ| FunctionResult { name: None, typ })
            .collect::<Vec<_>>();

        let template = Template {
            versioned_template_id: VersionedTemplateId {
                template_id: Uuid::max(),
                version: 0,
            },
            user_template_id: UserTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: Uuid::max(),
                    version: 0,
                },
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: Uuid::max(),
                    version: 0,
                },
            },
            template_name: String::new(),
            template_size: 0,
            metadata: TemplateMetadata {
                producers: Vec::new(),
                exports: vec![Export::Function(ExportFunction {
                    name: "func_name".to_string(),
                    parameters: Vec::new(),
                    results: func_res,
                })],
            },
        };

        InvokeResultView::try_parse_or_json(InvokeResult { result: json }, &template, "func_name")
    }

    #[test]
    fn represented_as_wave() {
        let res = parse(
            vec![golem_wasm_rpc::Value::Bool(true)],
            vec![Type::Bool(TypeBool {})],
        );

        assert!(matches!(res, InvokeResultView::Wave(_)))
    }

    #[test]
    fn fallback_to_json() {
        let res = parse(
            vec![golem_wasm_rpc::Value::Handle {
                uri: Uri {
                    value: "".to_string(),
                },
                resource_id: 1,
            }],
            vec![Type::Handle(TypeHandle {
                resource_id: 1,
                mode: ResourceMode::Owned,
            })],
        );

        assert!(matches!(res, InvokeResultView::Json(_)))
    }
}
