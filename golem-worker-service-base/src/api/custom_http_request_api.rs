use std::sync::Arc;

use async_trait::async_trait;
use hyper::header::HOST;
use poem::{Body, Endpoint, Request, Response};
use poem::http::StatusCode;
use tracing::{error, info};
use crate::api_definition::http::HttpApiDefinition;

use crate::http::{ApiInputPath, InputHttpRequest};
use crate::service::http_request_definition_lookup::ApiDefinitionLookup;

use crate::worker_binding::worker_binding_resolver::WorkerBindingResolver;
use crate::worker_bridge::WorkerRequest;
use crate::worker_bridge::worker_request_executor::WorkerRequestExecutor;

// Executes custom request with the help of worker_request_executor and definition_service
// This is a common API projects can make use of, similar to healthcheck service
#[derive(Clone)]
pub struct CustomHttpRequestApi {
    pub worker_to_http_response_service:
        Arc<dyn WorkerRequestExecutor<Response> + Sync + Send>,
    pub api_definition_lookup_service: Arc<dyn ApiDefinitionLookup<InputHttpRequest, HttpApiDefinition> + Sync + Send>,
}

impl CustomHttpRequestApi {
    pub fn new(
        worker_to_http_response_service: Arc<
            dyn WorkerRequestExecutor<Response> + Sync + Send,
        >,
        api_definition_lookup_service: Arc<dyn ApiDefinitionLookup<InputHttpRequest, HttpApiDefinition> + Sync + Send>,
    ) -> Self {
        Self {
            worker_to_http_response_service,
            api_definition_lookup_service,
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

        let api_request = InputHttpRequest {
            input_path: ApiInputPath {
                base_path: uri.path().to_string(),
                query_path: uri.query().map(|x| x.to_string())
            },
            headers,
            req_method: req_parts.method,
            req_body: json_request_body,
        };

        let api_definition = match self.api_definition_lookup_service.get(api_request.clone()).await {
            Ok(api_definition) => api_definition,
            Err(err) => {
                error!("API request host: {} - error: {}", host, err);
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from_string("Internal error".to_string()));
            }
        };

        match api_request.resolve(&api_definition) {
            Some(resolved_route) => {
                let resolved_worker_request =
                    match WorkerRequest::from_resolved_route(resolved_route.clone()) {
                        Ok(golem_worker_request) => golem_worker_request,
                        Err(e) => {
                            error!(
                                "API request id: {} - request error: {}",
                                &api_definition.id, e
                            );
                            return Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(Body::from_string(
                                    format!("API request error {}", e).to_string(),
                                ));
                        }
                    };

                // Execute the request using a executor
                match self.worker_to_http_response_service
                    .execute(
                        resolved_worker_request.clone(),
                    )
                    .await {
                    Ok(worker_response) => {
                        worker_response.to_http_response(&resolved_route.resolved_worker_binding_template.response,  &resolved_route.typed_value_from_input)
                    }

                    Err(e) => {
                        error!(
                            "API request id: {} - request error: {}",
                            &api_definition.id, e
                        );
                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Body::from_string(
                                format!("API request error {}", e).to_string(),
                            ))
                    }

                }
            }

            None => {
                error!(
                    "API request id: {} - request error: {}",
                    &api_definition.id, "Unable to find a route"
                );

                Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .finish()
            }
        }
    }
}

#[async_trait]
impl Endpoint for CustomHttpRequestApi {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Self::Output> {
        let result = self.execute(req).await;
        Ok(result)
    }
}
