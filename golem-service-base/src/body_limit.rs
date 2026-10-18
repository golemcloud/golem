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

use golem_common::base_model::api;
use golem_common::base_model::error::ErrorBody;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use poem::http::{StatusCode, header};
use poem::web::headers::{ContentLength, HeaderMapExt};
use poem::{Body, Endpoint, IntoResponse, Middleware, Request, Response};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Answers a request whose body is larger than the limit with `413 Payload Too Large`.
///
/// A request that declares a larger `Content-Length` is answered before any of its body is read.
/// A request without `Content-Length` (chunked HTTP/1.1, or HTTP/2) has its body read up to the
/// limit: the first byte past the limit fails the read, and the response becomes 413 whatever
/// the endpoint answered.
#[derive(Clone, Copy, Debug)]
pub struct RequestBodyLimit {
    max_size: usize,
    max_multipart_size: usize,
}

impl RequestBodyLimit {
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            max_multipart_size: max_size,
        }
    }

    /// Sets a separate limit for `multipart/form-data` requests, which carry file uploads.
    pub fn with_multipart_limit(self, max_multipart_size: usize) -> Self {
        Self {
            max_multipart_size,
            ..self
        }
    }

    fn max_size_for(&self, request: &Request) -> usize {
        let is_multipart = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/form-data"));
        if is_multipart {
            self.max_multipart_size
        } else {
            self.max_size
        }
    }
}

impl<E: Endpoint> Middleware<E> for RequestBodyLimit {
    type Output = RequestBodyLimitEndpoint<E>;

    fn transform(&self, inner: E) -> Self::Output {
        RequestBodyLimitEndpoint {
            inner,
            limit: *self,
        }
    }
}

pub struct RequestBodyLimitEndpoint<E> {
    inner: E,
    limit: RequestBodyLimit,
}

impl<E: Endpoint> Endpoint for RequestBodyLimitEndpoint<E> {
    type Output = Response;

    async fn call(&self, mut request: Request) -> poem::Result<Response> {
        let max_size = self.limit.max_size_for(&request);
        if let Some(ContentLength(length)) = request.headers().typed_get::<ContentLength>()
            && length > max_size as u64
        {
            return Ok(payload_too_large(max_size));
        }

        let exceeded = Arc::new(AtomicBool::new(false));
        let body = request.take_body();
        if !body.is_empty() {
            request.set_body(limited(body, max_size, exceeded.clone()));
        }

        let result = self.inner.call(request).await;
        if exceeded.load(Ordering::Acquire) {
            return Ok(payload_too_large(max_size));
        }
        result.map(IntoResponse::into_response)
    }
}

fn limited(body: Body, max_size: usize, exceeded: Arc<AtomicBool>) -> Body {
    let body: http_body_util::combinators::BoxBody<_, std::io::Error> = body.into();
    Limited::new(body, max_size)
        .map_err(move |error| {
            if error.is::<LengthLimitError>() {
                exceeded.store(true, Ordering::Release);
            }
            std::io::Error::other(error)
        })
        .boxed()
        .into()
}

fn payload_too_large(max_size: usize) -> Response {
    let body = serde_json::json!(ErrorBody {
        code: api::error_code::REQUEST_PAYLOAD_TOO_LARGE.to_string(),
        error: format!("The request body exceeds the limit of {max_size} bytes"),
        cause: None,
    });
    Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .content_type("application/json")
        .body(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::RequestBodyLimit;
    use bytes::Bytes;
    use futures::stream;
    use poem::http::StatusCode;
    use poem::test::TestClient;
    use poem::{Body, EndpointExt, handler};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    #[handler]
    async fn read_body(body: Body) -> poem::Result<String> {
        Ok(body.into_bytes().await?.len().to_string())
    }

    fn chunked(size: usize) -> Body {
        let chunks = (0..size).step_by(1024).map(move |start| {
            Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 1024.min(size - start)]))
        });
        Body::from_bytes_stream(stream::iter(chunks.collect::<Vec<_>>()))
    }

    #[test]
    async fn declared_length_above_the_limit_is_rejected_before_the_endpoint_runs() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let endpoint = poem::endpoint::make(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            async { "called" }
        })
        .with(RequestBodyLimit::new(10));
        let client = TestClient::new(endpoint);

        let response = client
            .post("/")
            .header("content-length", "11")
            .body(vec![b'x'; 11])
            .send()
            .await;

        response.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
        response
            .assert_json(serde_json::json!({
                "code": "REQUEST_PAYLOAD_TOO_LARGE",
                "error": "The request body exceeds the limit of 10 bytes",
            }))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    async fn body_without_length_above_the_limit_is_rejected() {
        let client = TestClient::new(read_body.with(RequestBodyLimit::new(4096)));

        let response = client.post("/").body(chunked(4097)).send().await;

        response.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    async fn body_at_the_limit_reaches_the_endpoint() {
        let client = TestClient::new(read_body.with(RequestBodyLimit::new(4096)));

        let declared = client
            .post("/")
            .header("content-length", "4096")
            .body(vec![b'x'; 4096])
            .send()
            .await;
        declared.assert_status_is_ok();
        declared.assert_text("4096").await;

        let streamed = client.post("/").body(chunked(4096)).send().await;
        streamed.assert_status_is_ok();
        streamed.assert_text("4096").await;
    }

    #[test]
    async fn request_without_body_reaches_the_endpoint() {
        let client = TestClient::new(read_body.with(RequestBodyLimit::new(0)));

        let response = client.get("/").send().await;

        response.assert_status_is_ok();
        response.assert_text("0").await;
    }

    #[test]
    async fn multipart_requests_use_the_multipart_limit() {
        let limit = RequestBodyLimit::new(10).with_multipart_limit(100);
        let client = TestClient::new(read_body.with(limit));

        let upload = client
            .post("/")
            .content_type("multipart/form-data; boundary=x")
            .body(chunked(100))
            .send()
            .await;
        upload.assert_status_is_ok();

        let too_large_upload = client
            .post("/")
            .content_type("multipart/form-data; boundary=x")
            .body(chunked(101))
            .send()
            .await;
        too_large_upload.assert_status(StatusCode::PAYLOAD_TOO_LARGE);

        let json = client
            .post("/")
            .content_type("application/json")
            .body(chunked(11))
            .send()
            .await;
        json.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    }
}
