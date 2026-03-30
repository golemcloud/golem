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

use super::environment_state::EnvironmentStateService;
use golem_common::model::PromiseId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::AgentWebhookId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;

pub struct AgentWebhooksService {
    environment_state_service: Arc<dyn EnvironmentStateService>,
    use_https_for_webhook_url: bool,
    hmac_key: Vec<u8>,
}

impl AgentWebhooksService {
    pub fn new(
        environment_state_service: Arc<dyn EnvironmentStateService>,
        use_https_for_webhook_url: bool,
        hmac_key: Vec<u8>,
    ) -> Self {
        Self {
            environment_state_service,
            use_https_for_webhook_url,
            hmac_key,
        }
    }

    pub async fn get_agent_webhook_url_for_promise(
        &self,
        environment: EnvironmentId,
        agent_type: &AgentTypeName,
        promise_id: &PromiseId,
    ) -> Result<Option<String>, WorkerExecutorError> {
        let Some(webhook_prefix_authority_and_path) = self
            .environment_state_service
            .get_agent_deployment(environment, agent_type)
            .await?
            .and_then(|ad| ad.webhook_prefix_authority_and_path)
        else {
            return Ok(None);
        };

        let webhook_id = AgentWebhookId::from_promise_id(promise_id, &self.hmac_key);
        let encoded_webhook_id = webhook_id.to_base64_url();

        let protocol = if self.use_https_for_webhook_url {
            "https"
        } else {
            "http"
        };

        Ok(Some(format!(
            "{}://{}/{}",
            protocol, webhook_prefix_authority_and_path, encoded_webhook_id
        )))
    }
}
