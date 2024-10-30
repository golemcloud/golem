use crate::api::WorkerBindingType;
use crate::api_definition::http::{CompiledHttpApiDefinition, VarInfo};
use crate::getter::GetterExt;
use crate::http::http_request::router;
use crate::http::router::RouterPattern;
use crate::http::InputHttpRequest;
use crate::path::Path;
use crate::worker_service_rib_interpreter::EvaluationError;
use crate::worker_service_rib_interpreter::WorkerServiceRibInterpreter;
use async_trait::async_trait;
use golem_common::model::IdempotencyKey;
use golem_service_base::model::VersionedComponentId;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions as _;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::typed_result::ResultValue;
use rib::RibInterpreterResult;
use rib::RibResult;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use crate::worker_binding::rib_input_value_resolver::RibInputValueResolver;
use crate::worker_binding::{RequestDetails, ResponseMappingCompiled, RibInputTypeMismatch};
use crate::worker_bridge_execution::to_response::ToResponse;

// Every type of request (example: InputHttpRequest (which corresponds to a Route)) can have an instance of this resolver,
// to resolve a single worker-binding is then executed with the help of worker_service_rib_interpreter, which internally
// calls the worker function.
#[async_trait]
pub trait RequestToWorkerBindingResolver<ApiDefinition> {
    async fn resolve_worker_binding(
        &self,
        api_definitions: Vec<ApiDefinition>,
    ) -> Result<ResolvedWorkerBindingFromRequest, WorkerBindingResolutionError>;
}

#[derive(Debug)]
pub struct WorkerBindingResolutionError(pub String);

impl<A: AsRef<str>> From<A> for WorkerBindingResolutionError {
    fn from(message: A) -> Self {
        WorkerBindingResolutionError(message.as_ref().to_string())
    }
}

impl Display for WorkerBindingResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker binding resolution error: {}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBindingFromRequest {
    pub worker_detail: WorkerDetail,
    pub request_details: RequestDetails,
    pub compiled_response_mapping: ResponseMappingCompiled,
    pub binding_type: WorkerBindingType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDetail {
    pub component_id: VersionedComponentId,
    pub worker_name: String,
    pub idempotency_key: Option<IdempotencyKey>,
}

impl WorkerDetail {
    pub fn as_json(&self) -> Value {
        let mut worker_detail_content = HashMap::new();
        worker_detail_content.insert(
            "component_id".to_string(),
            Value::String(self.component_id.component_id.0.to_string()),
        );
        worker_detail_content.insert("name".to_string(), Value::String(self.worker_name.clone()));
        if let Some(idempotency_key) = &self.idempotency_key {
            worker_detail_content.insert(
                "idempotency_key".to_string(),
                Value::String(idempotency_key.value.clone()),
            );
        }

        let map = serde_json::Map::from_iter(worker_detail_content);

        Value::Object(map)
    }
}

impl ResolvedWorkerBindingFromRequest {
    pub async fn interpret_response_mapping<R>(
        &self,
        evaluator: &Arc<dyn WorkerServiceRibInterpreter + Sync + Send>,
    ) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
        FileServerResult: ToResponse<R>,
    {
        let request_rib_input = self
            .request_details
            .resolve_rib_input_value(&self.compiled_response_mapping.rib_input);

        let worker_rib_input = self
            .worker_detail
            .resolve_rib_input_value(&self.compiled_response_mapping.rib_input);

        match (request_rib_input, worker_rib_input) {
            (Ok(request_rib_input), Ok(worker_rib_input)) => {
                let rib_input = request_rib_input.merge(worker_rib_input);
                let result = evaluator
                    .evaluate(
                        &self.worker_detail.worker_name,
                        &self.worker_detail.component_id.component_id,
                        &self.worker_detail.idempotency_key,
                        &self.compiled_response_mapping.compiled_response.clone(),
                        &rib_input,
                    )
                    .await;

                match result {
                    Ok(worker_response) => {
                        if self.binding_type == WorkerBindingType::FileServer {
                            Self::get_file_server_result(worker_response)
                                .get_file()
                                .await
                                .to_response(&self.request_details)
                        } else {
                            worker_response.to_response(&self.request_details)
                        }
                    }
                    Err(err) => err.to_response(&self.request_details),
                }
            }
            (Err(err), _) => err.to_response(&self.request_details),
            (_, Err(err)) => err.to_response(&self.request_details),
        }
    }

    fn get_file_server_result(worker_response: RibInterpreterResult) -> FileServerResult<String> {
        Self::get_file_server_result_internal(worker_response)
            .unwrap_or_else(FileServerResult::SimpleErr)
    }

    fn get_file_server_result_internal(worker_response: RibInterpreterResult) -> Result<FileServerResult<String>, String> {
        let RibInterpreterResult::Val(value) = worker_response else {
            return Err(format!("Response value expected"));
        };

        let (path_value, response_details) = match value {
            // Allow evaluating to a single string as a shortcut...
            path @ TypeAnnotatedValue::Str(_) => (path, None),
            // ...Or a Result<String, String>
            TypeAnnotatedValue::Result(res) =>
                match res.result_value.ok_or("result not set")? {
                    ResultValue::OkValue(ok) => (ok.type_annotated_value.ok_or("ok unset")?, None),
                    ResultValue::ErrorValue(err) => {
                        let err = err.type_annotated_value.ok_or("err unset")?;
                        let TypeAnnotatedValue::Str(err) = err else { Err("'file-server' result error must be a string")? };
                        return Err(err);
                    }
                },
            // Otherwise use 'file-path'
            rec @ TypeAnnotatedValue::Record(_) => {
                let Some(path) = rec.get_optional(&Path::from_key("file-path")) else {
                    // If there is no 'file-path', assume this is a standard error response
                    return Ok(FileServerResult::Err(rec));
                };

                (path, Some(rec))
            }
            _ => Err("Response value expected")?,
        };

        let TypeAnnotatedValue::Str(content) = path_value else {
            return Err(format!("'file-server' must provide a string path, but evaluated to '{}'", path_value.to_json_value()));
        };

        Ok(FileServerResult::Ok { content, response_details })
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum FileServerResult<Content = Vec<u8>> {
    Ok { content: Content, response_details: Option<TypeAnnotatedValue> },
    SimpleErr(String),
    // TypedRecord of status, etc.
    Err(TypeAnnotatedValue),
}

impl FileServerResult<String> {
    pub async fn get_file(self) -> FileServerResult<Vec<u8>> {
        match self {
            Self::Ok { content: _path, response_details } => {
                // TODO actually get file
                let content = std::io::Result::Ok(vec![0u8; 0]);

                match content {
                    Ok(content) => FileServerResult::Ok { content, response_details },
                    Err(_) => FileServerResult::SimpleErr(format!("File could not be read")),
                }
            },
            Self::SimpleErr(err) => FileServerResult::SimpleErr(err),
            Self::Err(type_annotated_value) => FileServerResult::Err(type_annotated_value),
        }
    }
}

#[async_trait]
impl RequestToWorkerBindingResolver<CompiledHttpApiDefinition> for InputHttpRequest {
    async fn resolve_worker_binding(
        &self,
        compiled_api_definitions: Vec<CompiledHttpApiDefinition>,
    ) -> Result<ResolvedWorkerBindingFromRequest, WorkerBindingResolutionError> {
        let compiled_routes = compiled_api_definitions
            .iter()
            .flat_map(|x| x.routes.clone())
            .collect::<Vec<_>>();

        let api_request = self;
        let router = router::build(compiled_routes);
        let path: Vec<&str> = RouterPattern::split(&api_request.input_path.base_path).collect();
        let request_query_variables = self.input_path.query_components().unwrap_or_default();
        let request_body = &self.req_body;
        let headers = &self.headers;

        let router::RouteEntry {
            path_params,
            query_params,
            binding,
        } = router
            .check_path(&api_request.req_method, &path)
            .ok_or("Failed to resolve route")?;

        let zipped_path_params: HashMap<VarInfo, &str> = {
            path_params
                .iter()
                .map(|(var, index)| (var.clone(), path[*index]))
                .collect()
        };

        let http_request_details = RequestDetails::from(
            &zipped_path_params,
            &request_query_variables,
            query_params,
            request_body,
            headers,
        )
        .map_err(|err| format!("Failed to fetch input request details {}", err.join(", ")))?;

        let resolve_rib_input = http_request_details
            .resolve_rib_input_value(&binding.worker_name_compiled.rib_input_type_info)
            .map_err(|err| {
                format!(
                    "Failed to resolve rib input value from http request details {}",
                    err
                )
            })?;

        // To evaluate worker-name, most probably
        let worker_name: String = rib::interpret_pure(
            &binding.worker_name_compiled.compiled_worker_name,
            &resolve_rib_input,
        )
        .await
        .map_err(|err| format!("Failed to evaluate worker name rib expression. {}", err))?
        .get_literal()
        .ok_or("Worker name is not a Rib expression that resolves to String".to_string())?
        .as_string();

        let component_id = &binding.component_id;

        let idempotency_key =
            if let Some(idempotency_key_compiled) = &binding.idempotency_key_compiled {
                let idempotency_key_value = rib::interpret_pure(
                    &idempotency_key_compiled.compiled_idempotency_key,
                    &resolve_rib_input,
                )
                .await
                .map_err(|err| err.to_string())?;

                let idempotency_key = idempotency_key_value
                    .get_literal()
                    .ok_or("Idempotency Key is not a string")?
                    .as_string();

                Some(IdempotencyKey::new(idempotency_key))
            } else {
                headers
                    .get("idempotency-key")
                    .and_then(|h| h.to_str().ok())
                    .map(|value| IdempotencyKey::new(value.to_string()))
            };

        let worker_detail = WorkerDetail {
            component_id: component_id.clone(),
            worker_name,
            idempotency_key,
        };

        let resolved_binding = ResolvedWorkerBindingFromRequest {
            worker_detail,
            request_details: http_request_details,
            compiled_response_mapping: binding.response_compiled.clone(),
            binding_type: binding.binding_type.clone(),
        };

        Ok(resolved_binding)
    }
}

#[cfg(test)]
mod file_server_result_test {
    use super::*;

    async fn into_result(interpreter: &mut rib::Interpreter, s: &str) -> FileServerResult<String> {
        let value = interpreter.run(
            rib::compile(
                &rib::from_string(
                    s.to_string()
                ).unwrap(),
            &vec![]).unwrap().byte_code
        ).await.unwrap();
        ResolvedWorkerBindingFromRequest::get_file_server_result(value)
    }

    #[tokio::test]
    async fn file_server_result() {
        let mut interpreter = rib::Interpreter::pure(Default::default());

        let res0 = into_result(&mut interpreter, r#"
            "./my_file.txt"
        "#).await;
        assert_eq!(res0, FileServerResult::Ok { content: format!("./my_file.txt"), response_details: None });

        let res1 = into_result(&mut interpreter, r#"
            ok("./my_file.txt")
        "#).await;
        assert_eq!(res1, FileServerResult::Ok { content: format!("./my_file.txt"), response_details: None });

        let res2 = into_result(&mut interpreter, r#"
            err("no file for you")
        "#).await;
        assert_eq!(res2, FileServerResult::SimpleErr("no file for you".to_string()));

        let res3 = into_result(&mut interpreter, r#"
            {
                file-path: "./my_file.txt",
                status: 418u32
            }
        "#).await;

        let FileServerResult::Ok { content, response_details } = res3 else {
            unreachable!("Expected Ok")
        };
        assert_eq!(&content, "./my_file.txt");
        assert!(response_details.is_some());
        
        let res4 = into_result(&mut interpreter, r#"
            {
                status: 418u32
            }
        "#).await;

        let FileServerResult::Err(_response_details) = res4 else {
            unreachable!("Expected Err")
        };
    }
}
