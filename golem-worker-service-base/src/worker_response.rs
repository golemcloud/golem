use std::collections::HashMap;

use crate::api_definition::ResponseMapping;
use crate::evaluator::{EvaluationError, Evaluator};
use crate::expr::Expr;
use crate::resolved_variables::ResolvedVariables;
use crate::worker_request::WorkerRequest;
use crate::worker_request_to_response::WorkerRequestToResponse;
use async_trait::async_trait;
use http::{HeaderMap, StatusCode};
use poem::{Body, ResponseParts};
use serde_json::{json, Value};
use tracing::info;

pub struct WorkerResponse {
    pub result: Value,
}

impl WorkerResponse {
    pub fn to_http_response(
        &self,
        response_mapping: &Option<ResponseMapping>,
        resolved_variables_from_request: &ResolvedVariables,
    ) -> poem::Response {
        if let Some(mapping) = response_mapping {
            match &self.to_intermediate_http_response(mapping, resolved_variables_from_request) {
                Ok(intermediate_response) => intermediate_response.to_http_response(),
                Err(e) => poem::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(format!(
                        "Error when  converting worker response to http response. Error: {}",
                        e
                    ))),
            }
        } else {
            let body: Body = Body::from_json(&self.result).unwrap();
            poem::Response::builder().body(body)
        }
    }

    fn to_intermediate_http_response(
        &self,
        response_mapping: &ResponseMapping,
        resolved_variables_from_request: &ResolvedVariables,
    ) -> Result<IntermediateHttpResponse, EvaluationError> {
        let variables = {
            let mut response_variables = ResolvedVariables::from_worker_response(&self.result);
            response_variables.extend(resolved_variables_from_request);
            response_variables
        };

        let status_code = get_status_code(&response_mapping.status, &variables)?;

        let headers = ResolvedResponseHeaders::from(&response_mapping.headers, &variables)?;

        let response_body = response_mapping.body.evaluate(&variables)?;

        Ok(IntermediateHttpResponse {
            body: response_body,
            status: status_code,
            headers,
        })
    }
}

pub struct IntermediateHttpResponse {
    pub body: Value,
    pub status: StatusCode,
    pub headers: ResolvedResponseHeaders,
}

impl IntermediateHttpResponse {
    fn to_http_response(&self) -> poem::Response {
        let headers: Result<HeaderMap, String> = (&self.headers.to_string_map())
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
                let body: Body = Body::from_json(body.clone()).unwrap();
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

#[derive(Default)]
pub struct ResolvedResponseHeaders {
    pub headers: HashMap<String, String>,
}

impl ResolvedResponseHeaders {
    pub fn to_string_map(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();

        for (key, value) in &self.headers {
            headers.insert(key.clone(), value.clone());
        }

        headers
    }

    // Example: In API definition, user may define a header as "X-Request-${worker-response.value}" to be added
    // to the http response. Here we resolve the expression based on the resolved variables (that was formed from the response of the worker)
    pub fn from(
        header_mapping: &HashMap<String, Expr>,
        gateway_variables: &ResolvedVariables,
    ) -> Result<ResolvedResponseHeaders, EvaluationError> {
        let mut resolved_headers: HashMap<String, String> = HashMap::new();

        for (header_name, value_expr) in header_mapping {
            let value = value_expr.evaluate(gateway_variables)?;

            let value_str = value.as_str().ok_or(EvaluationError::Message(format!(
                "Header value is not a string. {}",
                value
            )))?;

            resolved_headers.insert(header_name.clone(), value_str.to_string());
        }

        Ok(ResolvedResponseHeaders {
            headers: resolved_headers,
        })
    }
}

pub struct NoOpWorkerRequestExecutor {}

#[async_trait]
impl WorkerRequestToResponse<ResponseMapping, poem::Response> for NoOpWorkerRequestExecutor {
    async fn execute(
        &self,
        worker_request_params: WorkerRequest,
        response_mapping: &Option<ResponseMapping>,
        resolved_variables: &ResolvedVariables, // resolved variables from the input request can also be useful to form the response
    ) -> poem::Response {
        let worker_name = worker_request_params.worker_id;
        let template_id = worker_request_params.template;

        info!(
            "Executing request for template: {}, worker: {}, function: {}",
            template_id, worker_name, worker_request_params.function
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

        let worker_response = WorkerResponse {
            result: sample_json_data,
        };

        worker_response.to_http_response(response_mapping, resolved_variables)
    }
}

fn get_status_code(
    status_expr: &Expr,
    resolved_variables: &ResolvedVariables,
) -> Result<StatusCode, EvaluationError> {
    let status_value = status_expr.evaluate(resolved_variables)?;
    let status_res: Result<u16, EvaluationError> =
        match status_value {
            Value::String(status_str) => status_str.parse().map_err(|e| {
                EvaluationError::Message(format!(
                    "Invalid Status Code Expression. It is resolved to a string but not a number {}. Error: {}",
                    status_str, e
                ))
            }),
            Value::Number(number) => number.to_string().parse().map_err(|e| {
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
