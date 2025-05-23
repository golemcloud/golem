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

use crate::error::GolemError;
use bytes::Bytes;
use golem_common::virtual_exports::http_incoming_handler::*;
use golem_common::widen_infallible;
use golem_wasm_rpc::Value;
use http::{HeaderName, HeaderValue};
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use wasmtime_wasi_http::bindings::http::types::ErrorCode;

pub type SchemeAndRequest = (
    wasmtime_wasi_http::bindings::wasi::http::types::Scheme,
    hyper::Request<BoxBody<Bytes, hyper::Error>>,
);

pub fn input_to_hyper_request(inputs: &[Value]) -> Result<SchemeAndRequest, GolemError> {
    let request = IncomingHttpRequest::from_function_input(inputs).map_err(|e| {
        GolemError::invalid_request(format!("Failed contructing incoming request: {e}"))
    })?;

    let wasmtime_scheme = match request.scheme {
        HttpScheme::HTTP => wasmtime_wasi_http::bindings::wasi::http::types::Scheme::Http,
        HttpScheme::HTTPS => wasmtime_wasi_http::bindings::wasi::http::types::Scheme::Https,
        HttpScheme::Custom(ref custom) => {
            wasmtime_wasi_http::bindings::wasi::http::types::Scheme::Other(custom.clone())
        }
    };

    let converted_scheme = match request.scheme {
        HttpScheme::HTTP => http::uri::Scheme::HTTP,
        HttpScheme::HTTPS => http::uri::Scheme::HTTPS,
        HttpScheme::Custom(custom) => custom.as_str().try_into().map_err(|e| {
            GolemError::invalid_request(format!("Not a valid scheme: {custom} ({e})"))
        })?,
    };

    let uri = http::Uri::builder()
        .scheme(converted_scheme)
        .authority(request.authority)
        .path_and_query(request.path_and_query)
        .build()
        .map_err(|e| {
            GolemError::invalid_request(format!("Failed to construct a valid url: {e}"))
        })?;

    let mut builder = hyper::Request::builder().uri(uri).method(request.method);

    for (name, value) in request.headers.0 {
        let converted = http::HeaderValue::from_bytes(&value)
            .map_err(|e| GolemError::invalid_request(format!("Invalid header value: {e}")))?;

        builder = builder.header(name, converted);
    }

    let body = if let Some(b) = request.body {
        tracing::debug!("adding request body to wasi:http/incoming-request");

        let body = http_body_util::Full::new(b.content.0);

        let converted_trailers = if let Some(trailers) = b.trailers {
            let mut converted_trailers = http::HeaderMap::new();
            for (name, value) in trailers.0.into_iter() {
                let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                    GolemError::invalid_request(format!("Failed to convert header name {e}"))
                })?;
                let header_value = HeaderValue::from_bytes(&value).map_err(|e| {
                    GolemError::invalid_request(format!("Failed to convert header value {e}"))
                })?;

                converted_trailers.insert(header_name, header_value);
            }
            Some(Ok(converted_trailers))
        } else {
            None
        };

        let with_trailers = body.with_trailers(async { converted_trailers });
        BoxBody::new(with_trailers.map_err(widen_infallible))
    } else {
        BoxBody::new(http_body_util::Empty::new().map_err(widen_infallible))
    };

    let hyper_request = builder
        .body(body)
        .map_err(|e| GolemError::invalid_request(format!("Failed to attach body {e}")))?;

    Ok((wasmtime_scheme, hyper_request))
}

pub async fn http_response_to_output(
    response: http::Response<BoxBody<Bytes, ErrorCode>>,
) -> Result<Value, GolemError> {
    use http_body_util::BodyExt;

    tracing::debug!("Converting wasi:http/incoming-handler response to golem compatible value");

    let status = response.status().as_u16();

    let converted_headers = {
        let mut acc: Vec<(String, Bytes)> = Vec::new();
        for (name, value) in response.headers().into_iter() {
            acc.push((name.to_string(), Bytes::copy_from_slice(value.as_bytes())));
        }
        HttpFields(acc)
    };

    let collected = response.into_body().collect().await.map_err(|e| {
        GolemError::runtime(format!("Failed collection body of http response: {e}"))
    })?;

    let trailers = collected.trailers().cloned();
    let bytes = collected.to_bytes();

    let converted_body = if !bytes.is_empty() || trailers.is_some() {
        let converted_trailers = if let Some(hm) = trailers {
            let mut result = Vec::new();
            let mut previous = None;
            for (name, value) in hm.into_iter() {
                let current = match name {
                    None => previous.clone().unwrap(),
                    Some(next) => {
                        previous = Some(next.clone());
                        next
                    }
                };
                result.push((
                    current.to_string(),
                    Bytes::copy_from_slice(value.as_bytes()),
                ))
            }
            Some(HttpFields(result))
        } else {
            None
        };

        Some(HttpBodyAndTrailers {
            content: HttpBodyContent(bytes),
            trailers: converted_trailers,
        })
    } else {
        None
    };

    let response = HttpResponse {
        status,
        headers: converted_headers,
        body: converted_body,
    };

    Ok(response.to_value())
}
