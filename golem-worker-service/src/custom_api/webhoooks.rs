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

use super::RichRequest;
use super::error::RequestHandlerError;
use super::route_resolver::ResolvedRouteEntry;
use super::{ParsedRequestBody, RouteExecutionResult};
use crate::custom_api::ResponseBody;
use crate::service::worker::WorkerService;
use golem_service_base::custom_api::{AgentWebhookId, RequestBodySchema, WebhookCallbackBehaviour};
use golem_service_base::model::auth::AuthCtx;
use http::StatusCode;
use std::collections::HashMap;
use std::sync::Arc;

pub struct WebhookCallbackHandler {
    worker_service: Arc<WorkerService>,
    hmac_key: Vec<u8>,
}

impl WebhookCallbackHandler {
    pub fn new(worker_service: Arc<WorkerService>, hmac_key: Vec<u8>) -> Self {
        Self {
            worker_service,
            hmac_key,
        }
    }

    pub async fn handle_webhook_callback_behaviour(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
        behaviour: &WebhookCallbackBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let webhook_id_segment = resolved_route.captured_path_parameters.first().ok_or(
            RequestHandlerError::invariant_violated(
                "no variable path segments for webhook callback ",
            ),
        )?;

        let webhook_id = AgentWebhookId::from_base64_url(webhook_id_segment).map_err(|_| {
            RequestHandlerError::ValueParsingFailed {
                value: webhook_id_segment.clone(),
                expected: "AgentWebhookId",
            }
        })?;

        // check that worker-executor signed the webhook id. If this fails it means createWebhook was not called / the webhook was created manually.
        // return 404 as logically the webhook resource does not exist in this case;
        if !webhook_id.verify_checksum(behaviour.component_id, &self.hmac_key) {
            tracing::warn!("Received webhook callback with incorrect checksum");
            return Ok(RouteExecutionResult {
                status: StatusCode::NOT_FOUND,
                headers: HashMap::new(),
                body: ResponseBody::NoBody,
            });
        }

        let promise_id = webhook_id.into_promise_id(behaviour.component_id);

        let body = request
            .parse_request_body(&RequestBodySchema::UnrestrictedBinary)
            .await?;

        let ParsedRequestBody::UnstructuredBinary(mut body_data) = body else {
            return Err(RequestHandlerError::invariant_violated(
                "UnrestrictedBinary body parsing yielded wrong type",
            ));
        };

        let body_binary = body_data.take().map(|bs| bs.data).unwrap_or_default();

        let auth_ctx = AuthCtx::impersonated_user(resolved_route.route.account_id);

        tracing::debug!("Completing promise due to webhook_callback: {promise_id}");
        self.worker_service
            .complete_promise(
                &promise_id.worker_id,
                promise_id.oplog_idx.as_u64(),
                body_binary,
                auth_ctx,
            )
            .await?;

        Ok(RouteExecutionResult {
            status: StatusCode::NO_CONTENT,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        })
    }
}
