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

//! An in-process TCP relay that injects network drops between two endpoints.
//!
//! Clients connect to [`TcpRelay::local_addr`] instead of the target. Every accepted
//! connection is forwarded to the target until [`TcpRelay::drop_connections`] resets it.
//! A reset closes both sockets with `SO_LINGER` zero, so each side observes a connection
//! reset instead of a stalled connection or a clean end. The listener keeps accepting, so a
//! client that reconnects after a drop reaches the target again.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

pub struct TcpRelay {
    local_addr: SocketAddr,
    target: String,
    state: Arc<RelayState>,
    accept_task: JoinHandle<()>,
}

#[derive(Default)]
struct RelayState {
    next_id: AtomicU64,
    accepted: AtomicU64,
    resets: AtomicU64,
    connections: Mutex<HashMap<u64, Connection>>,
}

struct Connection {
    reset: oneshot::Sender<()>,
    task: JoinHandle<bool>,
}

impl TcpRelay {
    /// Listens on a free loopback port and forwards each connection to `target`, given as
    /// `host:port`. The target is resolved per connection, so it may be restarted on the
    /// same port while the relay keeps running.
    pub async fn start(target: impl Into<String>) -> std::io::Result<Self> {
        let target = target.into();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let state = Arc::new(RelayState::default());
        let accept_task = tokio::spawn(accept_loop(listener, target.clone(), state.clone()));
        debug!("TCP relay {local_addr} forwards to {target}");
        Ok(Self {
            local_addr,
            target,
            state,
            accept_task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    /// Number of client connections the relay has accepted so far.
    pub fn accepted_count(&self) -> u64 {
        self.state.accepted.load(Ordering::Acquire)
    }

    /// Number of open connections that [`Self::drop_connections`] has reset so far.
    pub fn reset_count(&self) -> u64 {
        self.state.resets.load(Ordering::Acquire)
    }

    /// Resets every connection that is open through the relay and returns how many it reset.
    /// Returns after both sockets of each reset connection are closed. New connections are
    /// accepted and forwarded as before.
    pub async fn drop_connections(&self) -> u64 {
        let connections: Vec<Connection> = self
            .state
            .connections
            .lock()
            .unwrap()
            .drain()
            .map(|(_, connection)| connection)
            .collect();
        let mut reset = 0;
        for connection in connections {
            let _ = connection.reset.send(());
            if connection.task.await.unwrap_or(false) {
                reset += 1;
            }
        }
        self.state.resets.fetch_add(reset, Ordering::AcqRel);
        debug!("TCP relay {} reset {reset} connection(s)", self.local_addr);
        reset
    }
}

impl Drop for TcpRelay {
    fn drop(&mut self) {
        self.accept_task.abort();
        for (_, connection) in self.state.connections.lock().unwrap().drain() {
            connection.task.abort();
        }
    }
}

async fn accept_loop(listener: TcpListener, target: String, state: Arc<RelayState>) {
    loop {
        let (client, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                warn!("TCP relay accept failed: {error}");
                continue;
            }
        };
        state.accepted.fetch_add(1, Ordering::AcqRel);
        let id = state.next_id.fetch_add(1, Ordering::AcqRel);
        let (reset, reset_rx) = oneshot::channel();
        // The registry lock is held across the spawn, so the task cannot remove its own entry
        // before the entry exists.
        let mut connections = state.connections.lock().unwrap();
        let task = tokio::spawn(forward(id, client, target.clone(), reset_rx, state.clone()));
        connections.insert(id, Connection { reset, task });
    }
}

/// Returns true when the connection was reset by the relay, false when it ended by itself.
async fn forward(
    id: u64,
    mut client: TcpStream,
    target: String,
    reset: oneshot::Receiver<()>,
    state: Arc<RelayState>,
) -> bool {
    let mut upstream = match TcpStream::connect(&target).await {
        Ok(upstream) => upstream,
        Err(error) => {
            debug!("TCP relay could not reach {target}: {error}");
            let _ = client.set_zero_linger();
            state.connections.lock().unwrap().remove(&id);
            return false;
        }
    };
    let was_reset = tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => false,
        _ = reset => true,
    };
    if was_reset {
        let _ = client.set_zero_linger();
        let _ = upstream.set_zero_linger();
    } else {
        state.connections.lock().unwrap().remove(&id);
    }
    was_reset
}

#[cfg(test)]
mod tests {
    use super::TcpRelay;
    use std::time::Duration;
    use test_r::test;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn echo_server() -> TcpListener {
        TcpListener::bind("127.0.0.1:0").await.unwrap()
    }

    fn serve_echo(listener: TcpListener) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    loop {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => {
                                if socket.write_all(&buffer[..read]).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        })
    }

    async fn round_trip(stream: &mut TcpStream, payload: &[u8]) -> std::io::Result<Vec<u8>> {
        stream.write_all(payload).await?;
        let mut received = vec![0u8; payload.len()];
        stream.read_exact(&mut received).await?;
        Ok(received)
    }

    #[test]
    async fn drop_resets_every_open_connection_and_accepts_new_ones() {
        let listener = echo_server().await;
        let target = listener.local_addr().unwrap().to_string();
        let _server = serve_echo(listener);
        let relay = TcpRelay::start(target).await.unwrap();

        let mut first = TcpStream::connect(relay.local_addr()).await.unwrap();
        let mut second = TcpStream::connect(relay.local_addr()).await.unwrap();
        assert_eq!(round_trip(&mut first, b"one").await.unwrap(), b"one");
        assert_eq!(round_trip(&mut second, b"two").await.unwrap(), b"two");

        assert_eq!(relay.drop_connections().await, 2);
        assert_eq!(relay.reset_count(), 2);

        // Each client observes the drop promptly: a read fails or ends, it does not hang.
        for stream in [&mut first, &mut second] {
            let mut buffer = [0u8; 8];
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                .await
                .expect("client hung after the relay dropped its connection");
            assert!(
                matches!(read, Ok(0) | Err(_)),
                "dropped connection still delivered data: {read:?}"
            );
        }

        let mut third = TcpStream::connect(relay.local_addr()).await.unwrap();
        assert_eq!(round_trip(&mut third, b"three").await.unwrap(), b"three");
        assert_eq!(relay.accepted_count(), 3);

        assert_eq!(relay.drop_connections().await, 1);
        assert_eq!(relay.reset_count(), 3);
    }

    #[test]
    async fn drop_does_not_count_connections_that_already_ended() {
        let listener = echo_server().await;
        let target = listener.local_addr().unwrap().to_string();
        let _server = serve_echo(listener);
        let relay = TcpRelay::start(target).await.unwrap();

        let mut closed = TcpStream::connect(relay.local_addr()).await.unwrap();
        assert_eq!(round_trip(&mut closed, b"x").await.unwrap(), b"x");
        drop(closed);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !relay.state.connections.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("relay kept a closed connection open");

        assert_eq!(relay.drop_connections().await, 0);
        assert_eq!(relay.reset_count(), 0);
    }

    #[test]
    async fn relays_to_a_target_that_restarts_on_the_same_port() {
        let listener = echo_server().await;
        let address = listener.local_addr().unwrap();
        let server = serve_echo(listener);
        let relay = TcpRelay::start(address.to_string()).await.unwrap();

        let mut before = TcpStream::connect(relay.local_addr()).await.unwrap();
        assert_eq!(round_trip(&mut before, b"a").await.unwrap(), b"a");
        server.abort();
        let _ = server.await;
        relay.drop_connections().await;

        let _restarted = serve_echo(TcpListener::bind(address).await.unwrap());
        let mut after = TcpStream::connect(relay.local_addr()).await.unwrap();
        assert_eq!(round_trip(&mut after, b"b").await.unwrap(), b"b");
    }
}
