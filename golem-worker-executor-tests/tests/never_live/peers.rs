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

//! Test peers that count the attempts that reach them.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::task::AbortOnDropHandle;

/// A TCP peer on `127.0.0.1` that counts its connections. An answering peer replies to each
/// request with an empty `200 OK`; a silent peer reads and never answers.
pub(crate) struct TcpPeer {
    pub(crate) port: u16,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDropHandle<()>,
}

impl TcpPeer {
    pub(crate) async fn answering() -> anyhow::Result<Self> {
        Self::start(true).await
    }

    pub(crate) async fn silent() -> anyhow::Result<Self> {
        Self::start(false).await
    }

    async fn start(answer: bool) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let connections = Arc::new(AtomicUsize::new(0));
        let task = {
            let connections = connections.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    connections.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let mut buffer = [0u8; 4096];
                        let mut received = Vec::new();
                        loop {
                            match stream.read(&mut buffer).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    received.extend_from_slice(&buffer[..n]);
                                    if answer && received.windows(4).any(|w| w == b"\r\n\r\n") {
                                        let _ = stream
                                            .write_all(
                                                b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n",
                                            )
                                            .await;
                                        received.clear();
                                    }
                                }
                            }
                        }
                    });
                }
            })
        };
        Ok(Self {
            port,
            connections,
            _task: AbortOnDropHandle::new(task),
        })
    }

    pub(crate) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Waits until the peer accepted `count` connections.
    pub(crate) async fn wait_for(&self, count: usize) -> anyhow::Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while self.connections() < count {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }
}

/// A UDP peer on `127.0.0.1` that counts the datagrams it receives.
pub(crate) struct UdpPeer {
    pub(crate) port: u16,
    datagrams: Arc<AtomicUsize>,
    _task: AbortOnDropHandle<()>,
}

impl UdpPeer {
    pub(crate) async fn start() -> anyhow::Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let port = socket.local_addr()?.port();
        let datagrams = Arc::new(AtomicUsize::new(0));
        let task = {
            let datagrams = datagrams.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                while socket.recv_from(&mut buffer).await.is_ok() {
                    datagrams.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        Ok(Self {
            port,
            datagrams,
            _task: AbortOnDropHandle::new(task),
        })
    }

    pub(crate) fn datagrams(&self) -> usize {
        self.datagrams.load(Ordering::SeqCst)
    }
}
