// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use poem::web::{LocalAddr, RemoteAddr};
use poem::{Endpoint, Request};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tracing::{Instrument, warn};

/// Serve the existing Poem endpoint with one owner per HTTP/1 or h2c connection.
/// A completed connection is never polled again: a body error must remain an abort,
/// not be followed by a graceful shutdown that can emit a successful chunked EOF.
pub(crate) async fn serve<E: Endpoint + 'static>(
    listener: TcpListener,
    endpoint: E,
) -> std::io::Result<()> {
    let local = listener.local_addr()?;
    let endpoint = Arc::new(endpoint);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            incoming = listener.accept() => {
                let (socket, remote) = match incoming {
                    Ok(connection) => connection,
                    Err(error) => {
                        warn!(error = %error, "HTTP gateway accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let endpoint = endpoint.clone();
                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        let endpoint = endpoint.clone();
                        async move {
                            let request = Request::from((request, LocalAddr(local.into()), RemoteAddr(remote.into()), http::uri::Scheme::HTTP));
                            let response: hyper::Response<_> = endpoint.get_response(request).await.into();
                            Ok::<_, Infallible>(response)
                        }
                    });
                    let mut builder = Builder::new(TokioExecutor::new());
                    builder.http2().max_pending_accept_reset_streams(Some(20)).max_header_list_size(16_384);
                    let _ = builder.serve_connection_with_upgrades(TokioIo::new(socket), service).await;
                }.in_current_span());
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed {
                    warn!(error = %error, "HTTP gateway connection task failed");
                }
            }
        }
    }
}
