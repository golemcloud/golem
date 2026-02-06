// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::await_pollable;
use crate::bindings::golem::api::host::GetPromiseResult;
use crate::golem_agentic::golem::agent::host::AgentWebhook;
use crate::{get_promise, PromiseId};
use std::future::Future;
use std::future::IntoFuture;
use std::pin::Pin;
use std::str::FromStr;

pub fn create_webhook() -> WebhookHandler {
    let agent_webhook = crate::golem_agentic::golem::agent::host::create_webhook();

    let url = agent_webhook.get_callback_url();

    WebhookHandler::new(url, agent_webhook)
}

pub struct WebhookHandler {
    url: String,
    inner: AgentWebhook,
}

impl WebhookHandler {
    fn new(url: String, inner: AgentWebhook) -> WebhookHandler {
        WebhookHandler { url, inner }
    }
    async fn wait(self) -> WebhookRequestPayload {
        let promise_id = self.url.trim_end_matches('/').rsplit('/').next();

        match promise_id {
            Some(promise_id) => {
                let promise_id = PromiseId::from_str(promise_id).expect(&format!(
                    "Internal Error: Invalid webhook URL: {}",
                    self.url
                ));

                let promise_result: GetPromiseResult = get_promise(&promise_id);

                let pollable = self.inner.subscribe();

                await_pollable(pollable).await;

                let bytes = promise_result.get().unwrap();

                WebhookRequestPayload { payload: bytes }
            }

            None => {
                panic!("Internal Error: Invalid webhook URL: {}", self.url);
            }
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl IntoFuture for WebhookHandler {
    type Output = WebhookRequestPayload;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

pub struct WebhookRequestPayload {
    payload: Vec<u8>,
}

impl WebhookRequestPayload {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_slice(&self.payload).map_err(|e| format!("Invalid input: {}", e))
    }
}
