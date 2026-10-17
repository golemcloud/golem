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

use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

pub const INTERVAL: Duration = Duration::from_millis(200);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(1);
pub const SILENCE: Duration = Duration::from_secs(3);

pub async fn server(
    endpoint: impl poem::Endpoint<Output = poem::Response> + 'static,
) -> (SocketAddr, AbortOnDropHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
    let task = tokio::spawn(async move {
        poem::Server::new_with_acceptor(acceptor)
            .run(endpoint)
            .await
    });
    (address, AbortOnDropHandle::new(task))
}

/// A TCP proxy that closes both halves after an interval without traffic in either direction.
pub async fn idle_proxy(upstream: SocketAddr) -> (SocketAddr, AbortOnDropHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut client, _) = accepted.unwrap();
                    connections.spawn(async move {
                        let mut server = TcpStream::connect(upstream).await.unwrap();
                        let mut from_client = [0; 8192];
                        let mut from_server = [0; 8192];
                        loop {
                            tokio::select! {
                                _ = tokio::time::sleep(IDLE_TIMEOUT) => return,
                                read = client.read(&mut from_client) => {
                                    match read {
                                        Ok(0) | Err(_) => return,
                                        Ok(len) => if server.write_all(&from_client[..len]).await.is_err() {
                                            return;
                                        },
                                    }
                                }
                                read = server.read(&mut from_server) => {
                                    match read {
                                        Ok(0) | Err(_) => return,
                                        Ok(len) => if client.write_all(&from_server[..len]).await.is_err() {
                                            return;
                                        },
                                    }
                                }
                            }
                        }
                    });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    });
    (address, AbortOnDropHandle::new(task))
}
