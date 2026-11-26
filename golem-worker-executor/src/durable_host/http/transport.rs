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

//! HTTP sends with separate response-body and I/O results.

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::BodyExt;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use wasmtime_wasi_http::{Error, HttpConnectionPool, PooledConnection, RequestOptions, WasiBody};

#[cfg(test)]
mod tests;

/// An owned I/O result, separate from the response body.
pub type TransportIo = Box<dyn Future<Output = Result<(), Error>> + Send>;

/// The response, its I/O future, and the connection that can be invalidated.
pub type TransportResult = Result<
    (
        http::Response<WasiBody>,
        TransportIo,
        Option<PooledConnection>,
    ),
    Error,
>;

/// Sends an owned request and preserves separate send, response-body, and I/O errors.
///
/// A response can arrive before request transmission ends. The response body and the I/O
/// future can require concurrent polling. The I/O future of a pooled send reports request
/// transmission; the default sender's future drives the connection too.
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(
        &self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> TransportResult;
}

/// Sends through the shared pool, or through the default WASI HTTP sender.
#[derive(Clone)]
pub struct WasiHttpTransport {
    pool: Option<HttpConnectionPool>,
}

impl WasiHttpTransport {
    pub fn new(pool: Option<HttpConnectionPool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HttpTransport for WasiHttpTransport {
    async fn send(
        &self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> TransportResult {
        let pool = &self.pool;
        match pool {
            Some(pool) => pool
                .send_request(request, options)
                .await
                .map(|(response, io, connection)| (response, io, Some(connection))),
            None => match wasmtime_wasi_http::default_send_request(request, options).await {
                Ok((response, io)) => Ok((
                    response.map(http_body_util::BodyExt::boxed_unsync),
                    Box::new(io)
                        as Box<dyn Future<Output = Result<(), wasmtime_wasi_http::Error>> + Send>,
                    None,
                )),
                Err(error) => Err(error),
            },
        }
    }
}

/// When a scripted response consumes the request body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestBodyTiming {
    /// Read the request body before returning the response.
    BeforeResponse,
    /// Return the response first; read the request body while polling the I/O future.
    WithIo,
}

/// One outcome in a finite in-memory send script.
pub struct ScriptedHttpExchange {
    outcome: ScriptedOutcome,
}

enum ScriptedOutcome {
    SendError(Box<Error>),
    SendErrorAfterRequestBody(Box<Error>),
    Response(Box<ScriptedResponse>),
}

struct ScriptedResponse {
    response: http::Response<WasiBody>,
    request_body: RequestBodyTiming,
    io: TransportIo,
}

impl ScriptedHttpExchange {
    /// Fail without polling the request body.
    pub fn send_error(error: Error) -> Self {
        Self {
            outcome: ScriptedOutcome::SendError(Box::new(error)),
        }
    }

    /// Read the complete request body, then fail before returning a response.
    /// A request-body error takes precedence over the scripted send error.
    pub fn send_error_after_request_body(error: Error) -> Self {
        Self {
            outcome: ScriptedOutcome::SendErrorAfterRequestBody(Box::new(error)),
        }
    }

    /// Return the response and preserve its body errors and the separate I/O result.
    pub fn response(
        response: http::Response<WasiBody>,
        request_body: RequestBodyTiming,
        io: TransportIo,
    ) -> Self {
        Self {
            outcome: ScriptedOutcome::Response(Box::new(ScriptedResponse {
                response,
                request_body,
                io,
            })),
        }
    }
}

/// An immutable snapshot of request-body intake.
#[derive(Clone, Debug)]
pub enum CapturedRequestBody {
    /// No body read has started.
    Unpolled,
    /// A read started, but no terminal result has been captured.
    Reading,
    /// The complete body and trailers.
    Complete(Arc<CapturedBody>),
    /// The request body returned an error.
    Failed,
}

/// The complete bytes and trailers of one request body.
#[derive(Debug)]
pub struct CapturedBody {
    pub bytes: Bytes,
    pub trailers: Option<http::HeaderMap>,
}

/// An owned request snapshot and its unmodified send options.
pub struct CapturedRequest {
    pub request: http::Request<Arc<CapturedRequestBody>>,
    pub options: Option<RequestOptions>,
}

enum CaptureEvent {
    Start,
    Complete(Arc<CapturedBody>),
    Fail,
}

fn transition(phase: &CapturedRequestBody, event: CaptureEvent) -> CapturedRequestBody {
    match (phase, event) {
        (CapturedRequestBody::Unpolled, CaptureEvent::Start) => CapturedRequestBody::Reading,
        (CapturedRequestBody::Reading, CaptureEvent::Complete(body)) => {
            CapturedRequestBody::Complete(body)
        }
        (CapturedRequestBody::Reading, CaptureEvent::Fail) => CapturedRequestBody::Failed,
        _ => unreachable!("invalid HTTP request capture transition"),
    }
}

struct RequestCapture {
    head: http::request::Parts,
    options: Option<RequestOptions>,
    body: Mutex<Arc<CapturedRequestBody>>,
}

fn apply(capture: &RequestCapture, event: CaptureEvent) {
    let mut phase = capture.body.lock().unwrap();
    *phase = Arc::new(transition(&phase, event));
}

async fn read_request(body: WasiBody, capture: Arc<RequestCapture>) -> Result<(), Error> {
    apply(&capture, CaptureEvent::Start);
    match body.collect().await {
        Ok(body) => {
            let trailers = body.trailers().cloned();
            apply(
                &capture,
                CaptureEvent::Complete(Arc::new(CapturedBody {
                    bytes: body.to_bytes(),
                    trailers,
                })),
            );
            Ok(())
        }
        Err(error) => {
            apply(&capture, CaptureEvent::Fail);
            Err(error)
        }
    }
}

struct MemoryState {
    exchanges: VecDeque<ScriptedHttpExchange>,
    requests: Vec<Arc<RequestCapture>>,
}

/// A finite send script with immutable request snapshots and no network connection.
///
/// Script exhaustion returns `InternalError` without polling the request body. No pooled
/// connection is returned. The response body and the scripted I/O future remain independent.
pub struct InMemoryHttpTransport {
    state: Mutex<MemoryState>,
}

impl InMemoryHttpTransport {
    pub fn new(exchanges: impl IntoIterator<Item = ScriptedHttpExchange>) -> Self {
        Self {
            state: Mutex::new(MemoryState {
                exchanges: exchanges.into_iter().collect(),
                requests: Vec::new(),
            }),
        }
    }

    /// Take the requests in send order, with the body state at the time of this call.
    ///
    /// Later I/O cannot change these snapshots. A later call returns only new sends.
    pub fn take_requests(&self) -> Box<[CapturedRequest]> {
        let requests = std::mem::take(&mut self.state.lock().unwrap().requests);
        requests
            .into_iter()
            .map(|capture| CapturedRequest {
                request: http::Request::from_parts(
                    capture.head.clone(),
                    capture.body.lock().unwrap().clone(),
                ),
                options: capture.options,
            })
            .collect()
    }
}

#[async_trait]
impl HttpTransport for InMemoryHttpTransport {
    async fn send(
        &self,
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> TransportResult {
        let (head, body) = request.into_parts();
        let capture = Arc::new(RequestCapture {
            head,
            options,
            body: Mutex::new(Arc::new(CapturedRequestBody::Unpolled)),
        });
        let exchange = {
            let mut state = self.state.lock().unwrap();
            state.requests.push(capture.clone());
            state.exchanges.pop_front()
        };
        let exchange = exchange.ok_or_else(|| {
            Error::InternalError(Some("HTTP transport script exhausted".to_owned()))
        })?;
        match exchange.outcome {
            ScriptedOutcome::SendError(error) => Err(*error),
            ScriptedOutcome::SendErrorAfterRequestBody(error) => {
                read_request(body, capture).await?;
                Err(*error)
            }
            ScriptedOutcome::Response(response) => {
                let ScriptedResponse {
                    response,
                    request_body,
                    io,
                } = *response;
                let io = match request_body {
                    RequestBodyTiming::BeforeResponse => {
                        read_request(body, capture).await?;
                        io
                    }
                    RequestBodyTiming::WithIo => Box::new(async move {
                        futures::try_join!(read_request(body, capture), Box::into_pin(io))?;
                        Ok(())
                    }) as TransportIo,
                };
                Ok((response, io, None))
            }
        }
    }
}
