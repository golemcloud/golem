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

mod parameter_parsing;
mod response_mapping;

use self::parameter_parsing::{
    parse_path_segment_value, parse_path_segment_value_to_component_model,
    parse_query_or_header_value,
};
use self::response_mapping::interpret_agent_response;
use super::RichRequest;
use super::error::RequestHandlerError;
use super::route_resolver::ResolvedRouteEntry;
use super::{ParsedRequestBody, RouteExecutionResult};
use crate::service::worker::WorkerService;
use anyhow::anyhow;
use golem_common::model::agent::{
    AgentId, BinaryReference, BinaryReferenceValue, DataValue, ElementValue, ElementValues,
    OidcPrincipal, Principal, UntypedDataValue, UntypedElementValue,
};
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_service_base::custom_api::{CallAgentBehaviour, ConstructorParameter, MethodParameter};
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{IntoValue, ValueAndType};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

pub struct CallAgentHandler {
    worker_service: Arc<WorkerService>,
}

impl CallAgentHandler {
    pub fn new(worker_service: Arc<WorkerService>) -> Self {
        Self { worker_service }
    }

    pub async fn handle_call_agent_behaviour(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let worker_id = self.build_worker_id(resolved_route, behaviour)?;

        let parsed_body = request
            .parse_request_body(&resolved_route.route.body)
            .await?;

        let method_params =
            self.resolve_method_arguments(resolved_route, request, behaviour, parsed_body)?;

        debug!("Invoking agent {worker_id}");

        let agent_response = self
            .invoke_agent(
                &worker_id,
                resolved_route,
                method_params,
                behaviour,
                request,
            )
            .await?;

        debug!("Received agent response: {agent_response:?}");

        debug!(
            "Json agent response: {}",
            agent_response.clone().unwrap().to_json_value().unwrap()
        );

        let route_result =
            interpret_agent_response(agent_response, &behaviour.expected_agent_response)?;

        debug!("Returning call agent route result: {route_result:?}");

        Ok(route_result)
    }

    fn build_worker_id(
        &self,
        resolved_route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
    ) -> Result<WorkerId, RequestHandlerError> {
        let CallAgentBehaviour {
            component_id,
            agent_type,
            constructor_parameters,
            phantom,
            ..
        } = behaviour;

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
        behaviour: &CallAgentBehaviour,
        mut body: ParsedRequestBody,
    ) -> Result<Vec<UntypedElementValue>, RequestHandlerError> {
        let query_params = request.query_params();
        let headers = request.headers();

        let mut values = Vec::with_capacity(behaviour.method_parameters.len());

        for param in &behaviour.method_parameters {
            let value = match param {
                MethodParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters
                        [usize::from(*path_segment_index)]
                    .clone();

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

                MethodParameter::JsonObjectBodyField { field_index } => match &body {
                    ParsedRequestBody::JsonBody(golem_wasm::Value::Record(fields)) => {
                        UntypedElementValue::ComponentModel(
                            fields[usize::from(*field_index)].clone(),
                        )
                    }

                    ParsedRequestBody::JsonBody(_) => {
                        return Err(RequestHandlerError::invariant_violated(
                            "Inconsistent API definition: JSON field parameter but body is not an object",
                        ));
                    }

                    _ => {
                        return Err(RequestHandlerError::invariant_violated(
                            "JSON body parameter used but no JSON body schema",
                        ));
                    }
                },

                MethodParameter::UnstructuredBinaryBody => match &mut body {
                    ParsedRequestBody::UnstructuredBinary(binary_source) => {
                        let binary_source = binary_source.take().ok_or_else(|| {
                            RequestHandlerError::invariant_violated(
                                "Parsed body was already consumed",
                            )
                        })?;

                        UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                            value: BinaryReference::Inline(binary_source),
                        })
                    }

                    _ => {
                        return Err(RequestHandlerError::invariant_violated(
                            "Binary body parameter used but no binary body schema",
                        ));
                    }
                },
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
        behaviour: &CallAgentBehaviour,
        request: &RichRequest,
    ) -> Result<Option<golem_wasm::ValueAndType>, RequestHandlerError> {
        let method_params_data_value = UntypedDataValue::Tuple(params);

        let principal = principal_from_request(request)?;

        self.worker_service
            .invoke_and_await_owned_agent(
                worker_id,
                Some(IdempotencyKey::fresh()),
                "golem:agent/guest.{invoke}".to_string(),
                vec![
                    golem_wasm::protobuf::Val::from(behaviour.method_name.clone().into_value()),
                    golem_wasm::protobuf::Val::from(method_params_data_value.into_value()),
                    golem_wasm::protobuf::Val::from(principal.into_value()),
                ],
                Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                    parent: None,
                    env: Default::default(),
                    wasi_config_vars: Some(BTreeMap::new().into()),
                    tracing: Some(request.invocation_context().into()),
                }),
                resolved_route.route.environment_id,
                resolved_route.route.account_id,
                AuthCtx::impersonated_user(resolved_route.route.account_id),
            )
            .await
            .map_err(Into::into)
    }
}

fn principal_from_request(request: &RichRequest) -> Result<Principal, RequestHandlerError> {
    match request.authenticated_session() {
        Some(session) => Ok(Principal::Oidc(OidcPrincipal {
            sub: session.subject.clone(),
            issuer: session.issuer.clone(),
            email: session.email.clone(),
            name: session.name.clone(),
            email_verified: session.email_verified,
            given_name: session.given_name.clone(),
            family_name: session.family_name.clone(),
            picture: session.picture.clone(),
            preferred_username: session.preferred_username.clone(),
            claims: serde_json::to_string(&session.claims)
                .map_err(|e| anyhow!("CoreIdTokenClaims serialization error: {e}"))?,
        })),
        None => Ok(Principal::anonymous()),
    }
}
