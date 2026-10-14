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

//! A TCP proxy in front of etcd that a test can break in the two ways that matter to a keepalive:
//! connections dropped, as a member restart does, and traffic black-holed with no error and no
//! close. Neither fault can be provoked through the etcd API.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;

/// How closely a parked pump watches for the black hole being lifted. Kept well inside the tests'
/// request timeouts, so they measure the fault rather than this.
const BLACK_HOLE_POLL: Duration = Duration::from_millis(20);

pub struct BreakableProxy {
    address: SocketAddr,
    black_holed: Arc<AtomicBool>,
    /// One handle per direction of every connection. Aborting a task closes its sockets, since it
    /// owns their halves - that is how the peers see a close.
    pumps: Arc<Mutex<Vec<JoinHandle<()>>>>,
    accept: JoinHandle<()>,
}

impl BreakableProxy {
    /// Starts a proxy on an ephemeral loopback port, forwarding to `upstream` - an
    /// `http://host:port` URL, the shape `DockerEtcd::client_url` returns.
    pub async fn start(upstream: &str) -> Self {
        let upstream: SocketAddr = upstream
            .trim_start_matches("http://")
            .parse()
            .unwrap_or_else(|err| panic!("Cannot parse the upstream address `{upstream}`: {err}"));

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("Binding the test proxy should succeed");
        let address = listener
            .local_addr()
            .expect("The test proxy should have a local address");

        let black_holed = Arc::new(AtomicBool::new(false));
        let pumps: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let accept = tokio::spawn({
            let black_holed = black_holed.clone();
            let pumps = pumps.clone();
            async move {
                loop {
                    let Ok((client, _)) = listener.accept().await else {
                        return;
                    };
                    let Ok(server) = TcpStream::connect(upstream).await else {
                        return;
                    };

                    let (client_read, client_write) = client.into_split();
                    let (server_read, server_write) = server.into_split();

                    let mut handles = pumps.lock().await;
                    handles.push(tokio::spawn(pump(
                        client_read,
                        server_write,
                        black_holed.clone(),
                    )));
                    handles.push(tokio::spawn(pump(
                        server_read,
                        client_write,
                        black_holed.clone(),
                    )));
                }
            }
        });

        info!(%address, %upstream, "Test etcd proxy started");
        Self {
            address,
            black_holed,
            pumps,
            accept,
        }
    }

    /// The endpoint to point an `EtcdConfig` at.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Closes every connection currently proxied, as an etcd member going away does.
    ///
    /// New connections are still accepted: the lease is untouched on the server, so a client that
    /// reconnects still holds it.
    pub async fn drop_all_connections(&self) {
        let mut handles = self.pumps.lock().await;
        for handle in handles.drain(..) {
            handle.abort();
        }
        info!("Test etcd proxy dropped all connections");
    }

    /// Stops forwarding bytes in either direction, without ever closing a socket.
    ///
    /// Applies to connections opened afterwards too, so a client cannot escape it by reconnecting.
    /// Lifted only by [`Self::restore`].
    pub fn black_hole(&self) {
        self.black_holed.store(true, Ordering::SeqCst);
        info!("Test etcd proxy is now black-holing traffic");
    }

    /// Forwards bytes again, as an etcd that was slow rather than gone does.
    ///
    /// Connections held open throughout recover delayed rather than broken: bytes a pump was
    /// holding are delivered now, even for requests the client has already given up on.
    pub fn restore(&self) {
        self.black_holed.store(false, Ordering::SeqCst);
        info!("Test etcd proxy is forwarding traffic again");
    }
}

impl Drop for BreakableProxy {
    fn drop(&mut self) {
        self.accept.abort();
        if let Ok(mut handles) = self.pumps.try_lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
    }
}

async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, black_holed: Arc<AtomicBool>) {
    let mut buffer = vec![0u8; 16 * 1024];

    loop {
        // Checked on both sides of the read: a black hole declared while parked in `read` must
        // hold the bytes that woke it rather than forward them.
        park_while_black_holed(&black_holed).await;
        let Ok(read) = from.read(&mut buffer).await else {
            return;
        };
        if read == 0 {
            return;
        }
        park_while_black_holed(&black_holed).await;

        if to.write_all(&buffer[..read]).await.is_err() {
            return;
        }
    }
}

/// Parks for as long as the black hole lasts, holding both socket halves open and unread.
///
/// Returning would drop them and close the connections, which is the *other* fault. Polled rather
/// than notified because an unlifted black hole parks here for the life of the test either way.
async fn park_while_black_holed(black_holed: &AtomicBool) {
    while black_holed.load(Ordering::SeqCst) {
        tokio::time::sleep(BLACK_HOLE_POLL).await;
    }
}
