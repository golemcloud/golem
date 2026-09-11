// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use poem::listener::Acceptor;
use poem::{Endpoint, Response};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::Instrument;

pub(super) async fn run<A, E>(mut acceptor: A, endpoint: E) -> std::io::Result<()>
where
    A: Acceptor,
    E: Endpoint<Output = Response> + 'static,
{
    let endpoint = Arc::new(endpoint);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = acceptor.accept() => {
                let Ok((socket, local_addr, remote_addr, scheme)) = accepted else {
                    continue;
                };
                let endpoint = endpoint.clone();
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
                        .max_header_list_size(16_384);
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
