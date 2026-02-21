use super::{OidcSession, ParsedRequestBody};

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

use super::error::RequestHandlerError;
use anyhow::anyhow;
use golem_common::model::agent::{BinarySource, BinaryType};
use golem_common::model::invocation_context::{
    InvocationContextSpan, InvocationContextStack, TraceId,
};
use golem_common::model::{IdempotencyKey, invocation_context};
use golem_service_base::custom_api::RequestBodySchema;
use golem_service_base::headers::TraceContextHeaders;
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use http::HeaderMap;
use std::collections::HashMap;
use std::sync::OnceLock;
use uuid::Uuid;

const COOKIE_HEADER_NAMES: [&str; 2] = ["cookie", "Cookie"];

pub struct RichRequest {
    pub underlying: poem::Request,
    pub request_id: Uuid,
    pub authenticated_session: Option<OidcSession>,

    parsed_cookies: OnceLock<HashMap<String, String>>,
    parsed_query_params: OnceLock<HashMap<String, Vec<String>>>,
}

impl RichRequest {
    pub fn new(underlying: poem::Request) -> RichRequest {
        RichRequest {
            underlying,
            request_id: Uuid::new_v4(),
            authenticated_session: None,
            parsed_cookies: OnceLock::new(),
            parsed_query_params: OnceLock::new(),
        }
    }

    pub fn origin(&self) -> Result<Option<&str>, RequestHandlerError> {
        self.header_string_value("origin")
    }

    pub fn headers(&self) -> &HeaderMap {
        self.underlying.headers()
    }

    pub fn header_string_value(
        &self,
        header_name: &str,
    ) -> Result<Option<&str>, RequestHandlerError> {
        match self.headers().get(header_name) {
            Some(header) => {
                let result =
                    header
                        .to_str()
                        .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                            header_name: "Origin".to_string(),
                        })?;
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    pub fn query_params(&self) -> &HashMap<String, Vec<String>> {
        self.parsed_query_params.get_or_init(|| {
            let mut params: HashMap<String, Vec<String>> = HashMap::new();

            if let Some(q) = self.underlying.uri().query() {
                for (key, value) in url::form_urlencoded::parse(q.as_bytes()).into_owned() {
                    params.entry(key).or_default().push(value);
                }
            }

            params
        })
    }

    pub fn get_single_param(&self, name: &'static str) -> Result<&str, RequestHandlerError> {
        match self.query_params().get(name).map(|qp| qp.as_slice()) {
            Some([single]) => Ok(single),
            None | Some([]) => Err(RequestHandlerError::MissingValue { expected: name }),
            _ => Err(RequestHandlerError::TooManyValues { expected: name }),
        }
    }

    pub fn cookies(&self) -> &HashMap<String, String> {
        self.parsed_cookies.get_or_init(|| {
            let mut map = HashMap::new();
            for header_name in COOKIE_HEADER_NAMES.iter() {
                if let Some(value) = self.underlying.header(header_name) {
                    for part in value.split(';') {
                        let mut kv = part.splitn(2, '=');
                        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                            map.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                }
            }
            map
        })
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies().get(name).map(|s| s.as_str())
    }

    pub fn set_authenticated_session(&mut self, session: OidcSession) {
        self.authenticated_session = Some(session);
    }

    pub fn authenticated_session(&self) -> Option<&OidcSession> {
        self.authenticated_session.as_ref()
    }

    // Will consume the request body
    pub async fn parse_request_body(
        &mut self,
        expected: &RequestBodySchema,
    ) -> Result<ParsedRequestBody, RequestHandlerError> {
        match expected {
            RequestBodySchema::Unused => Ok(ParsedRequestBody::Unused),

            RequestBodySchema::JsonBody { expected_type } => {
                let json_body: serde_json::Value = self
                    .underlying
                    .take_body()
                    .into_json()
                    .await
                    .map_err(|err| RequestHandlerError::BodyIsNotValidJson {
                        error: err.to_string(),
                    })?;
                let parsed_body = ValueAndType::parse_with_type(&json_body, expected_type)
                    .map_err(|errors| RequestHandlerError::JsonBodyParsingFailed { errors })?;
                Ok(ParsedRequestBody::JsonBody(parsed_body.value))
            }

            RequestBodySchema::UnrestrictedBinary => self.parse_binary_body(None).await,
            RequestBodySchema::RestrictedBinary { allowed_mime_types } => {
                self.parse_binary_body(Some(allowed_mime_types)).await
            }
        }
    }

    async fn parse_binary_body(
        &mut self,
        allowed_mime_types: Option<&Vec<String>>,
    ) -> Result<ParsedRequestBody, RequestHandlerError> {
        let data = self
            .underlying
            .take_body()
            .into_vec()
            .await
            .map_err(|err| anyhow!("Failed reading raw body: {err}"))?;

        let header_name = http::header::CONTENT_TYPE.to_string();

        let mime_type = self
            .headers()
            .get(header_name.clone())
            .map(|value| value.to_str())
            .transpose()
            .map_err(|_| RequestHandlerError::HeaderIsNotAscii { header_name })?
            .map(|v| v.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        if let Some(allowed) = allowed_mime_types
            && !allowed.iter().any(|allowed| allowed == &mime_type)
        {
            return Err(RequestHandlerError::UnsupportedMimeType {
                mime_type,
                allowed_mime_types: allowed.clone(),
            });
        }

        Ok(ParsedRequestBody::UnstructuredBinary(Some(BinarySource {
            data,
            binary_type: BinaryType { mime_type },
        })))
    }

    pub fn idempotency_key(&self) -> IdempotencyKey {
        self.underlying
            .headers()
            .get("idempotency-key")
            .and_then(|h| h.to_str().ok())
            .map(|value| IdempotencyKey::new(value.to_string()))
            .unwrap_or_else(IdempotencyKey::fresh)
    }

    pub fn invocation_context(&self) -> InvocationContextStack {
        let trace_context_headers = TraceContextHeaders::parse(self.underlying.headers());
        let request_attributes = extract_request_attributes(&self.underlying);

        match trace_context_headers {
            Some(ctx) => {
                // Trace context found in headers, starting a new span
                let mut ctx = InvocationContextStack::new(
                    ctx.trace_id,
                    InvocationContextSpan::external_parent(ctx.parent_id),
                    ctx.trace_states,
                );
                ctx.push(
                    InvocationContextSpan::local()
                        .with_attributes(request_attributes)
                        .with_parent(ctx.spans.first().clone())
                        .build(),
                );
                ctx
            }
            None => {
                // No trace context in headers, starting a new trace
                InvocationContextStack::new(
                    TraceId::generate(),
                    InvocationContextSpan::local()
                        .with_attributes(request_attributes)
                        .build(),
                    Vec::new(),
                )
            }
        }
    }
}

fn extract_request_attributes(
    request: &poem::Request,
) -> HashMap<String, invocation_context::AttributeValue> {
    let mut result = HashMap::new();

    result.insert(
        "request.method".to_string(),
        invocation_context::AttributeValue::String(request.method().to_string()),
    );
    result.insert(
        "request.uri".to_string(),
        invocation_context::AttributeValue::String(request.uri().to_string()),
    );
    result.insert(
        "request.remote_addr".to_string(),
        invocation_context::AttributeValue::String(request.remote_addr().to_string()),
    );

    result
}

#[cfg(test)]
mod request_body_tests {
    use super::*;
    use assert2::{assert, let_assert};
    use golem_service_base::custom_api::RequestBodySchema;
    use golem_wasm::analysis::{NameTypePair, analysed_type};
    use http::Method;
    use poem::{Body, Request};
    use serde_json::json;
    use test_r::test;

    fn raw_request_with_content_type(
        bytes: &'static [u8],
        content_type: &'static str,
    ) -> RichRequest {
        let req = Request::builder()
            .method(Method::POST)
            .header(http::header::CONTENT_TYPE, content_type)
            .body(Body::from(bytes));
        RichRequest::new(req)
    }

    fn json_request(value: serde_json::Value) -> RichRequest {
        let req = Request::builder()
            .method(Method::POST)
            .body(Body::from_json(value).unwrap());
        RichRequest::new(req)
    }

    fn raw_request(bytes: &'static [u8]) -> RichRequest {
        let req = Request::builder()
            .method(Method::POST)
            .body(Body::from(bytes));
        RichRequest::new(req)
    }

    #[test]
    async fn unused_body_schema_does_not_consume_body() {
        let mut request = json_request(json!({ "x": 1 }));

        let result = request
            .parse_request_body(&RequestBodySchema::Unused)
            .await
            .unwrap();

        assert!(let ParsedRequestBody::Unused = result);
    }

    #[test]
    async fn valid_json_body_is_parsed() {
        let mut request = json_request(json!({
            "x": 1
        }));

        let schema = RequestBodySchema::JsonBody {
            expected_type: analysed_type::record(vec![NameTypePair {
                name: String::from("x"),
                typ: analysed_type::s32(),
            }]),
        };

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(ParsedRequestBody::JsonBody(golem_wasm::Value::Record(_)) = result);
    }

    #[test]
    async fn invalid_json_body_returns_error() {
        let mut request = raw_request(b"this is not json");

        let schema = RequestBodySchema::JsonBody {
            expected_type: analysed_type::u8(),
        };

        let err = request.parse_request_body(&schema).await.unwrap_err();

        assert!(let RequestHandlerError::BodyIsNotValidJson { .. } = err);
    }

    #[test]
    async fn json_body_schema_mismatch_returns_error() {
        // JSON is valid, but shape does not match expected type
        let mut request = json_request(json!("not a record"));

        let schema = RequestBodySchema::JsonBody {
            expected_type: analysed_type::record(vec![NameTypePair {
                name: String::from("x"),
                typ: analysed_type::s32(),
            }]),
        };

        let err = request.parse_request_body(&schema).await.unwrap_err();

        assert!(let RequestHandlerError::JsonBodyParsingFailed { .. } = err);
    }

    #[test]
    async fn restricted_binary_body_accepts_allowed_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/octet-stream");

        let schema = RequestBodySchema::RestrictedBinary {
            allowed_mime_types: vec!["application/octet-stream".to_string()],
        };

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(BinarySource { data, binary_type })) =
                result
        );

        assert!(data == b"binary-data");
        assert!(binary_type.mime_type == "application/octet-stream");
    }

    #[test]
    async fn restricted_binary_body_rejects_disallowed_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/json");

        let schema = RequestBodySchema::RestrictedBinary {
            allowed_mime_types: vec!["application/octet-stream".to_string()],
        };

        let err = request.parse_request_body(&schema).await.unwrap_err();

        {
            let_assert!(
                RequestHandlerError::UnsupportedMimeType {
                    mime_type,
                    allowed_mime_types,
                } = err
            );

            assert!(mime_type == "application/json");
            assert!(allowed_mime_types == vec!["application/octet-stream"]);
        }
    }

    #[test]
    async fn restricted_binary_body_without_content_type_uses_default_and_is_checked() {
        let mut request = raw_request(b"binary-data");

        let schema = RequestBodySchema::RestrictedBinary {
            allowed_mime_types: vec!["application/octet-stream".to_string()],
        };

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(BinarySource { binary_type, .. })) = result
        );

        assert!(binary_type.mime_type == "application/octet-stream");
    }

    #[test]
    async fn unrestricted_binary_body_accepts_any_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/weird");

        let schema = RequestBodySchema::UnrestrictedBinary;

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(BinarySource { binary_type, .. })) = result
        );

        assert!(binary_type.mime_type == "application/weird");
    }
}
