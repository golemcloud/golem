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

use crate::api::agents::{AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult};
use crate::service::component::ComponentService;
use crate::service::worker::{WorkerResult, WorkerService, WorkerServiceError};
use golem_common::model::WorkerId;
use golem_common::model::agent::{AgentError, AgentId, DataValue, UntypedDataValue};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::{FromValue, IntoValueAndType, Value};
use std::sync::Arc;

pub struct AgentsService {
    registry_service: Arc<dyn RegistryService>,
    component_service: Arc<dyn ComponentService>,
    worker_service: Arc<WorkerService>,
}

impl AgentsService {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        component_service: Arc<dyn ComponentService>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            registry_service,
            component_service,
            worker_service,
        }
    }

    pub async fn invoke_agent(
        &self,
        request: AgentInvocationRequest,
        auth: AuthCtx,
    ) -> WorkerResult<AgentInvocationResult> {
        let registered_agent_type = self
            .registry_service
            .resolve_latest_agent_type_by_names(
                &auth.account_id(),
                &request.app_name,
                &request.env_name,
                &request.agent_type_name,
            )
            .await?;

        let component_metadata = self
            .component_service
            .get_revision(
                registered_agent_type.implemented_by.component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;

        let agent_type = component_metadata
            .metadata
            .find_agent_type_by_name(&request.agent_type_name)
            .map_err(|err| {
                WorkerServiceError::Internal(format!(
                    "Cannot get agent type {} from component metadata: {err}",
                    request.agent_type_name
                ))
            })?
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent type {} not found in component metadata",
                    request.agent_type_name
                ))
            })?;

        let constructor_parameters: DataValue = DataValue::try_from_untyped_json(
            request.parameters,
            agent_type.constructor.input_schema,
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;

        let agent_id = AgentId::new(
            request.agent_type_name.clone(),
            constructor_parameters,
            request.phantom_id,
        );

        let worker_id = WorkerId {
            component_id: component_metadata.id,
            worker_name: agent_id.to_string(),
        };

        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == request.method_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent method {} not found in agent type {}",
                    request.method_name, request.agent_type_name
                ))
            })?;

        let method_parameters: DataValue = DataValue::try_from_untyped_json(
            request.method_parameters,
            method.input_schema.clone(),
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!("Agent method parameters type error: {err}"))
        })?;

        // invoke: func(method-name: string, input: data-value) -> result<data-value, agent-error>;

        match request.mode {
            AgentInvocationMode::Await => {
                let invoke_result = self
                    .worker_service
                    .invoke_and_await_typed(
                        &worker_id,
                        request.idempotency_key,
                        "golem:agent/guest.{invoke}".to_string(),
                        vec![
                            request.method_name.into_value_and_type(),
                            method_parameters.into_value_and_type(),
                            // Fixme: this needs to come from the invocation that caused this agent to be created
                            golem_common::model::agent::Principal::anonymous()
                                .into_value_and_type(),
                        ],
                        None,
                        auth,
                    )
                    .await?;

                match invoke_result {
                    Some(value_and_type) => {
                        // return value has type 'result<data-value, agent-error>'
                        let result = match value_and_type.value {
                            Value::Result(Ok(Some(data_value_value))) => {
                                let untyped_data_value = UntypedDataValue::from_value(
                                    *data_value_value,
                                )
                                .map_err(|err| {
                                    WorkerServiceError::Internal(format!(
                                        "Unexpected DataValue value: {err}"
                                    ))
                                })?;
                                Ok(DataValue::try_from_untyped(
                                    untyped_data_value,
                                    method.output_schema.clone(),
                                )
                                .map_err(|err| {
                                    WorkerServiceError::TypeChecker(format!(
                                        "DataValue conversion error: {err}"
                                    ))
                                })?)
                            }
                            Value::Result(Err(Some(agent_error_value))) => {
                                Err(AgentError::from_value(*agent_error_value).map_err(|err| {
                                    WorkerServiceError::Internal(format!(
                                        "Unexpected AgentError value: {err}"
                                    ))
                                })?)
                            }
                            _ => Err(WorkerServiceError::Internal(
                                "Unexpected return value from agent invocation".to_string(),
                            ))?,
                        };
                        match result {
                            Ok(data_value) => Ok(AgentInvocationResult {
                                result: Some(data_value.into()),
                            }),
                            Err(err) => Err(WorkerServiceError::Internal(format!(
                                "Agent invocation failed: {err}"
                            ))),
                        }
                    }
                    None => Err(WorkerServiceError::Internal(
                        "Unexpected missing invoke result value".to_string(),
                    )),
                }
            }
            AgentInvocationMode::Schedule => {
                if let Some(_schedule_at) = request.schedule_at {
                    // schedule at time
                    // TODO
                    Err(WorkerServiceError::Internal("Not implemented".to_string()))?
                } else {
                    // trigger
                    self.worker_service
                        .invoke_typed(
                            &worker_id,
                            request.idempotency_key,
                            "golem:agent/guest.{invoke}".to_string(),
                            vec![
                                request.method_name.into_value_and_type(),
                                method_parameters.into_value_and_type(),
                            ],
                            None,
                            auth,
                        )
                        .await?;
                    Ok(AgentInvocationResult { result: None })
                }
            }
        }
    }
}
