use super::{
    AuthorizationServer, AuthorizationServerMetadata, ProtectedResourceMetadata,
    authorization_metadata_urls, https_url, invalid, protocol, validate_scopes,
};
use crate::transport::{HttpSend, TransportError};
use bytes::Bytes;
use headers::HeaderMapExt;
use http::{HeaderMap, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use oauth2::{AsyncHttpClient, HttpRequest, HttpResponse, RequestTokenError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::{fmt, future::Future, pin::Pin, time::Duration};
use tokio::sync::Mutex;
use url::Url;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub document_bytes: usize,
    pub request_bytes: usize,
    pub challenge_bytes: usize,
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            document_bytes: 1 << 20,
            request_bytes: 64 << 10,
            challenge_bytes: 16 << 10,
            timeout: Duration::from_secs(20),
        }
    }
}

impl Limits {
    pub fn validate(self) -> Result<Self, TransportError> {
        if self.timeout.is_zero() || self.document_bytes == 0 || self.request_bytes == 0 {
            return Err(invalid("invalid OAuth limits"));
        }
        Ok(self)
    }
}

pub struct Discovery {
    pub server: AuthorizationServer,
    pub scopes: Vec<String>,
}

/// Headers belong to the resource's unauthenticated response, never another
/// origin's response. The host must authorize every discovered network target.
/// No credentials are sent during discovery and no failed POST is retried here.
pub async fn discover<S: HttpSend>(
    sender: &mut S,
    resource: &str,
    configured_issuer: &str,
    headers: &HeaderMap,
    limits: Limits,
) -> Result<Discovery, S::Error> {
    let limits = limits.validate()?;
    let resource_url = https_url(resource)?;
    let issuer_urls = authorization_metadata_urls(configured_issuer)?;
    let challenge = parse_challenge(headers, limits.challenge_bytes)?;
    let urls = match challenge.metadata {
        Some(url) => vec![url],
        None => resource_metadata_urls(resource_url),
    };
    tokio::time::timeout(limits.timeout, async {
        let metadata: ProtectedResourceMetadata = fetch_metadata(sender, urls, limits).await?;
        metadata.validate(resource, configured_issuer)?;
        let metadata_scopes = metadata.scopes_supported.unwrap_or_default();
        let scopes = challenge.scopes.unwrap_or(metadata_scopes);
        validate_scopes(&scopes)?;
        let metadata: AuthorizationServerMetadata =
            fetch_metadata(sender, issuer_urls, limits).await?;
        let server = AuthorizationServer::validate(metadata, configured_issuer)?;
        Ok(Discovery { server, scopes })
    })
    .await
    .map_err(|_| TransportError::Timeout)?
}

#[derive(Default)]
struct Challenge {
    metadata: Option<Url>,
    scopes: Option<Vec<String>>,
}

fn parse_challenge(headers: &HeaderMap, maximum: usize) -> Result<Challenge, TransportError> {
    let mut remaining = maximum;
    let mut selected = None;
    for value in headers.get_all(header::WWW_AUTHENTICATE) {
        remaining = remaining
            .checked_sub(value.len())
            .ok_or_else(|| TransportError::Limit("OAuth challenge bytes".into()))?;
        let value = value
            .to_str()
            .map_err(|_| protocol("invalid OAuth challenge"))?;
        for challenge in http_auth::ChallengeParser::new(value) {
            let challenge = challenge.map_err(|_| protocol("invalid OAuth challenge"))?;
            if !challenge.scheme.eq_ignore_ascii_case("Bearer") {
                continue;
            }
            if selected.is_some() {
                return Err(protocol("ambiguous OAuth bearer challenges"));
            }
            let mut parsed = Challenge::default();
            let mut names = std::collections::BTreeSet::new();
            for (name, value) in challenge.params {
                if !names.insert(name.to_ascii_lowercase()) {
                    return Err(protocol("duplicate OAuth challenge parameter"));
                }
                if name.eq_ignore_ascii_case("resource_metadata") {
                    parsed.metadata = Some(https_url(&value.to_unescaped())?);
                } else if name.eq_ignore_ascii_case("scope") {
                    let scopes: Vec<_> =
                        value.to_unescaped().split(' ').map(str::to_owned).collect();
                    validate_scopes(&scopes)?;
                    parsed.scopes = Some(scopes);
                }
            }
            selected = Some(parsed);
        }
    }
    Ok(selected.unwrap_or_default())
}

fn resource_metadata_urls(mut resource: Url) -> Vec<Url> {
    let path = resource.path().trim_end_matches('/').to_owned();
    resource.set_path(&format!("/.well-known/oauth-protected-resource{path}"));
    let mut urls = vec![resource.clone()];
    if !path.is_empty() || resource.query().is_some() {
        resource.set_query(None);
        resource.set_path("/.well-known/oauth-protected-resource");
        urls.push(resource);
    }
    urls
}

async fn fetch_metadata<S: HttpSend, T: DeserializeOwned>(
    sender: &mut S,
    urls: Vec<Url>,
    limits: Limits,
) -> Result<T, S::Error> {
    for url in urls {
        let request = Request::get(url.as_str())
            .header(header::ACCEPT, "application/json")
            .header(header::ACCEPT_ENCODING, "identity")
            .body(Bytes::new())
            .map_err(|_| invalid("invalid OAuth metadata request"))?;
        let response = sender.send(request).await?;
        if response.status() == StatusCode::NOT_FOUND {
            continue;
        }
        if response.status() != StatusCode::OK {
            return Err(TransportError::HttpStatus(response.status().as_u16()).into());
        }
        let response = read_json(response, limits.document_bytes).await?;
        return serde_json::from_slice(response.body())
            .map_err(|_| protocol("invalid OAuth metadata").into());
    }
    Err(protocol("OAuth metadata not found").into())
}

async fn read_json<B: http_body::Body<Data = Bytes>>(
    response: Response<B>,
    maximum: usize,
) -> Result<HttpResponse, TransportError> {
    let headers = response.headers();
    if let Some(length) = headers
        .typed_try_get::<headers::ContentLength>()
        .map_err(|_| protocol("invalid OAuth Content-Length"))?
        && length.0 > maximum as u64
    {
        return Err(TransportError::Limit("OAuth document bytes".into()));
    }
    if headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .flat_map(|v| v.as_bytes().split(|b| *b == b','))
        .map(|v| v.trim_ascii())
        .any(|v| !v.is_empty() && !v.eq_ignore_ascii_case(b"identity"))
    {
        return Err(protocol("unsupported OAuth Content-Encoding"));
    }
    let content_type = headers
        .typed_try_get::<headers::ContentType>()
        .map_err(|_| protocol("invalid OAuth Content-Type"))?
        .map(mime::Mime::from);
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || !content_type.is_some_and(|mime| mime.essence_str() == "application/json")
    {
        return Err(protocol("expected OAuth JSON Content-Type"));
    }
    let (parts, body) = response.into_parts();
    let mut body = Box::pin(body);
    let mut bytes = Vec::new();
    loop {
        tokio::task::yield_now().await;
        let Some(frame) = body.frame().await else {
            break;
        };
        if let Ok(chunk) = frame.map_err(|_| TransportError::Network)?.into_data() {
            if chunk.len() > maximum.saturating_sub(bytes.len()) {
                return Err(TransportError::Limit("OAuth document bytes".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
    }
    Ok(Response::from_parts(parts, bytes))
}

pub(super) struct TokenSender<'a, S> {
    sender: Mutex<&'a mut S>,
    limits: Limits,
}

impl<'a, S> TokenSender<'a, S> {
    pub(super) fn new(sender: &'a mut S, limits: Limits) -> Result<Self, TransportError> {
        Ok(Self {
            sender: Mutex::new(sender),
            limits: limits.validate()?,
        })
    }
}

// The library requires an Error implementation, but host traps (including
// anyhow::Error) need only be Send. Do not format those or provider responses.
pub(super) struct SendFailure<E>(E);
impl<E> fmt::Debug for SendFailure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuth HTTP failure")
    }
}
impl<E> fmt::Display for SendFailure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuth HTTP failure")
    }
}
impl<E: 'static> std::error::Error for SendFailure<E> {}

impl<'c, S: HttpSend + Send> AsyncHttpClient<'c> for TokenSender<'_, S>
where
    S::Error: 'static,
{
    type Error = SendFailure<S::Error>;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, Self::Error>> + Send + 'c>>;

    fn call(&'c self, mut request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let result = tokio::time::timeout(self.limits.timeout, async {
                if request.body().len() > self.limits.request_bytes {
                    return Err(TransportError::Limit("OAuth request bytes".into()).into());
                }
                request.headers_mut().insert(
                    header::ACCEPT_ENCODING,
                    http::HeaderValue::from_static("identity"),
                );
                if let Some(value) = request.headers_mut().get_mut(header::AUTHORIZATION) {
                    value.set_sensitive(true);
                }
                let response = self
                    .sender
                    .lock()
                    .await
                    .send(request.map(Bytes::from))
                    .await?;
                if !matches!(
                    response.status(),
                    StatusCode::OK | StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
                ) {
                    return Err(TransportError::HttpStatus(response.status().as_u16()).into());
                }
                read_json(response, self.limits.document_bytes)
                    .await
                    .map_err(Into::into)
            })
            .await
            .unwrap_or_else(|_| Err(TransportError::Timeout.into()));
            result.map_err(SendFailure)
        })
    }
}

pub(super) fn exchange_error<E: From<TransportError>>(
    error: RequestTokenError<SendFailure<E>, oauth2::basic::BasicErrorResponse>,
) -> E {
    use oauth2::basic::BasicErrorResponseType;
    match error {
        RequestTokenError::Request(SendFailure(error)) => error,
        RequestTokenError::ServerResponse(response) => match response.error() {
            BasicErrorResponseType::InvalidGrant => TransportError::OAuthGrantRejected.into(),
            BasicErrorResponseType::InvalidClient => invalid("OAuth client rejected").into(),
            BasicErrorResponseType::InvalidRequest => {
                invalid("OAuth token request rejected").into()
            }
            BasicErrorResponseType::InvalidScope => invalid("OAuth scope rejected").into(),
            BasicErrorResponseType::UnauthorizedClient => {
                invalid("OAuth client not authorized").into()
            }
            BasicErrorResponseType::UnsupportedGrantType => {
                invalid("OAuth grant type unsupported").into()
            }
            _ => protocol("unrecognized OAuth token error").into(),
        },
        // SDK parse/server errors may contain tokens or reflected client secrets.
        _ => protocol("invalid OAuth token response").into(),
    }
}
