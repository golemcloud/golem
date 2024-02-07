use std::collections::HashMap;

use hyper::{HeaderMap, StatusCode};
use poem::{Body, Response, ResponseParts};

use crate::api_spec::ResponseMapping;
use crate::evaluator::{EvaluationError, Evaluator};
use crate::expr::Expr;
use crate::resolved_variables::ResolvedVariables;
use crate::typed_json::TypedJson;
use crate::worker_request_executor::WorkerResponse;

// Getting a gateway http response from a worker response should never fail
// and all failure scenarios to be captured in status code and response body
pub trait GetGatewayResponse {
    fn to_gateway_response(
        &self,
        response_mapping: &ResponseMapping,
        input_gateway_variables: &ResolvedVariables,
    ) -> GatewayResponse;
}

pub struct GatewayResponse {
    pub body: TypedJson,
    pub status: TypedJson,
    pub headers: ResolvedHeaders,
}

impl GatewayResponse {
    pub fn to_http_response(&self) -> Response {
        if let Some(status_code) = self.status.get_http_status_code() {
            let headers: Result<HeaderMap, String> =
                self.headers.to_string_map().and_then(|headers| {
                    (&headers)
                        .try_into()
                        .map_err(|e: hyper::http::Error| e.to_string())
                });

            match headers {
                Ok(response_headers) => {
                    let parts = ResponseParts {
                        status: status_code,
                        version: Default::default(),
                        headers: response_headers,
                        extensions: Default::default(),
                    };
                    let body: Body = Body::from_json(self.body.convert_to_json()).unwrap();
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
        } else {
            Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string(format!(
                    "Unable to resolve a valid status code. It is resolved to {}",
                    self.status
                )))
        }
    }
}

#[derive(Default)]
pub struct ResolvedHeaders {
    pub headers: HashMap<String, TypedJson>,
}

impl ResolvedHeaders {
    fn to_string_map(&self) -> Result<HashMap<String, String>, String> {
        let mut headers = HashMap::new();

        for (key, value) in &self.headers {
            let value = value
                .get_primitive_string()
                .ok_or(format!("{} is not a string to be part of a header", value))?;

            headers.insert(key.clone(), value);
        }

        Ok(headers)
    }

    fn from(
        header_mapping: &HashMap<String, Expr>,
        gateway_variables: &ResolvedVariables,
    ) -> Result<ResolvedHeaders, EvaluationError> {
        let mut resolved_headers: HashMap<String, TypedJson> = HashMap::new();

        for (header_name, value_expr) in header_mapping {
            let value = value_expr.evaluate(gateway_variables)?;
            resolved_headers.insert(header_name.clone(), value);
        }

        Ok(ResolvedHeaders {
            headers: resolved_headers,
        })
    }
}

impl GetGatewayResponse for WorkerResponse {
    fn to_gateway_response(
        &self,
        response_mapping: &ResponseMapping,
        input_gateway_variables: &ResolvedVariables,
    ) -> GatewayResponse {
        let variables = {
            let mut response_variables = ResolvedVariables::from_worker_response(self);
            response_variables.extend(input_gateway_variables);
            response_variables
        };

        let status_result = response_mapping.status.evaluate(&variables);

        let headers_result = ResolvedHeaders::from(&response_mapping.headers, &variables);

        if let Ok(headers) = headers_result {
            if let Ok(status) = status_result {
                match response_mapping.body.evaluate(&variables) {
                    Ok(body) => GatewayResponse {
                        body,
                        status,
                        headers,
                    },
                    Err(err) => GatewayResponse {
                        body: TypedJson::String(format!(
                            "Unable to obtain a response from the result of worker function error: {}",
                            err
                        )),
                        status: TypedJson::U64(500),
                        headers,
                    },
                }
            } else {
                GatewayResponse {
                    body: TypedJson::String(format!(
                        "Unable to resolve a status code based on the status code expression {:?}",
                        response_mapping.status,
                    )),
                    status: TypedJson::U64(400),
                    headers,
                }
            }
        } else {
            GatewayResponse {
                body: TypedJson::String(format!(
                    "Unable to resolve headers based on the header expressions {:?}",
                    response_mapping.status,
                )),
                status: TypedJson::U64(500),
                headers: ResolvedHeaders::default(),
            }
        }
    }
}
