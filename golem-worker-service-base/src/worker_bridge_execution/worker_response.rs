use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::{get_json_from_typed_value, get_typed_value_from_json};
use golem_wasm_rpc::TypeAnnotatedValue;
use http::StatusCode;
use poem::Body;
use serde_json::json;
use tracing::info;

use golem_service_base::type_inference::*;

use crate::tokeniser::tokenizer::Token;
use crate::worker_binding::ResponseMapping;
use crate::worker_bridge_execution::worker_request_executor::{
    WorkerRequestExecutor, WorkerRequestExecutorError,
};
use crate::worker_bridge_execution::WorkerRequest;

pub struct WorkerResponse {
    pub result: TypeAnnotatedValue,
}

impl WorkerResponse {
    pub(crate) fn to_http_response(
        &self,
        response_mapping: &Option<ResponseMapping>,
        input_request: &TypeAnnotatedValue,
    ) -> poem::Response {
        if let Some(mapping) = response_mapping {
            match internal::IntermediateHttpResponse::from(self, mapping, input_request) {
                Ok(intermediate_response) => intermediate_response.to_http_response(),
                Err(e) => poem::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(format!(
                        "Error when  converting worker response to http response. Error: {}",
                        e
                    ))),
            }
        } else {
            let json = get_json_from_typed_value(&self.result);
            let body: Body = Body::from_json(json).unwrap();
            poem::Response::builder().body(body)
        }
    }

    // This makes sure that the result is injected into the worker.response field
    // So that clients can refer to the worker response using worker.response keyword
    pub(crate) fn result_with_worker_response_key(&self) -> TypeAnnotatedValue {
        let worker_response_value = &self.result;
        let worker_response_typ = AnalysedType::from(worker_response_value);
        let response_key = "response".to_string();

        let response_type = vec![(response_key.clone(), worker_response_typ.clone())];

        TypeAnnotatedValue::Record {
            typ: vec![(
                Token::worker().to_string(),
                AnalysedType::Record(response_type.clone()),
            )],
            value: vec![(
                Token::worker().to_string(),
                TypeAnnotatedValue::Record {
                    typ: response_type.clone(),
                    value: vec![(response_key.clone(), worker_response_value.clone())],
                },
            )],
        }
    }
}

pub struct NoOpWorkerRequestExecutor {}

#[async_trait]
impl WorkerRequestExecutor<poem::Response> for NoOpWorkerRequestExecutor {
    async fn execute(
        &self,
        worker_request_params: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerRequestExecutorError> {
        let worker_name = worker_request_params.worker_id;
        let component_id = worker_request_params.component;

        info!(
            "Executing request for component: {}, worker: {}, function: {}",
            component_id, worker_name, worker_request_params.function
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
            result: type_anntoated_value,
        };

        Ok(worker_response)
    }
}

mod internal {
    use crate::api_definition::http::HttpResponseMapping;
    use crate::evaluator::EvaluationError;
    use crate::evaluator::Evaluator;
    use crate::expression::Expr;
    use crate::merge::Merge;
    use crate::primitive::{GetPrimitive, Primitive};
    use crate::worker_binding::ResponseMapping;
    use crate::worker_bridge_execution::WorkerResponse;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, StatusCode};
    use poem::{Body, ResponseParts};
    use std::collections::HashMap;

    pub(crate) struct IntermediateHttpResponse {
        body: TypeAnnotatedValue,
        status: StatusCode,
        headers: ResolvedResponseHeaders,
    }

    impl IntermediateHttpResponse {
        pub(crate) fn from(
            worker_response: &WorkerResponse,
            response_mapping: &ResponseMapping,
            input_request: &TypeAnnotatedValue,
        ) -> Result<IntermediateHttpResponse, EvaluationError> {
            let mut input_request = input_request.clone();
            let type_annotated_value =
                input_request.merge(&worker_response.result_with_worker_response_key());

            let http_response_mapping = HttpResponseMapping::try_from(response_mapping)
                .map_err(EvaluationError::Message)?;

            let status_code = get_status_code(&http_response_mapping.status, type_annotated_value)?;

            let headers = ResolvedResponseHeaders::from(
                &http_response_mapping.headers,
                type_annotated_value,
            )?;

            let response_body = http_response_mapping.body.evaluate(type_annotated_value)?;

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
            let body = &self.body;

            match headers {
                Ok(response_headers) => {
                    let parts = ResponseParts {
                        status: *status,
                        version: Default::default(),
                        headers: response_headers,
                        extensions: Default::default(),
                    };
                    let body: Body = Body::from_json(get_json_from_typed_value(body)).unwrap();
                    poem::Response::from_parts(parts, body)
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
        resolved_variables: &TypeAnnotatedValue,
    ) -> Result<StatusCode, EvaluationError> {
        let status_value = status_expr.evaluate(resolved_variables)?;
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
            input: &TypeAnnotatedValue, // The input to evaluating header expression is a type annotated value
        ) -> Result<ResolvedResponseHeaders, EvaluationError> {
            let mut resolved_headers: HashMap<String, String> = HashMap::new();

            for (header_name, header_value_expr) in header_mapping {
                let value = header_value_expr.evaluate(input)?;

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
