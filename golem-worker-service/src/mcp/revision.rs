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

//! The MCP protocol revisions that the endpoint serves, and the rule that selects the revision
//! of a request.

use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Middleware, Request, Response};
use rmcp::model::ProtocolVersion;
use serde::Deserialize;
use std::borrow::Cow;

/// The MCP revisions that the endpoint serves, oldest first.
pub(crate) const SERVED_REVISIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

/// The revision of a request that is not `initialize`: the value of its `MCP-Protocol-Version`
/// header, or 2025-03-26 when the request has no header. `Err` holds the text of the `400`
/// answer for a value that is not UTF-8 or that the endpoint does not serve.
fn request_revision(header: Option<&[u8]>) -> Result<ProtocolVersion, Box<str>> {
    header.map_or(Ok(ProtocolVersion::V_2025_03_26), |value| {
        let value = std::str::from_utf8(value).map_err(|_| {
            Box::<str>::from("Bad Request: Invalid MCP-Protocol-Version header encoding")
        })?;
        SERVED_REVISIONS
            .iter()
            .find(|revision| revision.as_str() == value)
            .cloned()
            .ok_or_else(|| format!("Bad Request: Unsupported MCP-Protocol-Version: {value}").into())
    })
}

/// Whether the endpoint serves a request. An `initialize` is always served, because its body
/// negotiates the revision. Every other request is served when [`request_revision`] accepts its
/// `MCP-Protocol-Version` header.
fn admit(initialize: bool, header: Option<&[u8]>) -> Result<(), Box<str>> {
    if initialize {
        Ok(())
    } else {
        request_revision(header).map(drop)
    }
}

/// Whether the endpoint serves a `POST` with the JSON-RPC `body` and the `MCP-Protocol-Version`
/// `header`, as [`admit`] says. A body that is not a JSON-RPC message is served, so that rmcp
/// answers it.
fn admit_post(body: &[u8], header: Option<&[u8]>) -> Result<(), Box<str>> {
    let is_object = body
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'{');
    is_object
        .then(|| serde_json::from_slice::<JsonRpcMethod>(body).ok())
        .flatten()
        .map_or(Ok(()), |message| {
            admit(message.method.as_deref() == Some("initialize"), header)
        })
}

/// The method of a JSON-RPC message object. A response has no method.
#[derive(Deserialize)]
struct JsonRpcMethod<'a> {
    #[serde(borrow, default)]
    method: Option<Cow<'a, str>>,
}

/// Answers `400 Bad Request` to a `POST` that is not `initialize` and whose
/// `MCP-Protocol-Version` header names a revision that the endpoint does not serve. Every other
/// request goes on unchanged, so `GET` and `DELETE` always reach rmcp.
///
/// The middleware reads the whole body of a `POST`, so it must sit inside the request body limit.
pub(crate) struct ServedRevisions;

impl<E: Endpoint> Middleware<E> for ServedRevisions {
    type Output = ServedRevisionsEndpoint<E>;

    fn transform(&self, inner: E) -> Self::Output {
        ServedRevisionsEndpoint(inner)
    }
}

pub(crate) struct ServedRevisionsEndpoint<E>(E);

impl<E: Endpoint> Endpoint for ServedRevisionsEndpoint<E> {
    type Output = Response;

    async fn call(&self, mut request: Request) -> poem::Result<Response> {
        if request.method() == Method::POST {
            let body = request.take_body().into_bytes().await?;
            let header = request
                .headers()
                .get("MCP-Protocol-Version")
                .map(|value| value.as_bytes());
            if let Err(refusal) = admit_post(&body, header) {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(refusal.into_string()));
            }
            request.set_body(body);
        }
        self.0.call(request).await.map(IntoResponse::into_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn a_request_without_a_header_is_2025_03_26() {
        assert_eq!(request_revision(None), Ok(ProtocolVersion::V_2025_03_26));
    }

    #[test]
    fn each_served_revision_is_accepted() {
        SERVED_REVISIONS.iter().for_each(|revision| {
            assert_eq!(
                request_revision(Some(revision.as_str().as_bytes())).as_ref(),
                Ok(revision),
            );
        });
    }

    #[test]
    fn other_revisions_and_malformed_values_are_refused() {
        [
            &b"2024-11-05"[..],
            b"1999-01-01",
            b"2026-07-29",
            b"",
            b"\xff\xfe",
        ]
        .into_iter()
        .for_each(|header| {
            assert!(
                request_revision(Some(header)).is_err(),
                "{:?}",
                String::from_utf8_lossy(header)
            );
            assert!(admit(false, Some(header)).is_err());
        });
    }

    #[test]
    fn the_refusal_names_the_value() {
        assert_eq!(
            request_revision(Some(b"2024-11-05")),
            Err("Bad Request: Unsupported MCP-Protocol-Version: 2024-11-05".into())
        );
    }

    #[test]
    fn initialize_is_admitted_with_any_header() {
        [
            None,
            Some(&b"2024-11-05"[..]),
            Some(b"\xff"),
            Some(b"2025-06-18"),
        ]
        .into_iter()
        .for_each(|header| assert_eq!(admit(true, header), Ok(())));
    }

    #[test]
    fn a_post_is_refused_only_when_it_is_not_initialize_and_names_another_revision() {
        let old = Some(&b"2024-11-05"[..]);
        let initialize = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let list = br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let notification = br#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#;
        assert_eq!(admit_post(initialize, old), Ok(()));
        assert!(admit_post(list, old).is_err());
        assert!(admit_post(notification, old).is_err());
        assert_eq!(admit_post(list, Some(b"2025-06-18")), Ok(()));
        assert_eq!(admit_post(list, None), Ok(()));
        assert_eq!(admit_post(b"not json", old), Ok(()));
        assert_eq!(admit_post(b"[]", old), Ok(()));
    }

    #[test]
    fn other_requests_are_admitted_with_a_served_revision_or_no_header() {
        assert_eq!(admit(false, None), Ok(()));
        assert_eq!(admit(false, Some(b"2026-07-28")), Ok(()));
    }
}
