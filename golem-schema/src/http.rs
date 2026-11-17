// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

//! Shared HTTP route metadata and path/file-mapping grammar.

use crate::Empty;
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[cfg(feature = "protobuf")]
mod protobuf;
#[cfg(any(feature = "host", feature = "guest"))]
pub mod wit;

#[cfg(test)]
mod tests;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum HttpMethod {
    Get(Empty),
    Head(Empty),
    Post(Empty),
    Put(Empty),
    Delete(Empty),
    Connect(Empty),
    Options(Empty),
    Trace(Empty),
    Patch(Empty),
    Custom(CustomHttpMethod),
}

#[cfg(feature = "full")]
impl TryFrom<HttpMethod> for http::Method {
    type Error = http::method::InvalidMethod;

    fn try_from(value: HttpMethod) -> Result<Self, Self::Error> {
        Ok(match value {
            HttpMethod::Get(_) => http::Method::GET,
            HttpMethod::Head(_) => http::Method::HEAD,
            HttpMethod::Post(_) => http::Method::POST,
            HttpMethod::Put(_) => http::Method::PUT,
            HttpMethod::Delete(_) => http::Method::DELETE,
            HttpMethod::Connect(_) => http::Method::CONNECT,
            HttpMethod::Options(_) => http::Method::OPTIONS,
            HttpMethod::Trace(_) => http::Method::TRACE,
            HttpMethod::Patch(_) => http::Method::PATCH,
            HttpMethod::Custom(value) => http::Method::from_bytes(value.value.as_bytes())?,
        })
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct CustomHttpMethod {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PathSegment {
    Literal(LiteralSegment),
    PathVariable(PathVariable),
    RemainingPathVariable(PathVariable),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct LiteralSegment {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PathVariable {
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HeaderVariable {
    pub header_name: String,
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct QueryVariable {
    pub query_param_name: String,
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct FileResponseHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub enum HttpRouteMode {
    DurableStreams,
    Plain,
    Sse,
    WebSocket,
}

impl HttpRouteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DurableStreams => "durableStreams",
            Self::Plain => "plain",
            Self::Sse => "sse",
            Self::WebSocket => "webSocket",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DurableStreamRouteOptions {
    pub slots: Vec<DurableStreamSlotOptions>,
    pub load: Option<DurableStreamRouteLoadOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DurableStreamSlotOptions {
    pub source: DurableStreamSlotSource,
    pub name: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(
    feature = "full",
    oai(discriminator_name = "type", one_of = true, rename_all = "camelCase")
)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DurableStreamSlotSource {
    Input(DurableStreamInputSlotSource),
    Output(DurableStreamOutputSlotSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct DurableStreamInputSlotSource {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct DurableStreamOutputSlotSource {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DurableStreamRouteLoadOptions {
    pub max_concurrent_readers_per_stream: Option<u32>,
    pub max_append_requests_per_second_per_stream: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum FileMapping {
    Exact(ExactFileMapping),
    Subtree(SubtreeFileMapping),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ExactFileMapping {
    pub public_path: Vec<String>,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SubtreeFileMapping {
    pub public_prefix: Vec<String>,
    pub filesystem_root: String,
}

/// A public request target with one shared segment boundary for routing and files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestTarget {
    path: String,
    query: Option<String>,
    segments: Vec<String>,
    trailing_slash: bool,
}

impl HttpRequestTarget {
    pub fn parse(target: &str) -> Result<Self, String> {
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query.to_string())),
            None => (target, None),
        };
        let remaining = path.strip_prefix('/').ok_or("unsafe-path")?;
        let trailing_slash = !remaining.is_empty() && remaining.ends_with('/');
        let segments = if remaining.is_empty() {
            Vec::new()
        } else {
            let remaining = if trailing_slash {
                &remaining[..remaining.len() - 1]
            } else {
                remaining
            };
            remaining
                .split('/')
                .map(decode_segment)
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self {
            path: path.to_string(),
            query,
            segments,
            trailing_slash,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    pub fn segments(&self) -> &[String] {
        &self.segments
    }
    pub fn trailing_slash(&self) -> bool {
        self.trailing_slash
    }
}

impl FileMapping {
    /// Compile URI source syntax once; targets are filesystem text, not URIs.
    pub fn compile(source: &str, target: &str) -> Result<Self, String> {
        let (source, subtree) = match source.strip_suffix("/*") {
            Some(prefix) if prefix.ends_with('/') => return Err("source-path".into()),
            Some(prefix) => (if prefix.is_empty() { "/" } else { prefix }, true),
            None => (source, false),
        };
        if source.contains('*') {
            return Err("source-wildcard".into());
        }
        if source.contains(['$', '?', '#']) {
            return Err("source-path".into());
        }
        let path = HttpRequestTarget::parse(source).map_err(|_| "source-path")?;
        if path.trailing_slash {
            return Err("source-path".into());
        }
        let (target, allow_root) = if subtree {
            let root = target.strip_suffix("/$1").ok_or("target-placeholder")?;
            if root.ends_with('/') {
                return Err("target-path".into());
            }
            (if root.is_empty() { "/" } else { root }, true)
        } else {
            (target, false)
        };
        validate_file_mapping(&path.segments, target, allow_root)?;
        Ok(if subtree {
            Self::Subtree(SubtreeFileMapping {
                public_prefix: path.segments,
                filesystem_root: target.into(),
            })
        } else {
            Self::Exact(ExactFileMapping {
                public_path: path.segments,
                file_path: target.into(),
            })
        })
    }

    pub fn compile_list<'a>(
        mappings: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Vec<Self>, String> {
        let compiled = mappings
            .into_iter()
            .map(|(source, target)| Self::compile(source, target))
            .collect::<Result<Vec<_>, _>>()?;
        Self::validate_list(&compiled)?;
        Ok(compiled)
    }

    /// Structural metadata already contains decoded public segments.
    pub fn validate(&self) -> Result<(), String> {
        let (segments, target, allow_root) = match self {
            Self::Exact(mapping) => (&mapping.public_path, mapping.file_path.as_str(), false),
            Self::Subtree(mapping) => (
                &mapping.public_prefix,
                mapping.filesystem_root.as_str(),
                true,
            ),
        };
        validate_file_mapping(segments, target, allow_root)
    }

    pub fn validate_list(mappings: &[Self]) -> Result<(), String> {
        let mut seen = HashSet::new();
        for mapping in mappings {
            mapping.validate()?;
            if !seen.insert(mapping) {
                return Err("duplicate-mapping".into());
            }
        }
        Ok(())
    }
}

/// Validate already-decoded structural mapping fields without decoding again.
pub fn validate_file_mapping(
    segments: &[String],
    target: &str,
    allow_root: bool,
) -> Result<(), String> {
    if segments
        .iter()
        .any(|segment| !valid_decoded_segment(segment))
    {
        return Err("source-path".into());
    }
    if target.contains('$') {
        return Err("target-placeholder".into());
    }
    if !target.starts_with('/')
        || target.chars().any(char::is_control)
        || (!allow_root && target == "/")
        || (target != "/"
            && target[1..]
                .split('/')
                .any(|segment| !valid_decoded_segment(segment)))
    {
        return Err("target-path".into());
    }
    Ok(())
}

pub fn valid_decoded_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment
            .bytes()
            .any(|byte| byte <= 0x1f || byte == 0x7f || byte == b'/' || byte == b'\\')
}

fn decode_segment(raw: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(hex).ok_or("unsafe-path")?;
            let low = bytes.next().and_then(hex).ok_or("unsafe-path")?;
            decoded.push(high * 16 + low);
        } else if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            decoded.push(byte);
        } else {
            return Err("unsafe-path".into());
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "unsafe-path")?;
    if !valid_decoded_segment(&decoded) {
        return Err("unsafe-path".into());
    }
    Ok(decoded)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
