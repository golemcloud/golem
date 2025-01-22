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

use crate::gateway_binding::{HttpRequestDetails, WorkerDetail};
use crate::gateway_execution::GatewayResolvedWorkerRequest;
use crate::service::worker::WorkerService;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::TryStreamExt;
use golem_common::model::HasAccountId;
use golem_common::virtual_exports;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{TypeAnnotatedValueConstructors, Value};
use http::StatusCode;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;

use super::{GatewayWorkerRequestExecutor, WorkerRequestExecutorError};

#[async_trait]
pub trait HttpHandlerBindingHandler<Namespace> {
    async fn handle_http_handler_binding(
        &self,
        namespace: &Namespace,
        worker_detail: &WorkerDetail,
        request_details: &HttpRequestDetails,
    ) -> HttpHandlerBindingResult;
}

pub type HttpHandlerBindingResult = Result<HttpHandlerBindingSuccess, HttpHandlerBindingError>;

pub struct HttpHandlerBindingSuccess {
    pub response: poem::Response,
}

pub enum HttpHandlerBindingError {
    InternalError(String),
    WorkerRequestExecutorError(WorkerRequestExecutorError),
}

pub struct DefaultHttpHandlerBindingHandler<Namespace> {
    worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor<Namespace> + Sync + Send>,
}

impl<Namespace> DefaultHttpHandlerBindingHandler<Namespace> {
    pub fn new(
        worker_request_executor: Arc<dyn GatewayWorkerRequestExecutor<Namespace> + Sync + Send>,
    ) -> Self {
        Self {
            worker_request_executor,
        }
    }
}

#[async_trait]
impl<Namespace: HasAccountId + Send + Sync + Clone + 'static> HttpHandlerBindingHandler<Namespace>
    for DefaultHttpHandlerBindingHandler<Namespace>
{
    async fn handle_http_handler_binding(
        &self,
        namespace: &Namespace,
        worker_detail: &WorkerDetail,
        request_details: &HttpRequestDetails,
    ) -> HttpHandlerBindingResult {
        let component_id = worker_detail.component_id.component_id.clone();

        let http_request_function_param = {
            use golem_common::virtual_exports::http_incoming_handler as hic;

            let headers = {
                let mut acc = Vec::new();
                for header in request_details.request_headers.0.fields.iter() {
                    acc.push((
                        header.name.clone(),
                        Bytes::from(header.value.to_string().into_bytes()),
                    ));
                }
                hic::HttpFields(acc)
            };

            let body = hic::HttpBodyAndTrailers {
                content: hic::HttpBodyContent(Bytes::from(
                    request_details
                        .request_body_value
                        .0
                        .to_string()
                        .into_bytes(),
                )),
                trailers: None,
            };

            hic::IncomingHttpRequest {
                uri: request_details.get_api_input_path(),
                method: hic::HttpMethod::from_http_method(request_details.request_method.clone()),
                headers,
                body: Some(body),
            }
        };

        let typ: golem_wasm_ast::analysis::protobuf::Type = (&golem_common::virtual_exports::http_incoming_handler::IncomingHttpRequest::analysed_type()).into();

        let type_annotated_param =
            TypeAnnotatedValue::create(&http_request_function_param.to_value(), typ).map_err(
                |e| {
                    HttpHandlerBindingError::InternalError(format!(
                        "Failed converting request into wasm rpc: {:?}",
                        e
                    ))
                },
            )?;

        let resolved_request: GatewayResolvedWorkerRequest<Namespace> =
            GatewayResolvedWorkerRequest {
                component_id,
                worker_name: worker_detail.worker_name.clone(),
                function_name: virtual_exports::http_incoming_handler::FUNCTION_NAME.to_string(),
                function_params: vec![type_annotated_param],
                idempotency_key: worker_detail.idempotency_key.clone(),
                namespace: namespace.clone(),
            };

        let response = self
            .worker_request_executor
            .execute(resolved_request)
            .await
            .map_err(HttpHandlerBindingError::WorkerRequestExecutorError)?;

        let poem_response = {
            use golem_common::virtual_exports::http_incoming_handler as hic;

            let response_value: Value = response.result.try_into().map_err(|e| {
                HttpHandlerBindingError::InternalError(format!(
                    "Failed to parse response as wasm rpc value: {}",
                    e
                ))
            })?;

            let parsed_response = hic::HttpResponse::from_value(response_value).map_err(|e| {
                HttpHandlerBindingError::InternalError(format!("Failed parsing response: {}", e))
            })?;

            let converted_status_code =
                StatusCode::from_u16(parsed_response.status).map_err(|e| {
                    HttpHandlerBindingError::InternalError(format!(
                        "Failed to parse response status: {}",
                        e
                    ))
                })?;

            let mut builder = poem::Response::builder().status(converted_status_code);

            for (header_name, header_value) in parsed_response.headers.0 {
                let converted_header_value =
                    http::HeaderValue::from_bytes(&header_value).map_err(|e| {
                        HttpHandlerBindingError::InternalError(format!(
                            "Failed to parse response header: {}",
                            e
                        ))
                    })?;
                builder = builder.header(header_name, converted_header_value);
            }

            if let Some(body) = parsed_response.body {
                let converted_body = http_body_util::Full::new(body.content.0);

                let trailers = if let Some(trailers) = body.trailers {
                    let mut acc = http::HeaderMap::new();
                    for (header_name, header_value) in trailers.0.into_iter() {
                        let converted_header_name = http::HeaderName::from_str(&header_name)
                            .map_err(|e| {
                                HttpHandlerBindingError::InternalError(format!(
                                    "Failed to parse response trailer name: {}",
                                    e
                                ))
                            })?;
                        let converted_header_value = http::HeaderValue::from_bytes(&header_value)
                            .map_err(|e| {
                            HttpHandlerBindingError::InternalError(format!(
                                "Failed to parse response trailer value: {}",
                                e
                            ))
                        })?;

                        acc.insert(converted_header_name, converted_header_value);
                    }
                    Some(Ok(acc))
                } else {
                    None
                };

                let body_with_trailers = converted_body.with_trailers(async { trailers });

                let boxed: BoxBody<Bytes, std::io::Error> = BoxBody::new(
                    body_with_trailers.map_err(error_from_infallible::<std::io::Error>),
                );

                builder.body(boxed)
            } else {
                builder.body(poem::Body::empty())
            }
        };

        Ok(HttpHandlerBindingSuccess {
            response: poem_response,
        })
    }
}

fn error_from_infallible<E>(_infallible: Infallible) -> E {
    unreachable!()
}
