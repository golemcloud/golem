use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::{get_json_from_typed_value, get_typed_value_from_json};
use golem_wasm_rpc::TypeAnnotatedValue;
use http::StatusCode;
use poem::Body;
use serde_json::json;
use tracing::info;

use crate::service::worker::TypedResult;
use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::worker_request_executor::{
    WorkerRequestExecutor, WorkerRequestExecutorError,
};
use crate::worker_bridge_execution::{WorkerRequest, WorkerResponse};
use golem_service_base::type_inference::*;
use crate::evaluator::{EvaluationContext, Evaluator};

// Refined Worker response is different from WorkerResponse, because,
// it ensures that we are not returning a vector of result if they are not named results
// or uni
#[derive(Clone)]
pub enum RefinedWorkerResponse {
    Unit,
    SingleResult(TypeAnnotatedValue),
    MultipleResults(TypeAnnotatedValue),
}

impl RefinedWorkerResponse {
    pub(crate) fn to_type_annotated_value(&self) -> Option<TypeAnnotatedValue> {
        match self {
            RefinedWorkerResponse::Unit => None,
            RefinedWorkerResponse::SingleResult(value) => Some(value.clone()),
            RefinedWorkerResponse::MultipleResults(results) => Some(results.clone()),
        }
    }

    pub(crate) fn from_worker_response(
        worker_response: &WorkerResponse,
    ) -> Result<RefinedWorkerResponse, String> {
        let result = &worker_response.result.result;
        let function_result_types = &worker_response.result.function_result_types;

        if function_result_types.iter().all(|r| r.name.is_none())
            && !function_result_types.is_empty()
        {
            match result {
                TypeAnnotatedValue::Tuple { value, .. } => {
                    if value.len() == 1 {
                        Ok(RefinedWorkerResponse::SingleResult(value[0].clone()))
                    } else if value.is_empty() {
                        Ok(RefinedWorkerResponse::Unit)
                    } else {
                        Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Tuple with 1 element if results are unnamed. Obtained {:?}", AnalysedType::from(result)))
                    }
                }
                ty => Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Tuple if results ae unnamed. Obtained {:?}", AnalysedType::from(ty))),
            }
        } else {
            match &worker_response.result.result  {
                TypeAnnotatedValue::Record { .. } => {
                    Ok(RefinedWorkerResponse::MultipleResults(worker_response.result.result.clone()))
                }

                // See wasm-rpc implementations for more details
                ty => Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Record if results are named. Obtained {:?}", AnalysedType::from(ty))),
            }
        }
    }

    pub(crate) fn to_http_response(
        &self,
        response_mapping: &Option<ResponseMapping>,
        input_request: &RequestDetails,
        worker_request: &WorkerRequest
    ) -> poem::Response {
        if let Some(mapping) = response_mapping {
            match internal::IntermediateHttpResponse::from(self, mapping, input_request, worker_request) {
                Ok(intermediate_response) => intermediate_response.to_http_response(),
                Err(e) => poem::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(format!(
                        "Error when  converting worker response to http response. Error: {}",
                        e
                    ))),
            }
        } else {
            let type_annotated_value = match self {
                RefinedWorkerResponse::Unit => None,
                RefinedWorkerResponse::SingleResult(value) => Some(value.clone()),
                RefinedWorkerResponse::MultipleResults(results) => Some(results.clone()),
            };

            match type_annotated_value {
                Some(value) => {
                    let json = get_json_from_typed_value(&value);
                    let body: Body = Body::from_json(json).unwrap();
                    poem::Response::builder().body(body)
                }
                None => poem::Response::builder().status(StatusCode::OK).finish(),
            }
        }
    }
}

pub struct NoOpWorkerRequestExecutor {}

#[async_trait]
impl WorkerRequestExecutor for NoOpWorkerRequestExecutor {
    async fn execute(
        &self,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name = worker_request_params.worker_name;
        let component_id = worker_request_params.component_id;

        info!(
            "Executing request for component: {}, worker: {}, function: {}",
            component_id, worker_name, worker_request_params.function_name
        );

        let sample_json_data = json!(
            [{
              "description" : "This is a sample in-memory response",
              "worker" : worker_name,
              "name": "John Doe",
              "age": 30,
              "email": "johndoe@example.com",
              "isStudent": false,
              "address": {
                "street": "123 Main Street",
                "city": "Anytown",
                "state": "CA",
                "postalCode": "12345"
              },
              "hobbies": ["reading", "hiking", "gaming"],
              "scores": [95, 88, 76, 92],
              "input" : worker_request_params.function_params.to_string()
            }]
        );

        // From request body you can infer analysed type
        let analysed_type = infer_analysed_type(&sample_json_data);
        let type_anntoated_value =
            get_typed_value_from_json(&sample_json_data, &analysed_type).unwrap();

        let worker_response = WorkerResponse {
            result: TypedResult {
                result: type_anntoated_value,
                function_result_types: vec![],
            },
        };

        Ok(worker_response)
    }
}

mod internal {
    use crate::api_definition::http::HttpResponseMapping;
    use crate::evaluator::Evaluator;
    use crate::evaluator::{EvaluationError, EvaluationResult, EvaluationContext};
    use crate::expression::Expr;
    use crate::primitive::{GetPrimitive, Primitive};
    use crate::worker_binding::{RequestDetails, ResponseMapping};
    use crate::worker_bridge_execution::worker_bridge_response::RefinedWorkerResponse;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, StatusCode};
    use poem::{Body, ResponseParts};
    use std::collections::HashMap;
    use crate::worker_bridge_execution::WorkerRequest;

    pub(crate) struct IntermediateHttpResponse {
        body: EvaluationResult,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateHttpResponse {
        pub(crate) fn from(
            worker_response: &RefinedWorkerResponse,
            response_mapping: &ResponseMapping,
            request_details: &RequestDetails,
            worker_request: &WorkerRequest
        ) -> Result<IntermediateHttpResponse, EvaluationError> {
            let evaluation_context =
                EvaluationContext::from(worker_request, worker_response, request_details);

            let http_response_mapping = HttpResponseMapping::try_from(response_mapping)
                .map_err(EvaluationError::Message)?;

            let status_code =
                get_status_code(&http_response_mapping.status, &evaluation_context)?;

            let headers = ResolvedResponseHeaders::from(
                &http_response_mapping.headers,
                &evaluation_context,
            )?;

            let response_body = http_response_mapping
                .body
                .evaluate(&evaluation_context)?;

            Ok(IntermediateHttpResponse {
                body: response_body,
                status: status_code,
                headers,
            })
        }
        pub(crate) fn to_http_response(&self) -> poem::Response {
            let headers: Result<HeaderMap, String> = (&self.headers.headers)
                .try_into()
                .map_err(|e: hyper::http::Error| e.to_string());

            let status = &self.status;
            let eval_result = &self.body;

            match headers {
                Ok(response_headers) => {
                    let parts = ResponseParts {
                        status: *status,
                        version: Default::default(),
                        headers: response_headers,
                        extensions: Default::default(),
                    };

                    match eval_result {
                        EvaluationResult::Value(value) => {
                            let json = get_json_from_typed_value(value);
                            let body: Body = Body::from_json(json).unwrap();
                            poem::Response::from_parts(parts, body)
                        }
                        EvaluationResult::Unit => poem::Response::from_parts(parts, Body::empty()),
                    }
                }
                Err(err) => poem::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(format!(
                        "Unable to resolve valid headers. Error: {}",
                        err
                    ))),
            }
        }
    }

    fn get_status_code(
        status_expr: &Expr,
        evaluator_context: &EvaluationContext,
    ) -> Result<StatusCode, EvaluationError> {
        let status_value = status_expr.evaluate(evaluator_context)?;
        let status_res: Result<u16, EvaluationError> =
            match status_value.get_primitive() {
                Some(Primitive::String(status_str)) => status_str.parse().map_err(|e| {
                    EvaluationError::Message(format!(
                        "Invalid Status Code Expression. It is resolved to a string but not a number {}. Error: {}",
                        status_str, e
                    ))
                }),
                Some(Primitive::Num(number)) => number.to_string().parse().map_err(|e| {
                    EvaluationError::Message(format!(
                        "Invalid Status Code Expression. It is resolved to a number but not a u16 {}. Error: {}",
                        number, e
                    ))
                }),
                _ => Err(EvaluationError::Message(format!(
                    "Status Code Expression is evaluated to a complex value. It is resolved to {:?}",
                    status_value
                )))
            };

        let status_u16 = status_res?;

        StatusCode::from_u16(status_u16).map_err(|e| EvaluationError::Message(format!(
            "Invalid Status Code. A valid status code cannot be formed from the evaluated status code expression {}. Error: {}",
            status_u16, e
        )))
    }

    #[derive(Default)]
    struct ResolvedResponseHeaders {
        headers: HashMap<String, String>,
    }

    impl ResolvedResponseHeaders {
        // Example: In API definition, user may define a header as "X-Request-${worker-response.value}" to be added
        // to the http response. Here we resolve the expression based on the resolved variables (that was formed from the response of the worker)
        fn from(
            header_mapping: &HashMap<String, Expr>,
            input: &EvaluationContext, // The input to evaluating header expression is a type annotated value
        ) -> Result<ResolvedResponseHeaders, EvaluationError> {
            let mut resolved_headers: HashMap<String, String> = HashMap::new();

            for (header_name, header_value_expr) in header_mapping {
                let value = header_value_expr
                    .evaluate(input, None)?
                    .get_value()
                    .ok_or("Unable to resolve header. Resulted in ()".to_string())?;

                let value_str = value
                    .get_primitive()
                    .ok_or(EvaluationError::Message(format!(
                        "Header value is not a string. {}",
                        get_json_from_typed_value(&value)
                    )))?;

                resolved_headers.insert(header_name.clone(), value_str.to_string());
            }

            Ok(ResolvedResponseHeaders {
                headers: resolved_headers,
            })
        }
    }
}
