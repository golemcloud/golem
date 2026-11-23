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

//! The MCP protocol revisions that the endpoint serves, the rule that refuses a request that
//! names another revision, and the rule that gives a request with no `MCP-Protocol-Version`
//! header the revision 2025-03-26.

use poem::http::{HeaderValue, Method, StatusCode};
use poem::{Endpoint, IntoResponse, Middleware, Request, Response};
use rmcp::model::ProtocolVersion;
use rmcp::transport::common::http_header::HEADER_MCP_PROTOCOL_VERSION;
use serde::Deserialize;
use std::borrow::Cow;

/// The MCP revisions that the endpoint serves, oldest first.
pub(crate) const SERVED_REVISIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

/// Checks the `MCP-Protocol-Version` header of a request that is not `initialize`. A request
/// without the header passes. A served revision passes. `Err` holds the text of the `400` answer
/// for a value that is not UTF-8 or that the endpoint does not serve.
fn check_header(header: Option<&[u8]>) -> Result<(), Box<str>> {
    header.map_or(Ok(()), |value| {
        let value = std::str::from_utf8(value).map_err(|_| {
            Box::<str>::from("Bad Request: Invalid MCP-Protocol-Version header encoding")
        })?;
        SERVED_REVISIONS
            .iter()
            .any(|revision| revision.as_str() == value)
            .then_some(())
            .ok_or_else(|| format!("Bad Request: Unsupported MCP-Protocol-Version: {value}").into())
    })
}

/// Checks a `POST` with the JSON-RPC `body` and the `MCP-Protocol-Version` `header`. A body that
/// is not a JSON object, or that does not parse as a JSON-RPC message, passes with no change, so
/// that rmcp answers it. An `initialize` passes with any header and with no change, because its
/// body negotiates the revision. Every other JSON-RPC message passes when [`check_header`]
/// accepts the header.
///
/// `Ok` holds the `MCP-Protocol-Version` header to add before the request goes to rmcp. A body
/// that is a JSON object with no `method`, a `null` `method`, or a string `method` other than
/// `initialize`, and that has no header, is 2025-03-26. Thus it gets the header `2025-03-26`.
/// rmcp then serves a request whose `_meta` names 2025-03-26, and answers `400` to a request
/// whose `_meta` names another revision. Each other request gets no header.
fn admit_post(body: &[u8], header: Option<&[u8]>) -> Result<Option<HeaderValue>, Box<str>> {
    let is_object = body
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'{');
    is_object
        .then(|| serde_json::from_slice::<JsonRpcMethod>(body).ok())
        .flatten()
        .filter(|message| message.method.as_deref() != Some("initialize"))
        .map_or(Ok(None), |_| {
            check_header(header).map(|()| {
                header
                    .is_none()
                    .then(|| HeaderValue::from_static(ProtocolVersion::V_2025_03_26.as_str()))
            })
        })
}

/// The method of a JSON-RPC message object. A response has no method.
#[derive(Deserialize)]
struct JsonRpcMethod<'a> {
    #[serde(borrow, default)]
    method: Option<Cow<'a, str>>,
}

/// Answers `400 Bad Request` to a `POST` when both of these are true:
///
/// - The body is a JSON object with no `method`, a `null` `method`, or a string `method` other
///   than `initialize`. Each JSON-RPC request, notification and response other than `initialize`
///   has this form.
/// - The `MCP-Protocol-Version` header has a value that is not UTF-8, or that names a revision
///   that the endpoint does not serve.
///
/// A `POST` with a body that is a JSON object with no `method`, a `null` `method`, or a string
/// `method` other than `initialize`, and with no `MCP-Protocol-Version` header, goes to rmcp with
/// the added header `MCP-Protocol-Version: 2025-03-26`.
///
/// Every other request goes on unchanged to rmcp. This includes each `GET` and `DELETE`, each
/// `initialize`, and each `POST` with a body that is not a JSON object, or that does not parse as
/// a JSON-RPC message.
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
                .get(HEADER_MCP_PROTOCOL_VERSION)
                .map(|value| value.as_bytes());
            let added = match admit_post(&body, header) {
                Ok(added) => added,
                Err(refusal) => {
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(refusal.into_string()));
                }
            };
            if let Some(value) = added {
                request
                    .headers_mut()
                    .insert(HEADER_MCP_PROTOCOL_VERSION, value);
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

    const INITIALIZE: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    const LIST: &[u8] = br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

    #[test]
    fn each_served_revision_is_accepted() {
        SERVED_REVISIONS.iter().for_each(|revision| {
            assert_eq!(check_header(Some(revision.as_str().as_bytes())), Ok(()));
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
                check_header(Some(header)).is_err(),
                "{:?}",
                String::from_utf8_lossy(header)
            );
            assert!(admit_post(LIST, Some(header)).is_err());
        });
    }

    #[test]
    fn the_refusal_names_the_value() {
        assert_eq!(
            check_header(Some(b"2024-11-05")),
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
        .for_each(|header| assert_eq!(admit_post(INITIALIZE, header), Ok(None)));
    }

    #[test]
    fn a_post_is_refused_only_when_it_is_not_initialize_and_names_another_revision() {
        let old = Some(&b"2024-11-05"[..]);
        let notification = br#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#;
        assert_eq!(admit_post(INITIALIZE, old), Ok(None));
        assert!(admit_post(LIST, old).is_err());
        assert!(admit_post(notification, old).is_err());
        assert!(admit_post(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#, old).is_err());
        assert_eq!(admit_post(LIST, Some(b"2025-06-18")), Ok(None));
        assert_eq!(admit_post(LIST, None), Ok(Some(header_2025_03_26())));
        assert_eq!(admit_post(b"not json", old), Ok(None));
        assert_eq!(admit_post(br#"{"jsonrpc":"#, old), Ok(None));
        assert_eq!(admit_post(b"[]", old), Ok(None));
    }

    #[test]
    fn other_requests_are_admitted_with_a_served_revision_or_no_header() {
        assert_eq!(check_header(None), Ok(()));
        assert_eq!(admit_post(LIST, None), Ok(Some(header_2025_03_26())));
        assert_eq!(admit_post(LIST, Some(b"2026-07-28")), Ok(None));
    }

    fn header_2025_03_26() -> HeaderValue {
        HeaderValue::from_static("2025-03-26")
    }

    #[test]
    fn a_message_without_a_header_gets_the_2025_03_26_header_unless_it_is_initialize() {
        let notification = br#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#;
        let response = br#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert_eq!(admit_post(LIST, None), Ok(Some(header_2025_03_26())));
        assert_eq!(
            admit_post(notification, None),
            Ok(Some(header_2025_03_26()))
        );
        assert_eq!(admit_post(response, None), Ok(Some(header_2025_03_26())));
        assert_eq!(admit_post(INITIALIZE, None), Ok(None));
        SERVED_REVISIONS.iter().for_each(|revision| {
            assert_eq!(
                admit_post(LIST, Some(revision.as_str().as_bytes())),
                Ok(None)
            );
        });
        [&b"not json"[..], b"[]", b"\"tools/list\"", b""]
            .into_iter()
            .for_each(|body| assert_eq!(admit_post(body, None), Ok(None)));
    }
}
