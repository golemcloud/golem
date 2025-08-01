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

use std::future::Future;
use std::sync::Arc;

use crate::gateway_execution::api_definition_lookup::HttpApiDefinitionsLookup;
use crate::gateway_execution::auth_call_back_binding_handler::DefaultAuthCallBack;
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_execution::gateway_http_input_executor::{
    DefaultGatewayInputExecutor, GatewayHttpInputExecutor,
};
use crate::gateway_execution::gateway_session::GatewaySession;
use crate::gateway_execution::http_handler_binding_handler::HttpHandlerBindingHandler;
use crate::gateway_execution::GatewayWorkerRequestExecutor;
use crate::gateway_rib_interpreter::DefaultRibInterpreter;
use crate::gateway_security::DefaultIdentityProvider;
use futures::FutureExt;
use poem::{Endpoint, Request, Response};

pub struct CustomHttpRequestApi {
    pub gateway_http_input_executor: Arc<dyn GatewayHttpInputExecutor>,
}

impl CustomHttpRequestApi {
    pub fn new(
        worker_request_executor_service: Arc<dyn GatewayWorkerRequestExecutor>,
        api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler>,
        http_handler_binding_handler: Arc<dyn HttpHandlerBindingHandler>,
        gateway_session_store: Arc<dyn GatewaySession>,
    ) -> Self {
        let evaluator = Arc::new(DefaultRibInterpreter::from_worker_request_executor(
            worker_request_executor_service.clone(),
        ));

        let auth_call_back_binding_handler = Arc::new(DefaultAuthCallBack);

        let gateway_http_input_executor = Arc::new(DefaultGatewayInputExecutor {
            evaluator,
            file_server_binding_handler,
            auth_call_back_binding_handler,
            http_handler_binding_handler,
            api_definition_lookup_service,
            gateway_session_store,
            identity_provider: Arc::new(DefaultIdentityProvider),
        });

        Self {
            gateway_http_input_executor,
        }
    }

    pub async fn execute(&self, request: Request) -> Response {
        self.gateway_http_input_executor
            .execute_http_request(request)
            .await
    }
}

impl Endpoint for CustomHttpRequestApi {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}
