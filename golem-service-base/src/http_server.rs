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

use golem_common::SafeDisplay;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use poem::listener::Acceptor;
use poem::{Endpoint, EndpointExt, IntoEndpoint};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::Instrument;

/// Largest HTTP/2 flow-control window (RFC 9113, section 6.9.1).
const MAX_HTTP2_WINDOW_SIZE: u32 = (1 << 31) - 1;

/// HTTP/2 settings of an HTTP server. HTTP/1.1 connections use hyper's defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpServerConfig {
    /// Largest header list, in bytes, that a request may send. A request with a larger header
    /// list gets `431 Request Header Fields Too Large`.
    pub http2_max_header_list_size: u32,
    /// Flow-control window, in bytes, that each request stream starts with.
    pub http2_initial_stream_window_size: u32,
    /// Flow-control window, in bytes, that each connection starts with.
    pub http2_initial_connection_window_size: u32,
    /// Sizes the windows from the measured bandwidth-delay product. When enabled, the initial
    /// window sizes have no effect.
    pub http2_adaptive_window: bool,
}

impl HttpServerConfig {
    pub fn validate(&self) -> Result<(), String> {
        for (name, size) in [
            (
                "http2_initial_stream_window_size",
                self.http2_initial_stream_window_size,
            ),
            (
                "http2_initial_connection_window_size",
                self.http2_initial_connection_window_size,
            ),
        ] {
            if size > MAX_HTTP2_WINDOW_SIZE {
                return Err(format!(
                    "{name} is {size}, but HTTP/2 allows at most {MAX_HTTP2_WINDOW_SIZE}"
                ));
            }
        }
        Ok(())
    }
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            // Above the 60 KiB that an Envoy gateway accepts, so the gateway decides.
            http2_max_header_list_size: 64 * 1024,
            http2_initial_stream_window_size: 1024 * 1024,
            http2_initial_connection_window_size: 1024 * 1024,
            http2_adaptive_window: false,
        }
    }
}

impl SafeDisplay for HttpServerConfig {
    fn to_safe_string(&self) -> String {
        format!(
            "HTTP/2 max header list size: {}, initial stream window size: {}, initial connection window size: {}, adaptive window: {}",
            self.http2_max_header_list_size,
            self.http2_initial_stream_window_size,
            self.http2_initial_connection_window_size,
            self.http2_adaptive_window,
        )
    }
}

/// Serves `endpoint` on every connection that `acceptor` accepts, with HTTP/1.1 (and upgrades)
/// or HTTP/2.
pub async fn run<A, E>(
    mut acceptor: A,
    endpoint: E,
    config: HttpServerConfig,
) -> std::io::Result<()>
where
    A: Acceptor,
    E: IntoEndpoint,
    E::Endpoint: 'static,
{
    let endpoint = Arc::new(endpoint.into_endpoint().map_to_response());
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = acceptor.accept() => {
                let Ok((socket, local_addr, remote_addr, scheme)) = accepted else {
                    continue;
                };
                let endpoint = endpoint.clone();
                let config = config.clone();
                connections.spawn(async move {
                    let service = hyper::service::service_fn(move |request: http::Request<Incoming>| {
                        let endpoint = endpoint.clone();
                        let local_addr = local_addr.clone();
                        let remote_addr = remote_addr.clone();
                        let scheme = scheme.clone();
                        async move {
                            Ok::<http::Response<_>, Infallible>(endpoint
                                .get_response((request, local_addr, remote_addr, scheme).into())
                                .await
                                .into())
                        }
                    });
                    let mut builder = auto::Builder::new(TokioExecutor::new());
                    builder.http2()
                        .max_concurrent_streams(None)
                        .max_pending_accept_reset_streams(Some(20))
                        .max_header_list_size(config.http2_max_header_list_size)
                        .initial_stream_window_size(config.http2_initial_stream_window_size)
                        .initial_connection_window_size(config.http2_initial_connection_window_size)
                        .adaptive_window(config.http2_adaptive_window);
                    // Drop the connection as soon as it completes, releasing idle response bodies.
                    // Only an external shutdown signal may initiate a separate graceful drain.
                    let _ = builder.serve_connection_with_upgrades(TokioIo::new(socket), service).await;
                }.in_current_span());
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

#[cfg(test)]
mod tests;
