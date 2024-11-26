// Copyright 2024 Golem Cloud
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

use std::future::Future;
use std::sync::Arc;

use crate::gateway_api_definition::http::CompiledHttpApiDefinition;
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_rib_interpreter::DefaultRibInterpreter;
use futures_util::FutureExt;
use poem::http::StatusCode;
use poem::{Body, Endpoint, Request, Response};
use tracing::error;

use crate::gateway_execution::api_definition_lookup::ApiDefinitionsLookup;

use crate::gateway_binding::{GatewayBindingResolver, GatewayRequestDetails};
use crate::gateway_execution::auth_call_back_binding_handler::DefaultAuthCallBack;
use crate::gateway_execution::gateway_http_input_executor::{
    DefaultGatewayInputExecutor, GatewayHttpInputExecutor, GatewayHttpInput,
};
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::GatewayWorkerRequestExecutor;
use crate::gateway_request::http_request::InputHttpRequest;
use crate::gateway_security::DefaultIdentityProviderResolver;

// Executes custom request with the help of worker_request_executor and definition_service
// This is a common API projects can make use of, similar to healthcheck service
#[derive(Clone)]
pub struct CustomHttpRequestApi<Namespace> {
    pub api_definition_lookup_service: Arc<
        dyn ApiDefinitionsLookup<
                InputHttpRequest,
                ApiDefinition = CompiledHttpApiDefinition<Namespace>,
            > + Sync
            + Send,
    >,
    pub gateway_binding_executor:
        Arc<dyn GatewayHttpInputExecutor<Namespace> + Sync + Send>,
    pub gateway_session_store: GatewaySessionStore,
}

impl<Namespace: Clone + Send + Sync + 'static> CustomHttpRequestApi<Namespace> {
    pub fn new(
        worker_request_executor_service: Arc<
            dyn GatewayWorkerRequestExecutor<Namespace> + Sync + Send,
        >,
        api_definition_lookup_service: Arc<
            dyn ApiDefinitionsLookup<
                    InputHttpRequest,
                    ApiDefinition = CompiledHttpApiDefinition<Namespace>,
                > + Sync
                + Send,
        >,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    ) -> Self {
        let evaluator = Arc::new(DefaultRibInterpreter::from_worker_request_executor(
            worker_request_executor_service.clone(),
        ));

        let auth_call_back_binding_handler = Arc::new(DefaultAuthCallBack);

        let gateway_binding_executor = Arc::new(DefaultGatewayInputExecutor {
            evaluator,
            file_server_binding_handler,
            auth_call_back_binding_handler,
        });

        let gateway_session_store = GatewaySessionStore::in_memory();

        Self {
            api_definition_lookup_service,
            gateway_binding_executor,
            gateway_session_store,
        }
    }

    pub async fn execute(&self, request: Request) -> Response {
        let input_http_request_result = InputHttpRequest::from_request(request).await;

        match input_http_request_result {
            Ok(input_http_request) => {
                let possible_api_definitions = match self
                    .api_definition_lookup_service
                    .get(&input_http_request)
                    .await
                {
                    Ok(api_defs) => api_defs,
                    Err(api_defs_lookup_error) => {
                        error!(
                            "API request host: {} - error: {}",
                            input_http_request.host, api_defs_lookup_error
                        );
                        return Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Body::from_string("Internal error".to_string()));
                    }
                };

                match input_http_request
                    .resolve_gateway_binding(possible_api_definitions)
                    .await
                {

                    Ok(resolved_gateway_binding) => {
                        let request = match resolved_gateway_binding.request_details {
                            GatewayRequestDetails::Http(http) => http
                        };

                        let input = GatewayHttpInput::new(
                            &request,
                            resolved_gateway_binding.resolved_binding,
                            &self.gateway_session_store,
                            Arc::new(DefaultIdentityProviderResolver),
                        );
                        let response: poem::Response =
                            self.gateway_binding_executor.execute_binding(&input).await;

                        response
                    }

                    Err(msg) => {
                        error!("Failed to resolve the API definition; error: {}", msg);

                        Response::builder()
                            .status(StatusCode::METHOD_NOT_ALLOWED)
                            .finish()
                    }
                }
            }
            Err(response) => response.into(),
        }
    }
}

impl<Namespace: Clone + Send + Sync + 'static> Endpoint for CustomHttpRequestApi<Namespace> {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}
