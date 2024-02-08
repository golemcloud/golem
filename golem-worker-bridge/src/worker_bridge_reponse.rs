use std::collections::HashMap;

use hyper::{HeaderMap, StatusCode};
use poem::{Body, Response, ResponseParts};
use serde_json::Value;

use crate::api_definition::ResponseMapping;
use crate::evaluator::{EvaluationError, Evaluator};
use crate::expr::Expr;
use crate::resolved_variables::ResolvedVariables;
use crate::worker_request_executor::WorkerResponse;

// Getting a gateway http response from a worker response should never fail
// and all failure scenarios to be captured in status code and response body
pub trait GetWorkerBridgeResponse {
    fn to_worker_bridge_response(
        &self,
        response_mapping: &ResponseMapping,
        resolved_variables: &ResolvedVariables,
    ) -> Result<WorkerBridgeResponse, EvaluationError>;
}

pub struct WorkerBridgeResponse {
    pub body: Value,
    pub status: StatusCode,
    pub headers: ResolvedHeaders,
}

impl WorkerBridgeResponse {
    pub fn to_http_response(&self) -> Response {
        let headers: Result<HeaderMap, String> = self.headers.to_string_map().and_then(|headers| {
            (&headers)
                .try_into()
                .map_err(|e: hyper::http::Error| e.to_string())
        });

        let status = &self.status;
        let body = &self.body;

        match headers {
            Ok(response_headers) => {
                let parts = ResponseParts {
                    status: status.clone(),
                    version: Default::default(),
                    headers: response_headers,
                    extensions: Default::default(),
                };
                let body: Body = Body::from_json(body.clone()).unwrap();
                Response::from_parts(parts, body)
            }
            Err(err) => {
                Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(format!(
                        "Unable to resolve valid headers. Error: {}",
                        err
                    )))
            }
        }
    }
}

#[derive(Default)]
pub struct ResolvedHeaders {
    pub headers: HashMap<String, String>,
}

impl ResolvedHeaders {
    fn to_string_map(&self) -> Result<HashMap<String, String>, String> {
        let mut headers = HashMap::new();

        for (key, value) in &self.headers {
            headers.insert(key.clone(), value.clone());
        }

        Ok(headers)
    }

    // Example: In API definition, user may define a header as "X-Request-${worker-response.value}" to be added
    // to the http response. Here we resolve the expression based on the resolved variables (that was formed from the response of the worker)
    fn from(
        header_mapping: &HashMap<String, Expr>,
        gateway_variables: &ResolvedVariables,
    ) -> Result<ResolvedHeaders, EvaluationError> {
        let mut resolved_headers: HashMap<String, String> = HashMap::new();

        for (header_name, value_expr) in header_mapping {
            let value = value_expr.evaluate(gateway_variables)?;

            let value_str = value.as_str().ok_or(EvaluationError::Message(format!(
                "Header value is not a string"
            )))?;

            resolved_headers.insert(header_name.clone(), value_str.to_string());
        }

        Ok(ResolvedHeaders {
            headers: resolved_headers,
        })
    }
}

impl GetWorkerBridgeResponse for WorkerResponse {
    fn to_worker_bridge_response(
        &self,
        response_mapping: &ResponseMapping,
        input_gateway_variables: &ResolvedVariables,
    ) -> Result<WorkerBridgeResponse, EvaluationError> {
        let variables = {
            let mut response_variables = ResolvedVariables::from_worker_response(self);
            response_variables.extend(input_gateway_variables);
            response_variables
        };

        let status_code = get_status_code(&response_mapping.status, &variables)?;

        let headers = ResolvedHeaders::from(&response_mapping.headers, &variables)?;

        let response_body = response_mapping.body.evaluate(&variables)?;

        Ok(WorkerBridgeResponse {
            body: response_body,
            status: status_code,
            headers,
        })
    }
}

fn get_status_code(
    status_expr: &Expr,
    resolved_variables: &ResolvedVariables,
) -> Result<StatusCode, EvaluationError> {
    let status_value = status_expr.evaluate(resolved_variables)?;
    let status_str = status_value
        .as_str()
        .ok_or(EvaluationError::Message(format!(
            "Status Code Expression is evaluated to a complex value. It is resolved to {:?}",
            status_value
        )))?;

    let status_u16 = status_str.parse::<u16>().map_err(|e| {
        EvaluationError::Message(format!(
            "Invalid Status Code Expression. It is resolved to {}. Error: {}",
            status_str, e
        ))
    })?;

    StatusCode::from_u16(status_u16).map_err(|e| EvaluationError::Message(format!(
        "Invalid Status Code. A valid status code cannot be formed from the evaluated status code expression {}. Error: {}",
        status_str, e
    )))
}
