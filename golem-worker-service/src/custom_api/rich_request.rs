use super::{OidcSession, ParsedRequestBody, RawBinaryBody};

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

use super::error::RequestHandlerError;
use anyhow::anyhow;
use golem_common::model::agent::{TextSource, TextType};
use golem_common::model::invocation_context::{
    InvocationContextSpan, InvocationContextStack, TraceId,
};
use golem_common::model::{IdempotencyKey, invocation_context};
use golem_common::schema::SchemaGraph;
use golem_common::schema::render::from_json_value;
use golem_common::schema::unstructured::{binary_body_restrictions, text_body_restrictions};
use golem_service_base::custom_api::RequestBodySchema;
use golem_service_base::headers::TraceContextHeaders;
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

            RequestBodySchema::JsonBody { expected } => {
                let json_body: serde_json::Value = self
                    .underlying
                    .take_body()
                    .into_json()
                    .await
                    .map_err(|err| RequestHandlerError::BodyIsNotValidJson {
                        error: err.to_string(),
                    })?;
                let parsed_body =
                    from_json_value(&expected.graph, &expected.graph.root, &json_body).map_err(
                        |err| RequestHandlerError::JsonBodyParsingFailed {
                            errors: vec![err.to_string()],
                        },
                    )?;
                Ok(ParsedRequestBody::JsonBody(parsed_body))
            }

            RequestBodySchema::BinaryBody { expected } => {
                let mime_types = binary_mime_types(&expected.graph)?;
                self.parse_binary_body(mime_types.as_ref()).await
            }

            RequestBodySchema::TextBody { expected } => {
                let languages = text_languages(&expected.graph)?;
                self.parse_text_body(languages.as_ref()).await
            }
        }
    }

    async fn parse_text_body(
        &mut self,
        allowed_language_codes: Option<&Vec<String>>,
    ) -> Result<ParsedRequestBody, RequestHandlerError> {
        // 1. Validate Content-Type
        let content_type_header_name = http::header::CONTENT_TYPE.to_string();
        let content_type = self
            .headers()
            .get(content_type_header_name.clone())
            .map(|value| value.to_str())
            .transpose()
            .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                header_name: content_type_header_name,
            })?;

        if let Some(ct) = content_type {
            validate_text_content_type(ct)?;
        }

        // 2. Read raw body and decode UTF-8
        let data = self
            .underlying
            .take_body()
            .into_vec()
            .await
            .map_err(|err| anyhow!("Failed reading raw body: {err}"))?;

        let text =
            String::from_utf8(data).map_err(|err| RequestHandlerError::BodyIsNotValidUtf8 {
                error: err.to_string(),
            })?;

        // 3. Validate Content-Language (must be a single value, no commas)
        let cl_header_name = http::header::CONTENT_LANGUAGE.to_string();
        let all_cl_values: Vec<&str> = self
            .headers()
            .get_all(http::header::CONTENT_LANGUAGE)
            .iter()
            .map(|h| {
                h.to_str()
                    .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                        header_name: cl_header_name.clone(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if all_cl_values.len() > 1 {
            return Err(RequestHandlerError::MultiValuedContentLanguageHeader);
        }

        let language_code: Option<String> = match all_cl_values.first() {
            None => None,
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    None
                } else if trimmed.contains(',') {
                    return Err(RequestHandlerError::MultiValuedContentLanguageHeader);
                } else {
                    Some(trimmed.to_string())
                }
            }
        };

        // 4. Validate against allowed_language_codes if specified
        if let Some(allowed) = allowed_language_codes
            && !allowed.is_empty()
            && let Some(lc) = &language_code
            && !allowed
                .iter()
                .any(|allowed_lc| allowed_lc.eq_ignore_ascii_case(lc))
        {
            return Err(RequestHandlerError::UnsupportedLanguage {
                language_code: lc.clone(),
                allowed_language_codes: allowed.clone(),
            });
        }

        let text_source = TextSource {
            data: text,
            text_type: language_code.map(|language_code| TextType { language_code }),
        };

        Ok(ParsedRequestBody::UnstructuredText(Some(text_source)))
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

        // A missing `Content-Type` header carries no MIME type; preserve that
        // absence rather than defaulting to `application/octet-stream`.
        let mime_type: Option<String> = self
            .headers()
            .get(header_name.clone())
            .map(|value| value.to_str())
            .transpose()
            .map_err(|_| RequestHandlerError::HeaderIsNotAscii { header_name })?
            .map(|v| v.to_string());

        // Lenient MIME handling (mirrors `parse_text_body` / the schema
        // semantics): a missing MIME type is always allowed; only a *present*
        // MIME type outside a *non-empty* allow-list is rejected.
        if let Some(allowed) = allowed_mime_types
            && !allowed.is_empty()
            && let Some(mime) = &mime_type
            && !allowed.iter().any(|allowed| allowed == mime)
        {
            return Err(RequestHandlerError::UnsupportedMimeType {
                mime_type: mime.clone(),
                allowed_mime_types: allowed.clone(),
            });
        }

        Ok(ParsedRequestBody::UnstructuredBinary(Some(RawBinaryBody {
            data,
            mime_type,
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

/// Validate Content-Type for text bodies.
///
/// Accept only `text/plain` (with no parameters) or `text/plain; charset=utf-8` (charset case-insensitive).
fn validate_text_content_type(content_type: &str) -> Result<(), RequestHandlerError> {
    let mut parts = content_type.split(';');
    let media_type = parts.next().unwrap_or("").trim();

    if !media_type.eq_ignore_ascii_case("text/plain") {
        return Err(RequestHandlerError::UnsupportedTextContentType {
            content_type: content_type.to_string(),
        });
    }

    for param in parts {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }

        let mut kv = param.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim();
        let value = kv.next().unwrap_or("").trim();
        // strip optional quotes
        let value = value.trim_matches('"');

        if key.eq_ignore_ascii_case("charset") {
            if !value.eq_ignore_ascii_case("utf-8") {
                return Err(RequestHandlerError::UnsupportedTextContentType {
                    content_type: content_type.to_string(),
                });
            }
        } else {
            // unknown parameter -> reject
            return Err(RequestHandlerError::UnsupportedTextContentType {
                content_type: content_type.to_string(),
            });
        }
    }

    Ok(())
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

/// Resolve a `BinaryBody` compiled-schema graph root to its allowed MIME types
/// (`None` = unrestricted). The producer guarantees the root is either a
/// canonical unstructured-binary `variant { inline, url }` wrapper or a bare
/// [`SchemaType::Binary`] rich scalar; both carry the MIME restrictions on their
/// (inline) binary type.
fn binary_mime_types(graph: &SchemaGraph) -> Result<Option<Vec<String>>, RequestHandlerError> {
    let restrictions = binary_body_restrictions(graph, &graph.root)
        .map_err(|err| anyhow!("Invalid binary body schema: {err}"))?;
    Ok(restrictions.mime_types.clone())
}

/// Resolve a `TextBody` compiled-schema graph root to its allowed language codes
/// (`None` = unrestricted). The producer guarantees the root is either a
/// canonical unstructured-text `variant { inline, url }` wrapper or a bare
/// [`SchemaType::Text`] rich scalar; both carry the language restrictions on
/// their (inline) text type.
fn text_languages(graph: &SchemaGraph) -> Result<Option<Vec<String>>, RequestHandlerError> {
    let restrictions = text_body_restrictions(graph, &graph.root)
        .map_err(|err| anyhow!("Invalid text body schema: {err}"))?;
    Ok(restrictions.languages.clone())
}

#[cfg(test)]
mod request_body_tests {
    use super::*;
    use assert2::{assert, let_assert};
    use golem_common::schema::SchemaValue;
    use golem_common::schema::schema_type::{
        BinaryRestrictions, NamedFieldType, SchemaType, TextRestrictions,
    };
    use golem_service_base::custom_api::{CompiledSchema, RequestBodySchema};
    use http::Method;
    use poem::{Body, Request};
    use serde_json::json;
    use test_r::test;

    fn json_body(root: SchemaType) -> RequestBodySchema {
        RequestBodySchema::JsonBody {
            expected: CompiledSchema {
                graph: SchemaGraph::anonymous(root),
            },
        }
    }

    fn unrestricted_binary() -> RequestBodySchema {
        RequestBodySchema::BinaryBody {
            expected: CompiledSchema {
                graph: SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions::default())),
            },
        }
    }

    fn restricted_binary(mime_types: Vec<String>) -> RequestBodySchema {
        RequestBodySchema::BinaryBody {
            expected: CompiledSchema {
                graph: SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions {
                    mime_types: Some(mime_types),
                    ..Default::default()
                })),
            },
        }
    }

    fn unrestricted_text() -> RequestBodySchema {
        RequestBodySchema::TextBody {
            expected: CompiledSchema {
                graph: SchemaGraph::anonymous(SchemaType::text(TextRestrictions::default())),
            },
        }
    }

    fn restricted_text(languages: Vec<String>) -> RequestBodySchema {
        RequestBodySchema::TextBody {
            expected: CompiledSchema {
                graph: SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
                    languages: Some(languages),
                    ..Default::default()
                })),
            },
        }
    }

    fn record_field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: Default::default(),
        }
    }

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

        let schema = json_body(SchemaType::record(vec![record_field(
            "x",
            SchemaType::s32(),
        )]));

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(ParsedRequestBody::JsonBody(SchemaValue::Record { .. }) = result);
    }

    #[test]
    async fn invalid_json_body_returns_error() {
        let mut request = raw_request(b"this is not json");

        let schema = json_body(SchemaType::u8());

        let err = request.parse_request_body(&schema).await.unwrap_err();

        assert!(let RequestHandlerError::BodyIsNotValidJson { .. } = err);
    }

    #[test]
    async fn json_body_schema_mismatch_returns_error() {
        // JSON is valid, but shape does not match expected type
        let mut request = json_request(json!("not a record"));

        let schema = json_body(SchemaType::record(vec![record_field(
            "x",
            SchemaType::s32(),
        )]));

        let err = request.parse_request_body(&schema).await.unwrap_err();

        assert!(let RequestHandlerError::JsonBodyParsingFailed { .. } = err);
    }

    #[test]
    async fn restricted_binary_body_accepts_allowed_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/octet-stream");

        let schema = restricted_binary(vec!["application/octet-stream".to_string()]);

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(RawBinaryBody { data, mime_type })) = result
        );

        assert!(data == b"binary-data");
        assert!(mime_type.as_deref() == Some("application/octet-stream"));
    }

    #[test]
    async fn restricted_binary_body_rejects_disallowed_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/json");

        let schema = restricted_binary(vec!["application/octet-stream".to_string()]);

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
    async fn restricted_binary_body_without_content_type_is_accepted_with_no_mime() {
        // Lenient MIME handling: a missing `Content-Type` is always allowed,
        // even under a restrictive schema, and its absence is preserved (not
        // defaulted to `application/octet-stream`).
        let mut request = raw_request(b"binary-data");

        let schema = restricted_binary(vec!["application/octet-stream".to_string()]);

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(RawBinaryBody { data, mime_type })) = result
        );

        assert!(data == b"binary-data");
        assert!(mime_type.is_none());
    }

    #[test]
    async fn binary_body_with_empty_allow_list_accepts_any_mime_type() {
        // An empty MIME allow-list (`Some(vec![])`) is treated as unrestricted,
        // consistent with the schema/MCP semantics.
        let mut request = raw_request_with_content_type(b"binary-data", "application/weird");

        let schema = restricted_binary(vec![]);

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(RawBinaryBody { mime_type, .. })) = result
        );

        assert!(mime_type.as_deref() == Some("application/weird"));
    }

    #[test]
    async fn unrestricted_binary_body_accepts_any_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/weird");

        let schema = unrestricted_binary();

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(RawBinaryBody { mime_type, .. })) = result
        );

        assert!(mime_type.as_deref() == Some("application/weird"));
    }

    fn text_request(
        bytes: &'static [u8],
        content_type: Option<&'static str>,
        content_languages: &[&'static str],
    ) -> RichRequest {
        let mut builder = Request::builder().method(Method::POST);
        if let Some(ct) = content_type {
            builder = builder.header(http::header::CONTENT_TYPE, ct);
        }
        for cl in content_languages {
            builder = builder.header(http::header::CONTENT_LANGUAGE, *cl);
        }
        let req = builder.body(Body::from(bytes));
        RichRequest::new(req)
    }

    #[test]
    async fn unrestricted_text_body_without_content_language_is_parsed() {
        let mut request = text_request(b"hello world", Some("text/plain"), &[]);

        let result = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        assert!(text_source.data == "hello world");
        assert!(text_source.text_type.is_none());
    }

    #[test]
    async fn unrestricted_text_body_without_content_type_is_accepted() {
        let mut request = text_request(b"hello world", None, &[]);

        let result = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        assert!(text_source.data == "hello world");
        assert!(text_source.text_type.is_none());
    }

    #[test]
    async fn text_body_accepts_text_plain_with_utf8_charset() {
        let mut request = text_request(b"hello", Some("text/plain; charset=utf-8"), &[]);

        let result = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        assert!(text_source.data == "hello");
    }

    #[test]
    async fn text_body_accepts_text_plain_with_utf8_charset_case_insensitive() {
        let mut request = text_request(b"hello", Some("Text/Plain; Charset=UTF-8"), &[]);

        let result = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(_)) = result);
    }

    #[test]
    async fn text_body_rejects_non_text_content_type() {
        let mut request = text_request(b"hello", Some("application/json"), &[]);

        let err = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap_err();

        let_assert!(RequestHandlerError::UnsupportedTextContentType { content_type } = err);
        assert!(content_type == "application/json");
    }

    #[test]
    async fn text_body_rejects_non_utf8_charset() {
        let mut request = text_request(b"hello", Some("text/plain; charset=iso-8859-1"), &[]);

        let err = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap_err();

        assert!(let RequestHandlerError::UnsupportedTextContentType { .. } = err);
    }

    #[test]
    async fn text_body_rejects_invalid_utf8() {
        // Invalid UTF-8 byte sequence
        let mut request = text_request(b"\xff\xfe\xfd", Some("text/plain"), &[]);

        let err = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap_err();

        assert!(let RequestHandlerError::BodyIsNotValidUtf8 { .. } = err);
    }

    #[test]
    async fn text_body_with_content_language_is_captured() {
        let mut request = text_request(b"bonjour", Some("text/plain"), &["fr"]);

        let result = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        let text_type = text_source.text_type.unwrap();
        assert!(text_type.language_code == "fr");
    }

    #[test]
    async fn text_body_rejects_multi_valued_content_language_header() {
        let mut request = text_request(b"hello", Some("text/plain"), &["en", "fr"]);

        let err = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap_err();

        assert!(let RequestHandlerError::MultiValuedContentLanguageHeader = err);
    }

    #[test]
    async fn text_body_rejects_comma_separated_content_language_value() {
        let mut request = text_request(b"hello", Some("text/plain"), &["en, fr"]);

        let err = request
            .parse_request_body(&unrestricted_text())
            .await
            .unwrap_err();

        assert!(let RequestHandlerError::MultiValuedContentLanguageHeader = err);
    }

    #[test]
    async fn restricted_text_body_accepts_allowed_language() {
        let mut request = text_request(b"hello", Some("text/plain"), &["en"]);

        let schema = restricted_text(vec!["en".to_string(), "de".to_string()]);

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        let text_type = text_source.text_type.unwrap();
        assert!(text_type.language_code == "en");
    }

    #[test]
    async fn restricted_text_body_accepts_allowed_language_case_insensitive() {
        let mut request = text_request(b"hello", Some("text/plain"), &["EN"]);

        let schema = restricted_text(vec!["en".to_string()]);

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        let text_type = text_source.text_type.unwrap();
        assert!(text_type.language_code == "EN");
    }

    #[test]
    async fn restricted_text_body_rejects_disallowed_language() {
        let mut request = text_request(b"hello", Some("text/plain"), &["fr"]);

        let schema = restricted_text(vec!["en".to_string(), "de".to_string()]);

        let err = request.parse_request_body(&schema).await.unwrap_err();

        let_assert!(
            RequestHandlerError::UnsupportedLanguage {
                language_code,
                allowed_language_codes,
            } = err
        );
        assert!(language_code == "fr");
        assert!(allowed_language_codes == vec!["en".to_string(), "de".to_string()]);
    }

    #[test]
    async fn restricted_text_body_without_content_language_is_accepted() {
        // Content-Language is optional even when restrictions exist
        let mut request = text_request(b"hello", Some("text/plain"), &[]);

        let schema = restricted_text(vec!["en".to_string()]);

        let result = request.parse_request_body(&schema).await.unwrap();

        let_assert!(ParsedRequestBody::UnstructuredText(Some(text_source)) = result);
        assert!(text_source.text_type.is_none());
    }
}
