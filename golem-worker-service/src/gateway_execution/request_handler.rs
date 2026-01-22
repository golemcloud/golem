// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::agent_response_mapping::interpret_agent_response;
use super::parameter_parsing::{
    parse_path_segment_value, parse_path_segment_value_to_component_model,
    parse_query_or_header_value, parse_request_body,
};
use super::request::RichRequest;
use super::route_resolver::{ResolvedRouteEntry, RouteResolver, RouteResolverError};
use super::{ParsedRequestBody, RouteExecutionResult};
use crate::service::worker::{WorkerService, WorkerServiceError};
use anyhow::anyhow;
use golem_common::model::agent::{
    AgentId, BinaryReference, BinaryReferenceValue, DataValue, ElementValue, ElementValues,
    UntypedDataValue, UntypedElementValue,
};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::custom_api::{ConstructorParameter, MethodParameter, RouteBehaviour};
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::IntoValue;
use golem_wasm::ValueAndType;
use http::StatusCode;
use poem::{Request, Response};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RequestHandlerError {
    #[error("Failed parsing value; Provided: {value}; Expected type: {expected}")]
    ValueParsingFailed {
        value: String,
        expected: &'static str,
    },
    #[error("Expected {expected} values to be provided, but found none")]
    MissingValue { expected: &'static str },
    #[error("Expected {expected} values to be provided, but found too many")]
    TooManyValues { expected: &'static str },
    #[error("Header value of {header_name} is not valid ascii")]
    HeaderIsNotAscii { header_name: String },
    #[error("Request body was not valid json: {error}")]
    BodyIsNotValidJson { error: String },
    #[error("Failed parsing json body: [{formatted}]", formatted=.errors.join(","))]
    JsonBodyParsingFailed { errors: Vec<String> },
    #[error("Agent response did not match expected type: {error}")]
    AgentResponseTypeMismatch { error: String },
    #[error("Invariant violated: {msg}")]
    InvariantViolated { msg: &'static str },
    #[error("Resolving route failed: {0}")]
    ResolvingRouteFailed(#[from] RouteResolverError),
    #[error("Invocation failed: {0}")]
    AgentInvocationFailed(#[from] WorkerServiceError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl RequestHandlerError {
    pub fn invariant_violated(msg: &'static str) -> Self {
        Self::InvariantViolated { msg }
    }
}

impl SafeDisplay for RequestHandlerError {
    fn to_safe_string(&self) -> String {
        todo!()
    }
}

error_forwarding!(RequestHandlerError);

pub struct RequestHandler {
    route_resolver: Arc<RouteResolver>,
    worker_service: Arc<WorkerService>,
}

#[allow(irrefutable_let_patterns)]
impl RequestHandler {
    pub fn new(route_resolver: Arc<RouteResolver>, worker_service: Arc<WorkerService>) -> Self {
        Self {
            route_resolver,
            worker_service,
        }
    }

    pub async fn handle_request(&self, request: Request) -> Result<Response, RequestHandlerError> {
        let matching_route = self.route_resolver.resolve_matching_route(&request).await?;
        let mut request = RichRequest::new(request);
        let execution_result = self.execute_route(&mut request, &matching_route).await?;
        let response = route_execution_result_to_response(execution_result)?;
        Ok(response)
    }

    async fn execute_route(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        match &resolved_route.route.behavior {
            RouteBehaviour::CallAgent { .. } => {
                self.execute_call_agent(request, resolved_route).await
            }
        }
    }

    async fn execute_call_agent(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let RouteBehaviour::CallAgent {
            expected_agent_response,
            ..
        } = &resolved_route.route.behavior
        else {
            unreachable!()
        };

        let worker_id = self.build_worker_id(resolved_route)?;

        let parsed_body = parse_request_body(request, &resolved_route.route.body).await?;

        let method_params = self.resolve_method_arguments(resolved_route, request, parsed_body)?;

        let agent_response = self
            .invoke_agent(&worker_id, resolved_route, method_params)
            .await?;

        let mapped_result = interpret_agent_response(agent_response, expected_agent_response)?;

        Ok(mapped_result)
    }

    fn build_worker_id(
        &self,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<WorkerId, RequestHandlerError> {
        let RouteBehaviour::CallAgent {
            component_id,
            agent_type,
            constructor_parameters,
            phantom,
            ..
        } = &resolved_route.route.behavior
        else {
            unreachable!()
        };

        let mut values = Vec::with_capacity(constructor_parameters.len());

        for param in constructor_parameters {
            match param {
                ConstructorParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters
                        [usize::from(*path_segment_index)]
                    .clone();

                    let value = parse_path_segment_value_to_component_model(raw, parameter_type)?;

                    values.push(ElementValue::ComponentModel(ValueAndType::new(
                        value,
                        parameter_type.clone().into(),
                    )));
                }
            }
        }

        let data_value = DataValue::Tuple(ElementValues { elements: values });

        let phantom_id = phantom.then(Uuid::new_v4);

        let agent_id = AgentId::new(agent_type.clone(), data_value, phantom_id);

        Ok(WorkerId {
            component_id: *component_id,
            worker_name: agent_id.to_string(),
        })
    }

    fn resolve_method_arguments(
        &self,
        resolved_route: &ResolvedRouteEntry,
        request: &RichRequest,
        mut body: ParsedRequestBody,
    ) -> Result<Vec<UntypedElementValue>, RequestHandlerError> {
        let RouteBehaviour::CallAgent {
            method_parameters, ..
        } = &resolved_route.route.behavior
        else {
            unreachable!()
        };

        let query_params = request.query_params();
        let headers = request.headers();

        let mut values = Vec::with_capacity(method_parameters.len());

        for param in method_parameters {
            let value = match param {
                MethodParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters[usize::from(*path_segment_index)].clone();

                    parse_path_segment_value(raw, parameter_type)?
                }

                MethodParameter::Query {
                    query_parameter_name,
                    parameter_type,
                } => {
                    let empty = Vec::new();
                    let vals = query_params.get(query_parameter_name).unwrap_or(&empty);

                    parse_query_or_header_value(vals, parameter_type)?
                }

                MethodParameter::Header {
                    header_name,
                    parameter_type,
                } => {
                    let vals = headers
                        .get_all(header_name)
                        .iter()
                        .map(|h| {
                            h.to_str().map(String::from).map_err(|_| {
                                RequestHandlerError::HeaderIsNotAscii {
                                    header_name: header_name.clone(),
                                }
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;

                    parse_query_or_header_value(&vals, parameter_type)?
                }

                MethodParameter::JsonObjectBodyField { field_index } => {
                    match &body {
                        ParsedRequestBody::JsonBody(golem_wasm::Value::Record(fields)) => {
                            UntypedElementValue::ComponentModel(fields[usize::from(*field_index)].clone())
                        }

                        ParsedRequestBody::JsonBody(_) => {
                            return Err(RequestHandlerError::invariant_violated(
                                "Inconsistent API definition: JSON field parameter but body is not an object",
                            ))
                        }

                        _ => return Err(RequestHandlerError::invariant_violated(
                            "JSON body parameter used but no JSON body schema",
                        )),
                    }
                }

                MethodParameter::UnstructuredBinaryBody => {
                    match &mut body {
                        ParsedRequestBody::UnstructuredBinary(binary_source) => {
                            let binary_source = binary_source.take().ok_or_else(|| RequestHandlerError::invariant_violated(
                                "Parsed body was already consumer",
                            ))?;

                            UntypedElementValue::UnstructuredBinary(BinaryReferenceValue { value: BinaryReference::Inline(binary_source) })
                        }

                        _ => return Err(RequestHandlerError::invariant_violated(
                            "Binary body parameter used but no binary body schema",
                        )),
                    }
                }
            };

            values.push(value);
        }

        Ok(values)
    }

    async fn invoke_agent(
        &self,
        worker_id: &WorkerId,
        resolved_route: &ResolvedRouteEntry,
        params: Vec<UntypedElementValue>,
    ) -> Result<Option<golem_wasm::ValueAndType>, RequestHandlerError> {
        let RouteBehaviour::CallAgent { method_name, .. } = &resolved_route.route.behavior else {
            unreachable!()
        };

        let method_params_data_value = UntypedDataValue::Tuple(params);

        self.worker_service
            .invoke_and_await_owned_agent(
                worker_id,
                Some(IdempotencyKey::fresh()),
                "golem:agent/guest.{invoke}".to_string(),
                vec![
                    golem_wasm::protobuf::Val::from(method_name.clone().into_value()),
                    golem_wasm::protobuf::Val::from(method_params_data_value.into_value()),
                    golem_wasm::protobuf::Val::from(
                        golem_common::model::agent::Principal::anonymous().into_value(),
                    ),
                ],
                None,
                resolved_route.route.environment_id,
                resolved_route.route.account_id,
                AuthCtx::impersonated_user(resolved_route.route.account_id),
            )
            .await
            .map_err(Into::into)
    }
}

fn route_execution_result_to_response(
    result: RouteExecutionResult,
) -> Result<Response, RequestHandlerError> {
    match result {
        RouteExecutionResult::NoBody { status } => Ok(Response::builder().status(status).finish()),

        RouteExecutionResult::ComponentModelJsonBody { body, status } => {
            let body = poem::Body::from_json(
                body.to_json_value()
                    .map_err(|e| anyhow!("ComponentModelJsonBody conversion error: {e}"))?,
            )
            .map_err(anyhow::Error::from)?;

            Ok(Response::builder().status(status).body(body))
        }

        RouteExecutionResult::UnstructuredBinaryBody { body } => Ok(Response::builder()
            .status(StatusCode::OK)
            .body(body.data)
            .set_content_type(body.binary_type.mime_type)),

        RouteExecutionResult::CustomAgentError { body } => {
            let body = poem::Body::from_json(
                body.to_json_value()
                    .map_err(|e| anyhow!("CustomAgentError conversion error: {e}"))?,
            )
            .map_err(anyhow::Error::from)?;

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(body))
        }
    }
}
