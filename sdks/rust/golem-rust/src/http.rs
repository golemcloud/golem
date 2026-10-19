// Copyright 2024-2026 Golem Cloud
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

//! Outgoing HTTP over WASI HTTP p3, using standard Rust HTTP request builders.
//!
//! Requests have buffered byte bodies; responses can be streamed or collected.
//! HTTP error statuses are responses, not transport errors. There are no automatic
//! redirects or retries. Sending requires the WASI p3 runtime used by Golem agents.

use crate::wasip3::http::{client, types};
use crate::wasip3::http_compat::{BodyWriter, http_from_wasi_response};
use bytes::Bytes;
use http_body::Body;
use http_body_util::{BodyExt, Full};
use std::future::IntoFuture;

pub use crate::wasip3::http_compat::{IncomingResponseBody, RequestOptionsExtension, Response};
pub use ::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, Uri, header};

/// Failures sending a request or reading and decoding its response body.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("WASI HTTP request or response body failed: {0}")]
    Wasi(#[from] types::ErrorCode),
    #[error("invalid request headers: {0}")]
    Headers(#[from] types::HeaderError),
    #[error("request body upload failed: {0}")]
    Upload(#[from] crate::wasip3::http_compat::Error),
    #[error("response body is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[cfg(feature = "json")]
    #[error("response body is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Send a request, checking both upload and request transmission completion.
///
/// Use an absolute HTTP(S) URI and `Vec::new()` for an empty request body.
/// Optional WASI timeouts can be supplied through a [`RequestOptionsExtension`].
/// Status codes such as 404 and 500 are returned normally. Inspect the status
/// and headers before passing the response to a consuming body helper.
pub async fn send(request: Request<Vec<u8>>) -> Result<Response, Error> {
    let (parts, body) = request.into_parts();
    let options = parts
        .extensions
        .get::<RequestOptionsExtension>()
        .cloned()
        .map(|options| options.0);
    let headers = types::Fields::try_from(parts.headers)?;
    let (writer, contents, trailers) = BodyWriter::new();
    let (request, transmitted) = types::Request::new(headers, Some(contents), trailers, options);

    request
        .set_method(&parts.method.into())
        .map_err(|()| types::ErrorCode::HttpRequestMethodInvalid)?;
    let scheme = parts.uri.scheme().map(Into::into);
    request
        .set_scheme(scheme.as_ref())
        .map_err(|()| types::ErrorCode::HttpProtocolError)?;
    request
        .set_authority(parts.uri.authority().map(|authority| authority.as_str()))
        .map_err(|()| types::ErrorCode::HttpRequestUriInvalid)?;
    request
        .set_path_with_query(parts.uri.path_and_query().map(|path| path.as_str()))
        .map_err(|()| types::ErrorCode::HttpRequestUriInvalid)?;

    let mut body = Full::new(Bytes::from(body));
    // Uploads must progress while the host sends the request. Unlike the generic
    // wasip3 conversion, keep and check both completion results.
    let (response, transmitted, uploaded) = futures::join!(
        client::send(request),
        transmitted.into_future(),
        writer.send_http_body(&mut body),
    );
    uploaded?;
    transmitted?;
    Ok(http_from_wasi_response(response?)?)
}

/// Collect the remaining body in memory, including checking its final result.
/// For large bodies, stream frames from `response.into_body()` instead.
pub async fn response_bytes(
    response: Response<impl Body<Data = Bytes, Error = types::ErrorCode>>,
) -> Result<Bytes, Error> {
    Ok(response.into_body().collect().await?.to_bytes())
}

/// Collect the remaining body in memory and decode it as strict UTF-8.
pub async fn response_text(
    response: Response<impl Body<Data = Bytes, Error = types::ErrorCode>>,
) -> Result<String, Error> {
    Ok(String::from_utf8(response_bytes(response).await?.to_vec())?)
}

/// Collect the remaining body in memory and decode JSON, regardless of status.
#[cfg(feature = "json")]
pub async fn response_json<T: serde::de::DeserializeOwned>(
    response: Response<impl Body<Data = Bytes, Error = types::ErrorCode>>,
) -> Result<T, Error> {
    Ok(serde_json::from_slice(&response_bytes(response).await?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use http_body::Frame;
    use http_body_util::StreamBody;
    use test_r::test;

    fn response(
        frames: Vec<Result<Frame<Bytes>, types::ErrorCode>>,
    ) -> Response<impl Body<Data = Bytes, Error = types::ErrorCode>> {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(StreamBody::new(stream::iter(frames)))
            .unwrap()
    }

    #[test]
    async fn collects_body_through_trailers_and_reports_late_errors() {
        let mut trailers = HeaderMap::new();
        trailers.insert("x-checksum", HeaderValue::from_static("ok"));
        let frames = vec![
            Ok(Frame::data(Bytes::from_static(b"abc"))),
            Ok(Frame::data(Bytes::from_static(b"defgh"))),
            Ok(Frame::trailers(trailers)),
        ];
        assert_eq!(response_bytes(response(frames)).await.unwrap(), "abcdefgh");

        let frames = vec![
            Ok(Frame::data(Bytes::from_static(b"partial"))),
            Err(types::ErrorCode::ConnectionTerminated),
        ];
        assert!(matches!(
            response_bytes(response(frames)).await,
            Err(Error::Wasi(types::ErrorCode::ConnectionTerminated))
        ));
    }

    #[test]
    async fn text_rejects_invalid_utf8() {
        assert!(matches!(
            response_text(response(vec![Ok(Frame::data(Bytes::from_static(b"\xff")))])).await,
            Err(Error::Utf8(_))
        ));
    }

    #[cfg(feature = "json")]
    #[test]
    async fn json_decodes_error_status_bodies_and_rejects_malformed_json() {
        let frames = vec![
            Ok(Frame::data(Bytes::from_static(b"{\"message\":"))),
            Ok(Frame::data(Bytes::from_static(b"\"bad input\"}"))),
        ];
        let value: serde_json::Value = response_json(response(frames)).await.unwrap();
        assert_eq!(value, serde_json::json!({ "message": "bad input" }));
        assert!(matches!(
            response_json::<serde_json::Value>(response(vec![Ok(Frame::data(
                Bytes::from_static(b"not json")
            ))]))
            .await,
            Err(Error::Json(_))
        ));
    }
}
