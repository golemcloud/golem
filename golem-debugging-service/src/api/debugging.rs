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

use super::errors::DebuggingApiError;
use crate::jrpc::run_jrpc_debug_websocket_session;
use crate::services::auth::AuthService;
use crate::services::debug_service::DebugService;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::WrappedGolemSecuritySchema;
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket};
use poem_openapi::*;
use std::sync::Arc;

pub struct DebuggingApi {
    debug_service: Arc<dyn DebugService>,
    auth_service: Arc<dyn AuthService>,
}

#[OpenApi(prefix_path = "/v1/debugger", tag = ApiTags::Debugging)]
impl DebuggingApi {
    pub fn new(debug_service: Arc<dyn DebugService>, auth_service: Arc<dyn AuthService>) -> Self {
        Self {
            debug_service,
            auth_service,
        }
    }

    /// Start a new debugging sessions
    #[oai(path = "/", method = "get", operation_id = "debugger_start")]
    pub async fn get_debugger(
        &self,
        websocket: WebSocket,
        token: WrappedGolemSecuritySchema,
    ) -> Result<BoxWebSocketUpgraded, DebuggingApiError> {
        let debug_service = self.debug_service.clone();
        let auth_ctx = self
            .auth_service
            .authenticate_token(token.0.secret())
            .await?;
        let upgraded: BoxWebSocketUpgraded = websocket.on_upgrade(Box::new(|socket_stream| {
            Box::pin(run_jrpc_debug_websocket_session(
                socket_stream,
                debug_service,
                auth_ctx,
            ))
        }));

        Ok(upgraded)
    }
}
