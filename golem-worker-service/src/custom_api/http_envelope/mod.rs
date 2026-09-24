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

use http::header::{CONNECTION, CONTENT_LENGTH, EXPECT, HOST, TRANSFER_ENCODING};
use http::uri::Authority;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr};

pub(super) type RawHeaders = Vec<(Vec<u8>, Vec<u8>)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HttpVersion {
    Http1,
    Http2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EnvelopeError {
    InvalidMethod,
    UnsupportedMethod,
    UnsupportedUpgrade,
    UnsupportedExpectation,
    InvalidTarget,
    InvalidHeader,
    InvalidFraming,
    UnsupportedTransferEncoding,
    InvalidOrigin,
    InvalidStatus,
}

impl EnvelopeError {
    pub(super) fn request_status(&self) -> StatusCode {
        match self {
            Self::UnsupportedExpectation => StatusCode::EXPECTATION_FAILED,
            Self::UnsupportedMethod
            | Self::UnsupportedUpgrade
            | Self::UnsupportedTransferEncoding => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RequestHead {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
    /// Validated transport framing, retained even if Connection nominates the field for removal.
    pub content_length: Option<u64>,
    pub chunked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Origin {
    pub scheme: String,
    pub authority: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResponseBodyPolicy {
    Stream,
    /// Dispose the producer without polling and commit only after session success.
    Bodyless,
}

#[derive(Clone, Debug)]
pub(super) struct ResponseHead {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// Validated byte promise for streaming responses, before hop-by-hop header removal.
    /// Bodyless responses apply status-specific length rules and never poll the producer.
    pub content_length: Option<u64>,
    pub body_policy: ResponseBodyPolicy,
}

/// Validate the envelope delivered by the HTTP transport, not the original wire bytes.
/// Hyper collapses equal duplicate Content-Length fields, uses chunked framing for
/// TE+CL (removing CL and closing the connection), and rejects bare unsupported TE
/// with 400 before this runs. We do not duplicate its HTTP/1 parser.
pub(super) fn process_request_head(
    method: &str,
    target: &str,
    headers: &RawHeaders,
    version: HttpVersion,
    h2_authority: Option<&str>,
) -> Result<RequestHead, EnvelopeError> {
    let parsed_method =
        Method::from_bytes(method.as_bytes()).map_err(|_| EnvelopeError::InvalidMethod)?;
    if parsed_method == Method::CONNECT || parsed_method == Method::TRACE {
        return Err(EnvelopeError::UnsupportedMethod);
    }
    if !target.starts_with('/') {
        return Err(EnvelopeError::InvalidTarget);
    }
    let uri: Uri = target.parse().map_err(|_| EnvelopeError::InvalidTarget)?;
    if uri.scheme().is_some() || uri.authority().is_some() {
        return Err(EnvelopeError::InvalidTarget);
    }
    let path_and_query = uri.path_and_query().ok_or(EnvelopeError::InvalidTarget)?;
    let (mut map, nominees) = validate_headers(headers)?;
    for value in map.values_mut() {
        *value = HeaderValue::from_bytes(trim_ows(value.as_bytes()))
            .map_err(|_| EnvelopeError::InvalidHeader)?;
    }
    let content_length = validate_content_length(&map)?;
    let transfer_encoding = values(&map, TRANSFER_ENCODING);
    if !transfer_encoding.is_empty() && content_length.is_some() {
        return Err(EnvelopeError::InvalidFraming);
    }
    let chunked = if transfer_encoding.is_empty() {
        false
    } else if version == HttpVersion::Http1
        && transfer_encoding.len() == 1
        && trim_ows(transfer_encoding[0].as_bytes()).eq_ignore_ascii_case(b"chunked")
    {
        true
    } else {
        return Err(EnvelopeError::UnsupportedTransferEncoding);
    };
    if map.contains_key("upgrade") {
        return Err(EnvelopeError::UnsupportedUpgrade);
    }
    for value in values(&map, EXPECT) {
        if !trim_ows(value.as_bytes()).eq_ignore_ascii_case(b"100-continue") {
            return Err(EnvelopeError::UnsupportedExpectation);
        }
    }
    if values(&map, EXPECT).len() > 1 {
        return Err(EnvelopeError::UnsupportedExpectation);
    }
    validate_host(&map, version, h2_authority)?;
    strip_hop_headers(&mut map, &nominees);
    map.remove(HOST);
    map.remove("forwarded");
    map.remove("x-forwarded-host");
    map.remove("x-forwarded-proto");
    map.remove(EXPECT);
    Ok(RequestHead {
        method: method.to_owned(),
        path: path_and_query.path().to_owned(),
        query: path_and_query.query().map(str::to_owned),
        headers: map,
        content_length,
        chunked,
    })
}

pub(crate) fn derive_origin(
    connection_scheme: &str,
    host: &str,
    headers: &RawHeaders,
    trusted_ingress: bool,
) -> Result<Origin, EnvelopeError> {
    let (map, _) = validate_headers(headers)?;
    let (scheme, authority) = if trusted_ingress {
        let proto =
            singleton_noncomma(&map, "x-forwarded-proto")?.ok_or(EnvelopeError::InvalidOrigin)?;
        let forwarded_host =
            singleton_noncomma(&map, "x-forwarded-host")?.ok_or(EnvelopeError::InvalidOrigin)?;
        (proto, forwarded_host)
    } else {
        (connection_scheme.as_bytes(), host.as_bytes())
    };
    let scheme = std::str::from_utf8(trim_ows(scheme)).map_err(|_| EnvelopeError::InvalidOrigin)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(EnvelopeError::InvalidOrigin);
    }
    let authority = normalize_authority(trim_ows(authority))?;
    Ok(Origin { scheme, authority })
}

pub(crate) fn origin_from_request(
    request: &poem::Request,
    trusted_ingress: bool,
) -> Result<Origin, EnvelopeError> {
    let headers: RawHeaders = request
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    let version = if request.version() == http::Version::HTTP_2 {
        HttpVersion::Http2
    } else {
        HttpVersion::Http1
    };
    let h2_authority = request.uri().authority().map(Authority::as_str);
    let (map, _) = validate_headers(&headers)?;
    validate_host(&map, version, h2_authority)?;
    let host = if version == HttpVersion::Http2 {
        h2_authority.or_else(|| map.get(HOST).and_then(|value| value.to_str().ok()))
    } else {
        map.get(HOST).and_then(|value| value.to_str().ok())
    }
    .ok_or(EnvelopeError::InvalidOrigin)?;
    derive_origin(request.scheme().as_str(), host, &headers, trusted_ingress)
}

pub(super) fn process_response_head(
    request_method: &str,
    status: u16,
    headers: &RawHeaders,
) -> Result<ResponseHead, EnvelopeError> {
    if !(200..=599).contains(&status) {
        return Err(EnvelopeError::InvalidStatus);
    }
    let status = StatusCode::from_u16(status).map_err(|_| EnvelopeError::InvalidStatus)?;
    let (mut map, nominees) = validate_headers(headers)?;
    let mut content_length = validate_content_length(&map)?;
    if !values(&map, TRANSFER_ENCODING).is_empty() && content_length.is_some() {
        return Err(EnvelopeError::InvalidFraming);
    }
    strip_hop_headers(&mut map, &nominees);
    let dispose = request_method == "HEAD" || matches!(status.as_u16(), 204 | 205 | 304);
    match status.as_u16() {
        204 => {
            map.remove(CONTENT_LENGTH);
            content_length = None;
        }
        205 => {
            map.try_insert(CONTENT_LENGTH, HeaderValue::from_static("0"))
                .map_err(|_| EnvelopeError::InvalidHeader)?;
            content_length = Some(0);
        }
        _ => {}
    }
    let body_policy = if dispose || content_length == Some(0) {
        ResponseBodyPolicy::Bodyless
    } else {
        ResponseBodyPolicy::Stream
    };
    Ok(ResponseHead {
        status,
        headers: map,
        content_length,
        body_policy,
    })
}

fn validate_headers(
    headers: &RawHeaders,
) -> Result<(HeaderMap, HashSet<HeaderName>), EnvelopeError> {
    let mut map = HeaderMap::new();
    let mut nominees = HashSet::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name).map_err(|_| EnvelopeError::InvalidHeader)?;
        if value
            .iter()
            .any(|byte| matches!(byte, 0 | 10 | 13 | 127) || (*byte < 32 && *byte != 9))
        {
            return Err(EnvelopeError::InvalidHeader);
        }
        let value = HeaderValue::from_bytes(value).map_err(|_| EnvelopeError::InvalidHeader)?;
        map.try_append(name, value)
            .map_err(|_| EnvelopeError::InvalidHeader)?;
    }
    for value in values(&map, CONNECTION) {
        for token in value.as_bytes().split(|byte| *byte == b',') {
            let token = trim_ows(token);
            if token.is_empty() {
                return Err(EnvelopeError::InvalidHeader);
            }
            nominees
                .insert(HeaderName::from_bytes(token).map_err(|_| EnvelopeError::InvalidHeader)?);
        }
    }
    Ok((map, nominees))
}

fn validate_content_length(map: &HeaderMap) -> Result<Option<u64>, EnvelopeError> {
    let lengths = values(map, CONTENT_LENGTH);
    if lengths.is_empty() {
        return Ok(None);
    }
    if lengths.len() != 1 || lengths[0].as_bytes().contains(&b',') {
        return Err(EnvelopeError::InvalidFraming);
    }
    let bytes = trim_ows(lengths[0].as_bytes());
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(EnvelopeError::InvalidFraming);
    }
    std::str::from_utf8(bytes)
        .unwrap()
        .parse()
        .map(Some)
        .map_err(|_| EnvelopeError::InvalidFraming)
}

fn validate_host(
    map: &HeaderMap,
    version: HttpVersion,
    h2_authority: Option<&str>,
) -> Result<(), EnvelopeError> {
    let hosts = values(map, HOST);
    if hosts.len() > 1
        || hosts
            .first()
            .is_some_and(|host| host.as_bytes().contains(&b','))
    {
        return Err(EnvelopeError::InvalidOrigin);
    }
    let host = hosts
        .first()
        .map(|v| normalize_authority(trim_ows(v.as_bytes())))
        .transpose()?;
    let authority = h2_authority
        .map(|v| normalize_authority(v.as_bytes()))
        .transpose()?;
    if version == HttpVersion::Http2 && host.is_some() && authority.is_some() && host != authority {
        return Err(EnvelopeError::InvalidOrigin);
    }
    Ok(())
}

fn normalize_authority(value: &[u8]) -> Result<String, EnvelopeError> {
    let value = std::str::from_utf8(value).map_err(|_| EnvelopeError::InvalidOrigin)?;
    let authority: Authority = value.parse().map_err(|_| EnvelopeError::InvalidOrigin)?;
    if authority.as_str().contains('@') || authority.host().is_empty() {
        return Err(EnvelopeError::InvalidOrigin);
    }
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let close = rest.find(']').ok_or(EnvelopeError::InvalidOrigin)?;
        let literal = &rest[..close];
        literal
            .parse::<Ipv6Addr>()
            .map_err(|_| EnvelopeError::InvalidOrigin)?;
        let suffix = &rest[close + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or(EnvelopeError::InvalidOrigin)?,
            )
        };
        (format!("[{literal}]").to_ascii_lowercase(), port)
    } else {
        let (host, port) = match value.split_once(':') {
            Some((host, port)) if !port.contains(':') => (host, Some(port)),
            Some(_) => return Err(EnvelopeError::InvalidOrigin),
            None => (value, None),
        };
        validate_origin_host(host)?;
        (host.to_ascii_lowercase(), port)
    };
    if let Some(port) = port {
        if port.is_empty()
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().is_err()
        {
            return Err(EnvelopeError::InvalidOrigin);
        }
        Ok(format!("{host}:{port}"))
    } else {
        Ok(host)
    }
}

fn validate_origin_host(host: &str) -> Result<(), EnvelopeError> {
    if host.is_empty() || !host.is_ascii() {
        return Err(EnvelopeError::InvalidOrigin);
    }
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        host.parse::<Ipv4Addr>()
            .map_err(|_| EnvelopeError::InvalidOrigin)?;
        return Ok(());
    }
    let dns = host.strip_suffix('.').unwrap_or(host);
    if dns.is_empty()
        || host.len() > 254
        || dns.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(EnvelopeError::InvalidOrigin);
    }
    Ok(())
}

fn singleton_noncomma<'a>(
    map: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a [u8]>, EnvelopeError> {
    let found = values(map, name);
    if found.len() > 1
        || found
            .first()
            .is_some_and(|value| value.as_bytes().contains(&b','))
    {
        return Err(EnvelopeError::InvalidOrigin);
    }
    Ok(found.first().map(|value| trim_ows(value.as_bytes())))
}

fn values<N>(map: &HeaderMap, name: N) -> Vec<&HeaderValue>
where
    N: http::header::AsHeaderName,
{
    map.get_all(name).iter().collect()
}

fn trim_ows(mut value: &[u8]) -> &[u8] {
    while value
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[1..];
    }
    while value
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[..value.len() - 1];
    }
    value
}

fn strip_hop_headers(map: &mut HeaderMap, nominees: &HashSet<HeaderName>) {
    for name in nominees {
        map.remove(name);
    }
    for name in [
        "connection",
        "proxy-connection",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
    ] {
        map.remove(name);
    }
}

#[cfg(test)]
mod tests;
