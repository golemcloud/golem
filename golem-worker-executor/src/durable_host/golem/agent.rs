// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::Host;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::agent::bindings::golem::agent::common::{
    AgentError, DataValue, RegisteredAgentType,
};
use golem_common::model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentTypeName, ParsedAgentId,
};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::oplog::host_functions::{
    GolemAgentCreateWebhook, GolemAgentGetAgentType, GolemAgentGetAllAgentTypes,
    GolemAgentGetConfigValue,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemAgentGetAgentType, HostRequestGolemAgentGetConfigValue,
    HostRequestGolemApiPromiseId, HostRequestNoInput, HostResponseGolemAgentAgentType,
    HostResponseGolemAgentAgentTypes, HostResponseGolemAgentGetConfigValue,
    HostResponseGolemAgentWebhookUrl,
};
use golem_common::model::PromiseId;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{NodeBuilder, WitType, WitValue, WitValueBuilderExtensions};

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    fn resolve_local_config(
        &self,
        key: &[String],
        key_str: &str,
        expected_type: &AnalysedType,
        declared_type: &AnalysedType,
    ) -> anyhow::Result<WitValue> {
        let config_value = self.state.agent_config.get(key);

        if expected_type != declared_type {
            return Err(anyhow!(
                "declared and expected type for config key {key_str} are not compatible"
            ));
        }

        let result = match (expected_type, config_value) {
            (AnalysedType::Option(_), None) => WitValue::builder().option_none(),
            (_, Some(value)) => value.value.clone().into(),
            (_, None) => return Err(anyhow!("required config key {key_str} is missing value")),
        };

        Ok(result)
    }

    async fn resolve_secret_config(
        &mut self,
        path: Vec<String>,
        path_str: &str,
        expected_type: AnalysedType,
        declared_type: &AnalysedType,
    ) -> anyhow::Result<WitValue> {
        let durability =
            Durability::<GolemAgentGetConfigValue>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        if durability.is_live() {
            let agent_secrets = self
                .state
                .environment_state_service
                .get_agent_secrets(self.state.component_metadata.environment_id)
                .await?;

            let canonical_agent_secret_path =
                CanonicalAgentSecretPath::from_path_in_unknown_casing(&path);
            let agent_secret = agent_secrets.get(&canonical_agent_secret_path);

            let agent_secret_type = agent_secret.map(|sec| &sec.secret_type);
            let agent_secret_value = agent_secret.and_then(|sec| sec.secret_value.as_ref());

            if *declared_type != expected_type {
                return Err(anyhow!(
                    "declared and expected type for secret key {path_str} are not compatible"
                ));
            }

            let result = match (&expected_type, agent_secret_type, agent_secret_value) {
                (AnalysedType::Option(_), None, None) => golem_wasm::Value::Option(None),

                (
                    AnalysedType::Option(expected_type),
                    Some(AnalysedType::Option(actual_type)),
                    None,
                ) if *expected_type == *actual_type => golem_wasm::Value::Option(None),

                (expected_type, Some(actual_type), Some(value)) if expected_type == actual_type => {
                    value.clone()
                }

                (_, None, _) => {
                    return Err(anyhow!(
                        "No secret for key {path_str} exists in environment"
                    ));
                }

                (_, Some(_), None) => {
                    return Err(anyhow!("Secret key {path_str} is missing value"));
                }

                (_, _, _) => {
                    return Err(anyhow!(
                        "declared and expected type for config key {path_str} are not compatible"
                    ));
                }
            };

            let persisted = durability
                .persist(
                    self,
                    HostRequestGolemAgentGetConfigValue {
                        path,
                        expected_type,
                    },
                    HostResponseGolemAgentGetConfigValue { result },
                )
                .await?;

            Ok(persisted.result.into())
        } else {
            Ok(durability.replay(self).await?.result.into())
        }
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_agent_types(&mut self) -> anyhow::Result<Vec<RegisteredAgentType>> {
        let mut durability =
            Durability::<GolemAgentGetAllAgentTypes>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
            let result = loop {
                let result = self
                    .agent_types_service()
                    .get_all(
                        self.owned_agent_id.environment_id,
                        self.owned_agent_id.agent_id.component_id,
                        self.state.component_metadata.revision,
                    )
                    .await
                    .map_err(|err| err.to_string());
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemAgentAgentTypes { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(result) => Ok(result.into_iter().map(|r| r.into()).collect()),
            Err(err) => Err(anyhow!(err)),
        }
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<Option<RegisteredAgentType>> {
        let agent_type_name = AgentTypeName(agent_type_name);
        let mut durability =
            Durability::<GolemAgentGetAgentType>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
            let component_revision = self.state.component_metadata.revision;
            let result = loop {
                let result = self
                    .agent_types_service()
                    .get(
                        self.owned_agent_id.environment_id,
                        self.owned_agent_id.agent_id.component_id,
                        component_revision,
                        &agent_type_name,
                    )
                    .await
                    .map_err(|err| err.to_string());
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            durability
                .persist(
                    self,
                    HostRequestGolemAgentGetAgentType { agent_type_name },
                    HostResponseGolemAgentAgentType { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(result) => Ok(result.map(|r| r.into())),
            Err(err) => Err(anyhow!(err)),
        }
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
    ) -> anyhow::Result<Result<String, AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "make_agent_id");

        if let Some(agent_type) = self.get_agent_type(agent_type_name.clone()).await? {
            match golem_common::model::agent::DataValue::try_from_bindings(
                input,
                agent_type.agent_type.constructor.input_schema,
            ) {
                Ok(input) => {
                    let agent_id = ParsedAgentId::new(
                        AgentTypeName(agent_type_name),
                        input,
                        phantom_id.map(|id| id.into()),
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                    Ok(Ok(agent_id.to_string()))
                }
                Err(err) => Ok(Err(AgentError::InvalidInput(err))),
            }
        } else {
            Ok(Err(AgentError::InvalidType(agent_type_name)))
        }
    }

    async fn parse_agent_id(
        &mut self,
        agent_id: String,
    ) -> anyhow::Result<Result<(String, DataValue, Option<golem_wasm::Uuid>), AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "parse_agent_id");

        let component_metadata = &self.component_metadata().metadata;
        match ParsedAgentId::parse(agent_id, component_metadata) {
            Ok(agent_id) => Ok(Ok((
                agent_id.agent_type.to_string(),
                agent_id.parameters.into(),
                agent_id.phantom_id.map(|id| id.into()),
            ))),
            Err(error) => Ok(Err(AgentError::InvalidAgentId(error))),
        }
    }

    async fn create_webhook(
        &mut self,
        promise_id: crate::preview2::golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<String> {
        let durability =
            Durability::<GolemAgentCreateWebhook>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        if durability.is_live() {
            let promise_id: PromiseId = promise_id.clone().into();
            if promise_id.agent_id.component_id != self.state.component_metadata.id {
                let error = "Attempted to create a webhook for a promise not created by the current component".to_string();
                let persisted = durability
                    .persist(
                        self,
                        HostRequestGolemApiPromiseId { promise_id },
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
                return persisted.result.map_err(|e| anyhow!(e));
            }

            let Some(agent_id) = self.state.agent_id.as_ref() else {
                let error =
                    "Creating webhook urls is only supported for agentic components".to_string();
                let persisted = durability
                    .persist(
                        self,
                        HostRequestGolemApiPromiseId { promise_id },
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
                return persisted.result.map_err(|e| anyhow!(e));
            };

            let webhook_url = self
                .state
                .agent_webhooks_service
                .get_agent_webhook_url_for_promise(
                    self.state.component_metadata.environment_id,
                    &agent_id.agent_type,
                    &promise_id,
                )
                .await?;

            let Some(webhook_url) = webhook_url else {
                return Err(anyhow!(
                    "Agent is not currently deployed as part of an http api. Only deployed agents can create webhook urls"
                ));
            };

            let persisted = durability
                .persist(
                    self,
                    HostRequestGolemApiPromiseId { promise_id },
                    HostResponseGolemAgentWebhookUrl {
                        result: Ok(webhook_url),
                    },
                )
                .await?;

            Ok(persisted.result.map_err(|e| anyhow!(e))?)
        } else {
            Ok(durability
                .replay(self)
                .await?
                .result
                .map_err(|e| anyhow!(e))?)
        }
    }

    async fn get_config_value(
        &mut self,
        path: Vec<String>,
        expected_type: WitType,
    ) -> anyhow::Result<WitValue> {
        let path_str = path.join(".");
        tracing::debug!("Agent getting config value for key {path_str}");

        let agent_id = self
            .parsed_agent_id()
            .ok_or_else(|| anyhow!("only agentic workers can access agent config"))?;

        let expected_type = AnalysedType::from(expected_type);

        let agent_type = self
            .component_metadata()
            .metadata
            .find_agent_type_by_name(&agent_id.agent_type)
            .expect("Active agent type of agent was not declared in component metadata");

        let declaration = agent_type.config.iter().find(|c| c.path == path);

        match declaration {
            // Allow reading undeclared optional config keys so that
            // newer agents can run against older component schemas.
            None if matches!(expected_type, AnalysedType::Option(_)) => {
                Ok(WitValue::builder().option_none())
            }
            None => Err(anyhow!("No config declared for path {path_str}")),
            Some(AgentConfigDeclaration {
                source: AgentConfigSource::Local,
                value_type,
                ..
            }) => self.resolve_local_config(&path, &path_str, &expected_type, value_type),
            Some(AgentConfigDeclaration {
                source: AgentConfigSource::Secret,
                value_type,
                ..
            }) => {
                self.resolve_secret_config(path, &path_str, expected_type, value_type)
                    .await
            }
        }
    }
}
