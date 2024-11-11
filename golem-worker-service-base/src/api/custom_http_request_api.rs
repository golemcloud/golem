use std::future::Future;
use std::sync::Arc;

use crate::gateway_api_definition::http::CompiledHttpApiDefinition;
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_rib_interpreter::DefaultRibInterpreter;
use futures_util::FutureExt;
use hyper::header::HOST;
use poem::http::StatusCode;
use poem::{Body, Endpoint, Request, Response};
use tracing::{error, info};

use crate::gateway_execution::api_definition_lookup::ApiDefinitionsLookup;

use crate::gateway_binding::GatewayBindingResolver;
use crate::gateway_execution::gateway_binding_executor::{
    DefaultGatewayBindingExecutor, GatewayBindingExecutor,
};
use crate::gateway_execution::GatewayWorkerRequestExecutor;
use crate::gateway_request::http_request::{ApiInputPath, InputHttpRequest};

// Executes custom request with the help of worker_request_executor and definition_service
// This is a common API projects can make use of, similar to healthcheck service
#[derive(Clone)]
pub struct CustomHttpRequestApi<Namespace> {
    pub api_definition_lookup_service: Arc<
        dyn ApiDefinitionsLookup<InputHttpRequest, CompiledHttpApiDefinition<Namespace>>
            + Sync
            + Send,
    >,
    pub gateway_binding_executor:
        Arc<dyn GatewayBindingExecutor<Namespace, poem::Response> + Sync + Send>,
}

impl<Namespace: Clone + Send + Sync + 'static> CustomHttpRequestApi<Namespace> {
    pub fn new(
        worker_request_executor_service: Arc<dyn GatewayWorkerRequestExecutor + Sync + Send>,
        api_definition_lookup_service: Arc<
            dyn ApiDefinitionsLookup<InputHttpRequest, CompiledHttpApiDefinition<Namespace>>
                + Sync
                + Send,
        >,
        fileserver_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    ) -> Self {
        let evaluator = Arc::new(DefaultRibInterpreter::from_worker_request_executor(
            worker_request_executor_service.clone(),
        ));

        let gateway_binding_executor = Arc::new(DefaultGatewayBindingExecutor {
            evaluator: evaluator.clone(),
            file_server_binding_handler: fileserver_binding_handler.clone(),
        });

        Self {
            api_definition_lookup_service,
            gateway_binding_executor,
        }
    }

    pub async fn execute(&self, request: Request) -> Response {
        let (req_parts, body) = request.into_parts();
        let headers = req_parts.headers;
        let uri = req_parts.uri;

        let host = match headers.get(HOST).and_then(|h| h.to_str().ok()) {
            Some(host) => host.to_string(),
            None => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string("Missing host".to_string()));
            }
        };

        info!("API request host: {}", host);

        let json_request_body: serde_json::Value = if body.is_empty() {
            serde_json::Value::Null
        } else {
            match body.into_json().await {
                Ok(json_request_body) => json_request_body,
                Err(err) => {
                    error!("API request host: {} - error: {}", host, err);
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from_string("Request body parse error".to_string()));
                }
            }
        };

        let input_http_request = InputHttpRequest {
            input_path: ApiInputPath {
                base_path: uri.path().to_string(),
                query_path: uri.query().map(|x| x.to_string()),
            },
            headers,
            req_method: req_parts.method,
            req_body: json_request_body,
        };

        let possible_api_definitions = match self
            .api_definition_lookup_service
            .get(input_http_request.clone())
            .await
        {
            Ok(api_defs) => api_defs,
            Err(api_defs_lookup_error) => {
                error!(
                    "API request host: {} - error: {}",
                    host, api_defs_lookup_error
                );
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from_string("Internal error".to_string()));
            }
        };

        match input_http_request
            .resolve_worker_binding(possible_api_definitions)
            .await
        {
            Ok(resolved_gateway_binding) => {
                let response: poem::Response = self
                    .gateway_binding_executor
                    .execute_binding(&resolved_gateway_binding)
                    .await;

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
}

impl<Namespace: Clone + Send + Sync + 'static> Endpoint for CustomHttpRequestApi<Namespace> {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}
