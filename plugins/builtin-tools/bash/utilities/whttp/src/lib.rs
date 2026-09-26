//! `whttp` — the shared HTTP transport behind `wcurl` and `waget`.
//!
//! One cfg-gated client seam (`wasi-fetch` on wasm, `reqwest` on native) plus **target-agnostic**
//! redirect following and timeouts, so the two command clients behave identically on native and on
//! the Golem agent. This is the "fourth crate" the duplicated seams in `wcurl`/`waget` anticipated.
//!
//! Redirect following lives here, not in the transport: both clients follow redirects on their own
//! (`reqwest` up to 10 by default, `wasi-fetch` likewise), and their policies differ in the details,
//! so leaving it to the transport made `curl`/`wget` behave differently on the two targets. Each
//! arm disables its client's own follower — `redirect(none)` natively, `redirect_limit(0)` on wasm —
//! so this loop is the single source of truth. `Location` resolution (see [`resolve_url`]) uses
//! `iri-string` for RFC 3986 reference resolution — pure-Rust and wasm-clean, avoiding the `url`
//! crate's `idna`→`icu` Unicode tables.
//!
//! [`fetch`] is `async` and creates no runtime — the caller awaits it under whatever executor is
//! live. On the agent that is the executor golem-rust drives agent methods with; in the standalone
//! `wcurl`/`waget` binaries it is the `block_on` their `main` supplies.

use http::Method;
use std::time::Duration;

/// A request to perform, including redirect and timeout policy.
#[derive(Clone, Debug)]
pub struct Request {
    /// The HTTP method to perform.
    pub method: Method,
    /// The absolute URL to request.
    pub url: String,
    /// Request headers, as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// An optional request body.
    pub body: Option<Vec<u8>>,
    /// Follow 3xx `Location` responses (curl `-L`; wget follows by default).
    pub follow_redirects: bool,
    /// Cap on redirects when following, to bound a redirect loop.
    pub max_redirects: u32,
    /// Cap on the response body the caller will buffer, in bytes. The whole body is held in memory, so
    /// this bounds peak allocation — which on the wasm agent (fixed linear memory) is the difference
    /// between an error and a trap.
    pub max_body: usize,
    /// Time budget for establishing the connection.
    pub connect_timeout: Option<Duration>,
    /// Overall time budget for the request (maps to reqwest's total timeout; on wasm, the
    /// first-byte timeout — the closest WASI-HTTP primitive).
    pub timeout: Option<Duration>,
    /// `curl --compressed`: send `Accept-Encoding: gzip, deflate` and transparently decode a
    /// `gzip`/`deflate` response body. The `Content-Encoding` header is left as the server sent it
    /// (matching curl, which reports it in `-i`/`-v` output even though the body it hands you back
    /// is already decoded) — only the body bytes and, for the buffered path, `size_download`, are
    /// affected.
    pub compressed: bool,
}

/// Default bound on establishing a connection, applied when the caller sets none.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default overall request budget, applied when the caller sets none. On wasm this bounds the
/// WHOLE operation — connect, headers and body — not just the first byte (see [`Deadline`]).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

/// Default cap on a buffered response body. Generous for API responses and ordinary downloads,
/// while still bounding peak memory on an agent with fixed linear memory that never restarts.
pub const DEFAULT_MAX_BODY: usize = 64 * 1024 * 1024;

/// Stands in for "no timeout" (curl `-m 0`, wget `-T 0`): long enough that no real invocation
/// ever reaches it, but finite, since it is added to an `Instant` and an actually-unbounded
/// `Duration` (`Duration::MAX`) would overflow that addition.
pub const NO_TIMEOUT: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

impl Request {
    /// A plain `GET` with no redirect following and no explicit timeouts — the base every client
    /// tweaks. `None` here means "use [`DEFAULT_CONNECT_TIMEOUT`] / [`DEFAULT_TIMEOUT`]", not
    /// "unbounded"; [`fetch`] resolves them.
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Request {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            follow_redirects: false,
            max_redirects: 50,
            max_body: DEFAULT_MAX_BODY,
            connect_timeout: None,
            timeout: None,
            compressed: false,
        }
    }
}

/// A completed HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The HTTP status code.
    pub status: u16,
    /// Response headers, as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// The response body bytes.
    pub body: Vec<u8>,
    /// The URL the response actually came from (differs from the request URL after redirects) —
    /// used for `-w`, `--content-disposition`, and wget's default filename.
    pub final_url: String,
    /// How many redirect hops [`fetch`] followed to reach this response — curl's `%{num_redirects}`.
    pub num_redirects: u32,
}

impl Response {
    /// Case-insensitive header lookup (the first match wins).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A transport or policy failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The host name did not resolve (curl's exit 6). Holds the transport's own description.
    Resolve(String),
    /// Nothing accepted the connection: refused, or the destination unreachable (curl's exit 7).
    /// Holds the transport's own description.
    Connect(String),
    /// The server's TLS certificate was not accepted (curl's exit 60). Holds the transport's own
    /// description.
    Certificate(String),
    /// Any other transport failure: a TLS or protocol error, an unreadable body, …
    Transport(String),
    /// The request's total time budget (`-m`/`--max-time`, `-T`) ran out, before the response
    /// or while its body was still arriving.
    Timeout,
    /// `follow_redirects` was set and the chain exceeded `max_redirects`.
    TooManyRedirects(u32),
    /// A `Location` header could not be resolved against the current URL.
    BadRedirect(String),
    /// The response body exceeded `max_body`. Carries the cap, not the actual size — on the
    /// streaming path the read is abandoned as soon as the cap is crossed, so the true size is
    /// unknown (which is the point).
    BodyTooLarge(usize),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Resolve(m) | Error::Connect(m) | Error::Certificate(m) | Error::Transport(m) => {
                write!(f, "{m}")
            }
            Error::Timeout => write!(f, "Operation timed out"),
            Error::TooManyRedirects(n) => write!(f, "too many redirects (exceeded {n})"),
            Error::BadRedirect(loc) => write!(f, "could not resolve redirect to {loc}"),
            Error::BodyTooLarge(cap) => write!(f, "response body exceeded {cap} bytes"),
        }
    }
}

impl std::error::Error for Error {}

/// Perform `req`, following redirects per its policy. Returns the final [`Response`].
///
/// # Errors
///
/// Returns [`Error::Resolve`], [`Error::Connect`] or [`Error::Certificate`] when the host does not
/// resolve, nothing accepts the connection or its certificate is refused, [`Error::Timeout`] when
/// the time budget runs out, [`Error::Transport`] on any other transport-level failure (a TLS or
/// protocol error, an unreadable body), [`Error::TooManyRedirects`] when `follow_redirects` is set and the chain exceeds
/// `max_redirects`, and [`Error::BadRedirect`] when a `Location` header cannot be resolved against the
/// current URL.
pub async fn fetch(req: &Request) -> Result<Response, Error> {
    let mut method = req.method.clone();
    let mut url = req.url.clone();
    let mut body = req.body.clone();
    let mut redirects = 0u32;
    // Headers travel with the chain so credentials can be DROPPED at an origin change (below).
    let mut headers = req.headers.clone();

    // Resolve the timeout defaults ONCE, here, so BOTH targets get them. They used to be applied
    // inside the native `fetch_once` only — which left the wasm path (the durable Golem agent)
    // unbounded, i.e. the mitigation landed on the target that doesn't have the problem. A hung peer
    // holds a durable invocation forever, and because Golem serializes invocations per instance it
    // wedges everything queued behind it. Callers wanting a different bound (wcurl `-m`, waget `-T`)
    // still override by setting the fields.
    let connect_timeout = req.connect_timeout.or(Some(DEFAULT_CONNECT_TIMEOUT));
    let timeout = req.timeout.or(Some(DEFAULT_TIMEOUT));

    // `--compressed`: ask for it on every hop (once, up front — same header set travels the
    // redirect chain like any other, credential-dropping aside).
    if req.compressed
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push(("Accept-Encoding".to_string(), "gzip, deflate".to_string()));
    }

    loop {
        let resp = fetch_once(
            &method,
            &url,
            &headers,
            body.clone(),
            connect_timeout,
            timeout,
            req.max_body,
            req.compressed,
        )
        .await?;

        if req.follow_redirects
            && is_redirect(resp.status)
            && let Some(location) = resp.header("location")
        {
            if redirects >= req.max_redirects {
                return Err(Error::TooManyRedirects(req.max_redirects));
            }
            redirects += 1;
            let next = resolve_url(&url, location)
                .ok_or_else(|| Error::BadRedirect(location.to_string()))?;
            // Drop credentials when the redirect leaves the origin. Headers used to be re-sent
            // verbatim on every hop, so `curl -u user:pass -L http://attacker/x` handed the
            // Basic credential synthesized by wcurl straight to whatever host the 302 named.
            // Real curl requires `--location-trusted` for exactly this, and reqwest's own
            // `remove_sensitive_headers` never runs here because redirects are followed by this
            // loop rather than by reqwest.
            if !same_origin(&url, &next) {
                headers.retain(|(name, _)| !is_credential_header(name));
            }
            (method, body) = redirect_method(&method, resp.status, body);
            url = next;
            continue;
        }

        return Ok(Response {
            final_url: url,
            num_redirects: redirects,
            ..resp
        });
    }
}

/// A [`fetch_streaming`] response: the status/headers are already in hand (same as [`Response`]),
/// but the body is read on demand via [`BodyStream::next_chunk`] instead of being buffered up
/// front, so that `curl URL | head -c 100` and `-o FILE` do not hold the whole body
/// in memory before the first byte reaches the caller.
#[derive(Debug)]
pub struct StreamingResponse {
    /// The HTTP status code.
    pub status: u16,
    /// Response headers, as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// The URL the response actually came from (differs from the request URL after redirects).
    pub final_url: String,
    /// How many redirect hops were followed to reach this response.
    pub num_redirects: u32,
    /// The body, read one chunk at a time.
    pub body: BodyStream,
}

impl StreamingResponse {
    /// Case-insensitive header lookup (the first match wins) — mirrors [`Response::header`].
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The still-open response body behind a [`StreamingResponse`], read on demand.
#[derive(Debug)]
pub struct BodyStream {
    inner: BodyStreamInner,
    /// The total-budget deadline (curl `-m`/wget `-T`) each `next_chunk()` checks after its read,
    /// on wasm; `None` on native, where the transport's own request-level timeout already bounds
    /// the whole response including the body.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    deadline: Option<std::time::Instant>,
}

#[cfg(target_arch = "wasm32")]
enum BodyStreamInner {
    Open(wasi_fetch::Body),
    Empty,
}

// `wasi_fetch::Body` has no `Debug` impl; a fixed label is enough for this crate's own `Debug`
// derives (`StreamingResponse`, `BodyStream`) to compile — nothing inspects this output.
#[cfg(target_arch = "wasm32")]
impl std::fmt::Debug for BodyStreamInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyStreamInner::Open(_) => f.write_str("Open(..)"),
            BodyStreamInner::Empty => f.write_str("Empty"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
enum BodyStreamInner {
    Open(reqwest::Response),
    Empty,
}

impl BodyStream {
    /// The next chunk of the body, or `None` at EOF. Each call reads only what the transport has
    /// already buffered/received — nothing is read ahead of what the caller asks for.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Timeout`] once the request's budget has run out, and (natively)
    /// [`Error::Transport`] if the connection fails while reading. On wasm `wasi-fetch`'s `chunk()`
    /// reports a failed read as end-of-body; one that ends past the deadline is the budget's
    /// between-bytes timeout, and is reported as such.
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
        match &mut self.inner {
            #[cfg(target_arch = "wasm32")]
            BodyStreamInner::Open(body) => {
                // Bounded by the same deadline `-m`/`--max-time` set for the whole request: a
                // slowly trickling body (headers already in hand) must not run past it either.
                let chunk = body.chunk().await;
                if self
                    .deadline
                    .is_some_and(|deadline| std::time::Instant::now() >= deadline)
                {
                    return Err(Error::Timeout);
                }
                Ok(chunk.map(|bytes| bytes.to_vec()))
            }
            #[cfg(not(target_arch = "wasm32"))]
            BodyStreamInner::Open(response) => response
                .chunk()
                .await
                .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
                .map_err(|e| native_error("reading response failed", &e)),
            BodyStreamInner::Empty => Ok(None),
        }
    }
}

/// Drain a body to completion without keeping it (an intermediate redirect hop's body — normally
/// empty, but a misbehaving server could send one), bounded by `cap` so a redirect response can't
/// be used to force unbounded reads either.
async fn drain(stream: &mut BodyStream, cap: usize) -> Result<(), Error> {
    let mut total = 0usize;
    while let Some(chunk) = stream.next_chunk().await? {
        total += chunk.len();
        if total > cap {
            return Err(Error::BodyTooLarge(cap));
        }
    }
    Ok(())
}

/// Like [`fetch`], but the final response's body is handed back UNREAD as a [`BodyStream`] instead
/// of being buffered into memory first. Redirect hops (which curl/wget also never stream — their
/// bodies are typically empty and are not what the caller asked for) are still read to completion
/// internally, bounded by `req.max_body`, so a chain never half-consumes a connection.
///
/// `req.max_body` does NOT bound the streamed response: the caller reads (and can stop reading)
/// chunk by chunk, so the memory bound is whatever the caller chooses to hold onto, not a fixed
/// cap here — the same shape as `cat`/`head` piping an upstream producer.
///
/// `req.compressed` is honored on the REQUEST side (the `Accept-Encoding` header is still sent),
/// but the response is NOT transparently decoded here: `gzip`/`deflate` decoding needs the whole
/// framed stream, and a chunk-at-a-time inflate is future work this milestone didn't reach.
/// Callers that need decoded output with `--compressed` should use the buffered [`fetch`] instead;
/// `wcurl` only takes the streaming path when `--compressed` was not requested.
///
/// # Errors
///
/// Same error cases as [`fetch`].
pub async fn fetch_streaming(req: &Request) -> Result<StreamingResponse, Error> {
    let mut method = req.method.clone();
    let mut url = req.url.clone();
    let mut body = req.body.clone();
    let mut redirects = 0u32;
    let mut headers = req.headers.clone();
    let connect_timeout = req.connect_timeout.or(Some(DEFAULT_CONNECT_TIMEOUT));
    let timeout = req.timeout.or(Some(DEFAULT_TIMEOUT));
    if req.compressed
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push(("Accept-Encoding".to_string(), "gzip, deflate".to_string()));
    }

    loop {
        let (status, resp_headers, mut stream) = fetch_once_streaming(
            &method,
            &url,
            &headers,
            body.clone(),
            connect_timeout,
            timeout,
        )
        .await?;

        if req.follow_redirects
            && is_redirect(status)
            && let Some(location) = resp_headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone())
        {
            if redirects >= req.max_redirects {
                return Err(Error::TooManyRedirects(req.max_redirects));
            }
            redirects += 1;
            drain(&mut stream, req.max_body).await?;
            let next =
                resolve_url(&url, &location).ok_or_else(|| Error::BadRedirect(location.clone()))?;
            if !same_origin(&url, &next) {
                headers.retain(|(name, _)| !is_credential_header(name));
            }
            (method, body) = redirect_method(&method, status, body);
            url = next;
            continue;
        }

        return Ok(StreamingResponse {
            status,
            headers: resp_headers,
            final_url: url,
            num_redirects: redirects,
            body: stream,
        });
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_once_streaming(
    method: &Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    connect_timeout: Option<Duration>,
    timeout: Option<Duration>,
) -> Result<(u16, Vec<(String, String)>, BodyStream), Error> {
    let mut builder = wasi_fetch::Client::new()
        .request(method.clone(), url)
        .redirect_limit(0);
    for (k, v) in headers {
        let name = http::HeaderName::try_from(k.as_str())
            .map_err(|e| Error::Transport(format!("bad request header '{k}': {e}")))?;
        let value = http::HeaderValue::try_from(v.as_str())
            .map_err(|e| Error::Transport(format!("bad value for request header '{k}': {e}")))?;
        builder = builder.header(name, value);
    }
    if let Some(bytes) = body {
        builder = builder.body(bytes);
    }
    // See `fetch_once`. The deadline carries across every later `next_chunk()` too, via
    // `BodyStream`'s own copy, so a slowly trickling body remains bounded after this returns.
    let deadline = Deadline::start(timeout.unwrap_or(DEFAULT_TIMEOUT));
    let builder = deadline.bound(builder, connect_timeout);
    let response = deadline.sent(builder.send().await)?;
    let status = response.status().as_u16();
    let headers = collect_headers(response.headers());
    let stream = if is_bodyless(method, status) {
        BodyStream {
            inner: BodyStreamInner::Empty,
            deadline: Some(deadline.at),
        }
    } else {
        BodyStream {
            inner: BodyStreamInner::Open(response.into_body()),
            deadline: Some(deadline.at),
        }
    };
    Ok((status, headers, stream))
}

#[cfg(not(target_arch = "wasm32"))]
async fn fetch_once_streaming(
    method: &Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    connect_timeout: Option<Duration>,
    timeout: Option<Duration>,
) -> Result<(u16, Vec<(String, String)>, BodyStream), Error> {
    let mut cb = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if let Some(d) = connect_timeout {
        cb = cb.connect_timeout(d);
    }
    if let Some(d) = timeout {
        cb = cb.timeout(d);
    }
    let client = cb
        .build()
        .map_err(|e| Error::Transport(format!("client init failed: {e}")))?;
    let mut builder = client.request(method.clone(), url);
    for (k, v) in headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if let Some(bytes) = body {
        builder = builder.body(bytes);
    }
    let response = builder
        .send()
        .await
        .map_err(|e| native_error("request failed", &e))?;
    let status = response.status().as_u16();
    let headers = collect_headers(response.headers());
    let stream = if is_bodyless(method, status) {
        BodyStream {
            inner: BodyStreamInner::Empty,
            deadline: None,
        }
    } else {
        BodyStream {
            inner: BodyStreamInner::Open(response),
            deadline: None,
        }
    };
    Ok((status, headers, stream))
}

/// A request's total time budget (curl `-m`/`--max-time`, wget `-T`) on wasm, without a timer.
///
/// A timer raced against the request is a second durable call in flight beside it, and a crash in
/// the middle of a non-idempotent request (POST, PATCH) is then recovered unreliably by Golem: its
/// replay can fail and leave the agent unusable. Instead WASI-HTTP's own timeouts bound each wait
/// — connect and first byte by the connect budget (capped at the total), each gap between body
/// bytes by the total budget — and the clock is read after every wait: a request still waiting
/// or reading at the deadline fails with [`Error::Timeout`] as soon as the wait returns. A body
/// that keeps arriving is cut at the deadline; one that stops arriving just before it is cut by
/// the between-bytes timeout, so the whole transfer takes at most about twice the budget.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug)]
struct Deadline {
    at: std::time::Instant,
    budget: Duration,
}

#[cfg(target_arch = "wasm32")]
impl Deadline {
    fn start(budget: Duration) -> Self {
        Self {
            at: std::time::Instant::now() + budget,
            budget,
        }
    }

    /// Bounds the request's waits by WASI-HTTP's own timeouts.
    fn bound(
        self,
        builder: wasi_fetch::RequestBuilder,
        connect_timeout: Option<Duration>,
    ) -> wasi_fetch::RequestBuilder {
        let first_byte = connect_timeout.map_or(self.budget, |d| d.min(self.budget));
        builder
            .timeout(first_byte)
            .between_bytes_timeout(self.budget)
    }

    /// Fails with [`Error::Timeout`] once the budget has run out.
    fn check(self) -> Result<(), Error> {
        if std::time::Instant::now() >= self.at {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }

    /// The outcome of sending the request: a WASI-HTTP timeout, or any failure past the
    /// deadline, is the budget running out.
    fn sent<T>(self, result: Result<T, wasi_fetch::Error>) -> Result<T, Error> {
        match result {
            Ok(value) => {
                self.check()?;
                Ok(value)
            }
            Err(error) => {
                self.check()?;
                let message = error.to_string();
                if message.contains("Timeout") {
                    Err(Error::Timeout)
                } else {
                    Err(classify(format!("request failed: {message}")))
                }
            }
        }
    }
}

/// A WASI-HTTP failure by its `error-code` (which `wasi-fetch` passes on in its message): one
/// that resolving the host, connecting, or checking a certificate met, or any other.
#[cfg(target_arch = "wasm32")]
fn classify(message: String) -> Error {
    const RESOLVE: &[&str] = &["DnsError", "DestinationNotFound"];
    const CONNECT: &[&str] = &[
        "ConnectionRefused",
        "DestinationUnavailable",
        "DestinationIpProhibited",
        "DestinationIpUnroutable",
    ];
    let is = |codes: &[&str]| {
        codes
            .iter()
            .any(|code| message.contains(&format!("ErrorCode::{code}")))
    };
    if is(RESOLVE) {
        Error::Resolve(message)
    } else if is(CONNECT) {
        Error::Connect(message)
    } else if is(&["TlsCertificateError"]) {
        Error::Certificate(message)
    } else {
        Error::Transport(message)
    }
}

/// A `reqwest` failure as a [`Error`]: its own timeout is the request's time budget running out,
/// and a failed connection is a failed name lookup when its cause says so.
#[cfg(not(target_arch = "wasm32"))]
fn native_error(context: &str, error: &reqwest::Error) -> Error {
    if error.is_timeout() {
        return Error::Timeout;
    }
    let message = format!("{context}: {error}");
    if error.is_connect() {
        let mut causes = std::iter::successors(
            std::error::Error::source(error),
            |cause: &&dyn std::error::Error| cause.source(),
        );
        if causes.any(|cause| cause.to_string().contains("dns error")) {
            Error::Resolve(message)
        } else {
            Error::Connect(message)
        }
    } else {
        Error::Transport(message)
    }
}

/// The redirect status codes a client following redirects acts on.
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Whether the response carries no body by spec: a HEAD request, or a `204 No Content` / `304 Not
/// Modified` status. Reading a body that never comes half-consumes the connection.
fn is_bodyless(method: &Method, status: u16) -> bool {
    *method == Method::HEAD || matches!(status, 204 | 304)
}

/// The method and body for the next hop, following curl/browser convention: a `303` (and a `POST`
/// under `301`/`302`) becomes a bodyless `GET`; `307`/`308` (and non-`POST` `301`/`302`) preserve
/// the method and body.
fn redirect_method(
    method: &Method,
    status: u16,
    body: Option<Vec<u8>>,
) -> (Method, Option<Vec<u8>>) {
    match status {
        303 => (Method::GET, None),
        301 | 302 if *method == Method::POST => (Method::GET, None),
        _ => (method.clone(), body),
    }
}

/// Whether a header carries a credential that must not follow a redirect off-origin — and, for
/// callers that display request headers, must not be shown verbatim.
///
/// Matched case-insensitively (HTTP field names are case-insensitive). `Cookie` is included because
/// a cookie is a bearer credential in practice, whatever its scoping rules say.
///
/// Public so `wcurl -v` masks exactly the same set the redirect logic drops; two lists that must
/// agree are a list that eventually won't.
#[must_use]
pub fn is_credential_header(name: &str) -> bool {
    const CREDENTIAL_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];
    CREDENTIAL_HEADERS
        .iter()
        .any(|c| name.eq_ignore_ascii_case(c))
}

/// The `scheme://host:port` origin of an absolute URL, lowercased. `None` if it will not parse.
fn origin_of(url: &str) -> Option<(String, String, Option<u16>)> {
    use iri_string::types::UriAbsoluteStr;
    let uri = UriAbsoluteStr::new(url.split('#').next().unwrap_or(url)).ok()?;
    let authority = uri.authority_components()?;
    Some((
        uri.scheme_str().to_ascii_lowercase(),
        authority.host().to_ascii_lowercase(),
        authority.port().and_then(|p| p.parse().ok()),
    ))
}

/// Whether two URLs share a scheme, host and port.
///
/// An unparseable URL on either side answers `false` — the conservative direction, since the caller
/// uses this to decide whether to KEEP credentials.
fn same_origin(a: &str, b: &str) -> bool {
    /// The port a scheme implies when the URL omits one.
    fn effective_port(scheme: &str, port: Option<u16>) -> Option<u16> {
        port.or(match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        })
    }
    match (origin_of(a), origin_of(b)) {
        (Some((sa, ha, pa)), Some((sb, hb, pb))) => {
            sa == sb && ha == hb && effective_port(&sa, pa) == effective_port(&sb, pb)
        }
        _ => false,
    }
}

/// Resolve a `Location` value (absolute, network-path, absolute-path, or relative) against the
/// current absolute URL — RFC 3986 §5 reference resolution, via `iri-string` (pure-Rust, no
/// `idna`/`icu`, wasm-clean). Returns `None` if the base isn't a valid absolute URI or the location
/// isn't a valid URI reference — the caller turns that into a `BadRedirect`.
fn resolve_url(base: &str, location: &str) -> Option<String> {
    use iri_string::types::{UriAbsoluteStr, UriReferenceStr};

    let loc = location.trim();
    // An empty `Location` re-resolves to the current URL: keep the base verbatim (nothing to merge).
    if loc.is_empty() {
        return Some(base.to_string());
    }
    // RFC 3986 §5.1: the base's own fragment plays no part in resolution, and the `absolute-URI`
    // grammar `iri-string` enforces for a base forbids one — so drop any `#…` before parsing.
    let base_no_frag = base.split('#').next().unwrap_or(base);
    let base_uri = UriAbsoluteStr::new(base_no_frag).ok()?;
    let reference = UriReferenceStr::new(loc).ok()?;
    Some(reference.resolve_against(base_uri).to_string())
}

// ---------------------------------------------------------------------------------------------
// The cfg-gated transport: a SINGLE request/response, no redirect handling (the loop above owns
// that). Returns a `Response` whose `final_url` the caller overwrites after the loop settles.
// ---------------------------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
async fn fetch_once(
    method: &Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    connect_timeout: Option<Duration>,
    timeout: Option<Duration>,
    max_body: usize,
    compressed: bool,
) -> Result<Response, Error> {
    // `redirect_limit(0)` is load-bearing, exactly as `redirect(none)` is on the native arm: the
    // shared loop above is the single source of redirect behavior across both targets.
    let mut builder = wasi_fetch::Client::new()
        .request(method.clone(), url)
        .redirect_limit(0);
    // Build each header value up front rather than letting the builder take `&str`: `wasi-fetch`
    // SILENTLY DROPS a header whose name or value does not parse, which would turn a malformed
    // `curl -H` into a request that quietly omits it. Reject it instead, as the native arm does.
    for (k, v) in headers {
        let name = http::HeaderName::try_from(k.as_str())
            .map_err(|e| Error::Transport(format!("bad request header '{k}': {e}")))?;
        let value = http::HeaderValue::try_from(v.as_str())
            .map_err(|e| Error::Transport(format!("bad value for request header '{k}': {e}")))?;
        builder = builder.header(name, value);
    }
    if let Some(bytes) = body {
        builder = builder.body(bytes);
    }
    // The total budget (curl `-m`/wget `-T`, defaulting to `DEFAULT_TIMEOUT`) bounds the whole
    // call: connect, headers AND body (see `Deadline`).
    let deadline = Deadline::start(timeout.unwrap_or(DEFAULT_TIMEOUT));
    let builder = deadline.bound(builder, connect_timeout);
    let response = deadline.sent(builder.send().await)?;
    let status = response.status().as_u16();
    let headers = collect_headers(response.headers());
    // A HEAD/204/304 response has NO body. Reading the stream anyway waits on `Content-Length`
    // bytes that never arrive, leaving the connection half-consumed — which on WASI-HTTP wedges
    // the NEXT outbound request. Skip the read for bodyless responses.
    let bytes = if is_bodyless(method, status) {
        Vec::new()
    } else {
        // Reject on the advertised length BEFORE reading, so an honest oversized response never gets
        // allocated at all.
        // (Nested rather than a let-chain: this crate is on the workspace's 2021 edition.)
        if let Some(len) = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            && len > max_body as u64
        {
            return Err(Error::BodyTooLarge(max_body));
        }
        // A chunked response advertises no length, so stream it and stop the moment the cap is
        // crossed. main's wstd arm documented this as a residual gap — its `contents()` was
        // all-or-nothing, so a Content-Length-less body was fully buffered before the post-hoc check
        // could fire, which bounded the returned value but not peak allocation. `wasi-fetch`'s
        // `Body::chunk` reads incrementally, so nothing past the cap is ever held: the gap main had
        // to accept on wstd is closed here rather than inherited.
        let mut body = response.into_body();
        let mut acc: Vec<u8> = Vec::new();
        while let Some(chunk) = {
            let chunk = body.chunk().await;
            deadline.check()?;
            chunk
        } {
            if acc.len() + chunk.len() > max_body {
                return Err(Error::BodyTooLarge(max_body));
            }
            acc.extend_from_slice(&chunk);
        }
        acc
    };
    let (headers, bytes) = decode_body(headers, bytes, compressed, max_body)?;
    Ok(Response {
        status,
        headers,
        body: bytes,
        final_url: url.to_string(),
        num_redirects: 0,
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn fetch_once(
    method: &Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    connect_timeout: Option<Duration>,
    timeout: Option<Duration>,
    max_body: usize,
    compressed: bool,
) -> Result<Response, Error> {
    // `redirect(none)` is load-bearing: the shared loop above is the single source of redirect
    // behavior across both targets, so reqwest's default auto-follow must be off.
    let mut cb = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    // Defaults are resolved by the caller (`fetch`) so both targets share them.
    if let Some(d) = connect_timeout {
        cb = cb.connect_timeout(d);
    }
    if let Some(d) = timeout {
        cb = cb.timeout(d);
    }
    let client = cb
        .build()
        .map_err(|e| Error::Transport(format!("client init failed: {e}")))?;

    let mut builder = client.request(method.clone(), url);
    for (k, v) in headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if let Some(bytes) = body {
        builder = builder.body(bytes);
    }
    let mut response = builder
        .send()
        .await
        .map_err(|e| native_error("request failed", &e))?;
    let status = response.status().as_u16();
    let headers = collect_headers(response.headers());
    // A HEAD/204/304 response has no body — skip the read (matches the wasm path; see the note
    // there on why reading a phantom body is harmful).
    let bytes = if is_bodyless(method, status) {
        Vec::new()
    } else {
        // Reject on the advertised length first, then STREAM with a running bound so a server that
        // lies about (or omits) Content-Length still cannot make us allocate past the cap. This is
        // the property `.bytes()` could not give: it reads to EOF, so a post-hoc length check
        // happens only after the memory is already committed.
        if let Some(len) = response.content_length()
            && len > max_body as u64
        {
            return Err(Error::BodyTooLarge(max_body));
        }
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| native_error("reading response failed", &e))?
        {
            if buf.len() + chunk.len() > max_body {
                return Err(Error::BodyTooLarge(max_body));
            }
            buf.extend_from_slice(&chunk);
        }
        buf
    };
    let (headers, bytes) = decode_body(headers, bytes, compressed, max_body)?;
    Ok(Response {
        status,
        headers,
        body: bytes,
        final_url: url.to_string(),
        num_redirects: 0,
    })
}

/// Decode a `gzip`/`deflate` `Content-Encoding` when `compressed` is set (`curl --compressed`).
/// Leaves the header and body untouched when `compressed` is false, the encoding is absent/
/// unrecognized, or decoding fails (a server lying about its own encoding shouldn't turn into a
/// hard transport error — the caller gets the raw bytes back instead, as curl does when its own
/// decoder is unhappy: it warns and passes the body through). The `Content-Encoding` header is
/// left in place either way; only the body bytes change.
/// Headers plus a decoded body, as [`decode_body`] returns them.
type HeadersAndBody = (Vec<(String, String)>, Vec<u8>);

fn decode_body(
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    compressed: bool,
    max_body: usize,
) -> Result<HeadersAndBody, Error> {
    if !compressed {
        return Ok((headers, body));
    }
    let Some((_, encoding)) = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
    else {
        return Ok((headers, body));
    };
    let decoded = match encoding.trim().to_ascii_lowercase().as_str() {
        "gzip" | "x-gzip" => Some(decode_capped(
            flate2::read::GzDecoder::new(&body[..]),
            max_body,
        )),
        "deflate" => Some(decode_capped(
            flate2::read::ZlibDecoder::new(&body[..]),
            max_body,
        )),
        _ => None,
    };
    match decoded {
        // A compressed body can be many times its wire size; decode it without ever holding more
        // than `max_body` bytes of it, the same cap the still-compressed body was already read
        // under. Without this, Wasmtime's own memory-growth limiter traps the whole component on
        // an innocuous-looking (and small-on-the-wire) decompression bomb, losing the response
        // entirely instead of refusing it the way an oversized body already is.
        Some(Decoded::TooLarge) => Err(Error::BodyTooLarge(max_body)),
        // A decode error (truncated/corrupt stream) falls back to the raw body, as before: this
        // crate never had a "malformed content-encoding" diagnostic, and manufacturing one now
        // is out of scope for closing the size gap.
        Some(Decoded::Failed) | None => Ok((headers, body)),
        Some(Decoded::Ok(out)) => Ok((headers, out)),
    }
}

enum Decoded {
    Ok(Vec<u8>),
    Failed,
    TooLarge,
}

/// Decode all of `reader`, never holding more than `max_body + 1` bytes of output — enough to
/// tell "exactly at the cap" from "over it" without first needing to hold an unbounded amount to
/// find out.
fn decode_capped(mut reader: impl std::io::Read, max_body: usize) -> Decoded {
    use std::io::Read as _;
    let mut out = Vec::new();
    match reader
        .by_ref()
        .take(max_body as u64 + 1)
        .read_to_end(&mut out)
    {
        Ok(_) if out.len() > max_body => Decoded::TooLarge,
        Ok(_) => Decoded::Ok(out),
        Err(_) => Decoded::Failed,
    }
}

/// Flatten an `http::HeaderMap` into `(name, value)` pairs, dropping any header whose value is not
/// valid UTF-8 (non-ASCII header values are vanishingly rare and unusable as text anyway).
fn collect_headers(map: &http::HeaderMap) -> Vec<(String, String)> {
    // `HeaderMap::iter()` yields headers in an UNSPECIFIED (hash) order that can differ between two
    // executions of the same request. On the durable agent that is a replay hazard: the first run
    // records this Vec (e.g. as a `curl -I` eval result) in the oplog, and a later resume re-executes
    // the request — a different iteration order produces a non-matching result and Golem refuses to
    // resume (`Unexpected oplog entry`, INTERNAL_AGENT_RESUME_FAILED). Sort to a stable order so the
    // recorded and replayed outputs are byte-identical. (curl/wget don't promise the wire order
    // anyway, and a stable order also makes `-I`/`-i` output reproducible for tests and demos.)
    let mut headers: Vec<(String, String)> = map
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_string(), v.to_string()))
        })
        .collect();
    headers.sort();
    headers
}

#[cfg(test)]
mod tests {
    // Test code: unwrap/expect on known-good fixtures is correct style. clippy's allow-unwrap-in-tests
    // does not fire here (compound/edge cfg-test detection), so scope it explicitly.
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn redirect_status_detection() {
        for s in [301, 302, 303, 307, 308] {
            assert!(is_redirect(s), "{s} should be a redirect");
        }
        for s in [200, 201, 300, 304, 400, 500] {
            assert!(!is_redirect(s), "{s} should not be a redirect");
        }
    }

    #[test]
    fn post_downgrades_to_get_on_301_302_303() {
        for s in [301, 302, 303] {
            let (m, b) = redirect_method(&Method::POST, s, Some(b"x".to_vec()));
            assert_eq!(m, Method::GET, "POST under {s} → GET");
            assert_eq!(b, None, "body dropped under {s}");
        }
    }

    #[test]
    fn method_and_body_preserved_on_307_308() {
        for s in [307, 308] {
            let (m, b) = redirect_method(&Method::POST, s, Some(b"x".to_vec()));
            assert_eq!(m, Method::POST, "POST preserved under {s}");
            assert_eq!(b, Some(b"x".to_vec()));
        }
    }

    #[test]
    fn non_post_methods_kept_on_301_302() {
        let (m, b) = redirect_method(&Method::PUT, 302, Some(b"x".to_vec()));
        assert_eq!(m, Method::PUT);
        assert_eq!(b, Some(b"x".to_vec()));
    }

    #[test]
    fn resolves_absolute_and_relative_locations() {
        assert_eq!(
            resolve_url("http://a.test/one/two", "https://b.test/x").as_deref(),
            Some("https://b.test/x")
        );
        assert_eq!(
            resolve_url("http://a.test/one/two", "/x").as_deref(),
            Some("http://a.test/x")
        );
        assert_eq!(
            resolve_url("http://a.test/one/two", "three").as_deref(),
            Some("http://a.test/one/three")
        );
        // Host-only base gets a root path.
        assert_eq!(
            resolve_url("http://a.test", "/x").as_deref(),
            Some("http://a.test/x")
        );
        // Network-path reference inherits the scheme.
        assert_eq!(
            resolve_url("https://a.test/p", "//b.test/q").as_deref(),
            Some("https://b.test/q")
        );
        // Query-only reference keeps the base path.
        assert_eq!(
            resolve_url("http://a.test/p/page", "?x=1").as_deref(),
            Some("http://a.test/p/page?x=1")
        );
        // Dot-segments are normalized.
        assert_eq!(
            resolve_url("http://a.test/one/two/three", "../x").as_deref(),
            Some("http://a.test/one/x")
        );
        assert_eq!(
            resolve_url("http://a.test/a/b/", "./c/../d").as_deref(),
            Some("http://a.test/a/b/d")
        );
    }

    #[test]
    fn resolve_rejects_a_non_absolute_base() {
        // The base must be an absolute URI; a bare relative base can't anchor resolution.
        assert_eq!(resolve_url("not-a-url", "/x"), None);
        assert_eq!(resolve_url("/only/a/path", "x"), None);
    }

    #[test]
    fn resolve_tolerates_a_fragment_in_the_base() {
        // RFC 3986 §5.1: the base's fragment is not used in resolution. It must not break parsing.
        assert_eq!(
            resolve_url("http://a.test/one/two#frag", "three").as_deref(),
            Some("http://a.test/one/three")
        );
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let r = Response {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html".into())],
            body: Vec::new(),
            final_url: "http://a.test".into(),
            num_redirects: 0,
        };
        assert_eq!(r.header("content-type"), Some("text/html"));
        assert_eq!(r.header("CONTENT-TYPE"), Some("text/html"));
        assert_eq!(r.header("x-missing"), None);
    }
}

/// End-to-end transport tests against a hermetic localhost server (native only). The mock serves a
/// SCRIPTED sequence of responses to successive requests, so a redirect chain can be exercised
/// without the internet.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod transport_tests {
    // Test code; see the note on `mod tests` above.
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::fmt::Write as _;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One scripted response: status, extra headers, body.
    type Reply = (u16, Vec<(String, String)>, String);

    /// Bind an ephemeral localhost server that answers `replies` to successive requests in order,
    /// then closes. Returns the `http://127.0.0.1:<port>` base. `${BASE}` in a header value is
    /// replaced with that base (so a `Location` can point back at the mock).
    fn scripted_server(replies: Vec<Reply>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let base_for_thread = base.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf); // drain request head
                let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
                for (k, v) in headers {
                    let v = v.replace("${BASE}", &base_for_thread);
                    let _ = write!(head, "{k}: {v}\r\n");
                }
                head.push_str("Connection: close\r\n\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        base
    }

    /// A server that OMITS `Content-Length` and streams until it is cut off — the shape that defeats
    /// a pre-check, so only a running bound during the read can stop it.
    fn lengthless_server(total: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let chunk = vec![b'x'; 64 * 1024];
                let mut sent = 0usize;
                while sent < total && stream.write_all(&chunk).is_ok() {
                    sent += chunk.len();
                }
                let _ = stream.flush();
            }
        });
        base
    }

    /// A server that echoes the request head it received as the response body, so a test can assert
    /// on which headers actually went out on a given hop.
    fn header_echo_server(replies: Vec<(u16, Vec<(String, String)>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let base_for_thread = base.clone();
        std::thread::spawn(move || {
            for (status, headers) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let body = String::from_utf8_lossy(&buf[..n]).into_owned();
                let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
                for (k, v) in headers {
                    let v = v.replace("${BASE}", &base_for_thread);
                    let _ = write!(head, "{k}: {v}\r\n");
                }
                head.push_str("Connection: close\r\n\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        base
    }

    #[tokio::test]
    async fn credentials_are_dropped_when_a_redirect_leaves_the_origin() {
        // Two DIFFERENT servers: the first 302s to the second, which echoes what it received.
        let victim = header_echo_server(vec![(200, vec![])]);
        let attacker = scripted_server(vec![reply(
            302,
            &[("Location", &format!("{victim}/collect"))],
            "go",
        )]);

        let mut req = Request::new(Method::GET, attacker);
        req.follow_redirects = true;
        req.headers = vec![
            ("Authorization".into(), "Basic c2VjcmV0".into()),
            ("Cookie".into(), "session=abc".into()),
            ("X-Harmless".into(), "keep-me".into()),
        ];
        let resp = fetch(&req).await.unwrap();
        let echoed = String::from_utf8_lossy(&resp.body).to_ascii_lowercase();

        assert!(
            !echoed.contains("c2VjcmV0".to_ascii_lowercase().as_str()),
            "the Basic credential must not reach the redirect target:\n{echoed}"
        );
        assert!(
            !echoed.contains("session=abc"),
            "the cookie must not reach the redirect target:\n{echoed}"
        );
        assert!(
            echoed.contains("keep-me"),
            "non-credential headers still travel:\n{echoed}"
        );
    }

    #[tokio::test]
    async fn credentials_survive_a_same_origin_redirect() {
        // Same server, so the same origin: this is the case curl -L is normally used for.
        let base = header_echo_server(vec![
            (302, vec![("Location".into(), "/next".into())]),
            (200, vec![]),
        ]);
        let mut req = Request::new(Method::GET, base);
        req.follow_redirects = true;
        req.headers = vec![("Authorization".into(), "Basic c2VjcmV0".into())];
        let resp = fetch(&req).await.unwrap();
        let echoed = String::from_utf8_lossy(&resp.body);
        assert!(
            echoed.contains("c2VjcmV0"),
            "a same-origin redirect must not strip credentials:\n{echoed}"
        );
    }

    #[tokio::test]
    async fn an_advertised_oversize_body_is_refused_before_it_is_read() {
        let base = scripted_server(vec![reply(200, &[], &"x".repeat(4096))]);
        let mut req = Request::new(Method::GET, base);
        req.max_body = 100;
        assert_eq!(fetch(&req).await.unwrap_err(), Error::BodyTooLarge(100));
    }

    #[tokio::test]
    async fn a_body_without_content_length_is_still_bounded_while_reading() {
        // Regression: the read used to be `.bytes()` (to EOF), so a peer that advertises no length
        // could commit unbounded memory before any check ran. The cap must hold during the read.
        let cap = 256 * 1024;
        let base = lengthless_server(8 * 1024 * 1024);
        let mut req = Request::new(Method::GET, base);
        req.max_body = cap;
        assert_eq!(fetch(&req).await.unwrap_err(), Error::BodyTooLarge(cap));
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &str) -> Reply {
        (
            status,
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body.to_string(),
        )
    }

    #[tokio::test]
    async fn does_not_follow_redirects_by_default() {
        let base = scripted_server(vec![reply(302, &[("Location", "${BASE}/next")], "go")]);
        let resp = fetch(&Request::new(Method::GET, base)).await.unwrap();
        assert_eq!(resp.status, 302);
        assert_eq!(
            resp.header("location").map(|l| l.ends_with("/next")),
            Some(true)
        );
    }

    #[tokio::test]
    async fn follows_a_redirect_chain_to_the_final_body() {
        let base = scripted_server(vec![
            reply(301, &[("Location", "${BASE}/a")], "one"),
            reply(302, &[("Location", "${BASE}/b")], "two"),
            reply(200, &[], "arrived"),
        ]);
        let mut req = Request::new(Method::GET, base.clone());
        req.follow_redirects = true;
        let resp = fetch(&req).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(String::from_utf8(resp.body).unwrap(), "arrived");
        assert!(
            resp.final_url.ends_with("/b"),
            "final_url tracks the last hop: {}",
            resp.final_url
        );
    }

    #[tokio::test]
    async fn too_many_redirects_is_an_error() {
        // A server that always redirects to itself.
        let base = scripted_server(vec![reply(302, &[("Location", "${BASE}/loop")], ""); 5]);
        let mut req = Request::new(Method::GET, base);
        req.follow_redirects = true;
        req.max_redirects = 2;
        assert_eq!(fetch(&req).await, Err(Error::TooManyRedirects(2)));
    }

    #[tokio::test]
    async fn response_headers_are_captured() {
        let base = scripted_server(vec![reply(200, &[("X-Custom", "yes")], "body")]);
        let resp = fetch(&Request::new(Method::GET, base)).await.unwrap();
        assert_eq!(resp.header("x-custom"), Some("yes"));
    }

    #[tokio::test]
    async fn head_returns_headers_but_no_body() {
        // The mock still puts bytes on the wire; a HEAD must not read them (returns empty body),
        // which is both correct and avoids half-consuming the connection.
        let base = scripted_server(vec![reply(
            200,
            &[("X-Custom", "yes")],
            "should-not-be-read",
        )]);
        let resp = fetch(&Request::new(Method::HEAD, base)).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-custom"), Some("yes"));
        assert!(
            resp.body.is_empty(),
            "HEAD body must be empty, got {:?}",
            resp.body
        );
    }

    #[tokio::test]
    async fn a_redirect_chain_reports_its_hop_count() {
        let base = scripted_server(vec![
            reply(301, &[("Location", "${BASE}/a")], ""),
            reply(302, &[("Location", "${BASE}/b")], ""),
            reply(200, &[], "arrived"),
        ]);
        let mut req = Request::new(Method::GET, base);
        req.follow_redirects = true;
        let resp = fetch(&req).await.unwrap();
        assert_eq!(resp.num_redirects, 2);
    }

    /// A one-shot server that answers with raw (non-UTF-8-safe) `body` bytes — for gzip/deflate
    /// payloads, which `scripted_server`'s `String` body can't carry.
    fn binary_server(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
                for (k, v) in &headers {
                    let _ = write!(head, "{k}: {v}\r\n");
                }
                head.push_str("Connection: close\r\n\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn gzip(text: &str) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    fn zlib(text: &str) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[tokio::test]
    async fn compressed_gzip_response_is_decoded_when_requested() {
        let base = binary_server(200, &[("Content-Encoding", "gzip")], gzip("hello, gzip"));
        let mut req = Request::new(Method::GET, base);
        req.compressed = true;
        let resp = fetch(&req).await.unwrap();
        // curl still reports the encoding the server actually sent, even though the body handed
        // back is already decoded.
        assert_eq!(resp.header("content-encoding"), Some("gzip"));
        assert_eq!(String::from_utf8(resp.body).unwrap(), "hello, gzip");
    }

    #[tokio::test]
    async fn compressed_deflate_zlib_response_is_decoded_when_requested() {
        let base = binary_server(200, &[("Content-Encoding", "deflate")], zlib("hi, deflate"));
        let mut req = Request::new(Method::GET, base);
        req.compressed = true;
        let resp = fetch(&req).await.unwrap();
        assert_eq!(String::from_utf8(resp.body).unwrap(), "hi, deflate");
    }

    // a compressed body can be many times its wire size, so decoding it must respect
    // `max_body` too — not just the still-compressed body already checked against it — or a
    // small-on-the-wire decompression bomb grows memory without bound.
    #[tokio::test]
    async fn a_compressed_body_over_max_body_when_decoded_is_refused() {
        // "aaaa...a" compresses far smaller than it decodes to.
        let payload = gzip(&"a".repeat(10_000));
        let base = binary_server(200, &[("Content-Encoding", "gzip")], payload);
        let mut req = Request::new(Method::GET, base);
        req.compressed = true;
        req.max_body = 100;
        let error = fetch(&req).await.unwrap_err();
        assert_eq!(error, Error::BodyTooLarge(100));
    }

    #[tokio::test]
    async fn without_compressed_the_body_is_left_encoded() {
        let raw = gzip("untouched");
        let base = binary_server(200, &[("Content-Encoding", "gzip")], raw.clone());
        // req.compressed defaults to false.
        let resp = fetch(&Request::new(Method::GET, base)).await.unwrap();
        assert_eq!(
            resp.body, raw,
            "body must stay encoded without --compressed"
        );
    }

    #[tokio::test]
    async fn compressed_sends_accept_encoding() {
        let base = header_echo_server(vec![(200, vec![])]);
        let mut req = Request::new(Method::GET, base);
        req.compressed = true;
        let resp = fetch(&req).await.unwrap();
        let echoed = String::from_utf8_lossy(&resp.body).to_ascii_lowercase();
        assert!(
            echoed.contains("accept-encoding: gzip, deflate"),
            "compressed must ask for gzip/deflate:\n{echoed}"
        );
    }

    #[tokio::test]
    async fn fetch_streaming_reads_the_body_one_chunk_at_a_time() {
        let base = scripted_server(vec![reply(200, &[], "streamed-body")]);
        let resp = fetch_streaming(&Request::new(Method::GET, base))
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        let mut body = resp.body;
        let mut collected = Vec::new();
        while let Some(chunk) = body.next_chunk().await.unwrap() {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(String::from_utf8(collected).unwrap(), "streamed-body");
    }

    #[tokio::test]
    async fn fetch_streaming_follows_redirects_to_the_final_body() {
        let base = scripted_server(vec![
            reply(302, &[("Location", "${BASE}/next")], ""),
            reply(200, &[], "final-streamed"),
        ]);
        let mut req = Request::new(Method::GET, base);
        req.follow_redirects = true;
        let mut resp = fetch_streaming(&req).await.unwrap();
        assert_eq!(resp.num_redirects, 1);
        let mut collected = Vec::new();
        while let Some(chunk) = resp.body.next_chunk().await.unwrap() {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(String::from_utf8(collected).unwrap(), "final-streamed");
    }

    #[tokio::test]
    async fn fetch_streaming_head_has_an_empty_body() {
        let base = scripted_server(vec![reply(200, &[], "should-not-be-read")]);
        let mut resp = fetch_streaming(&Request::new(Method::HEAD, base))
            .await
            .unwrap();
        assert_eq!(resp.body.next_chunk().await.unwrap(), None);
    }
}
