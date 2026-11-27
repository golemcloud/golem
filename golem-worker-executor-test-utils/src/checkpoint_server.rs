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

//! In-process servers that test components call at named points of their execution.
//!
//! A component parks at a crash checkpoint: it announces the checkpoint name with
//! `POST /{name}` to the announcement port, then connects to the gate port and waits for one
//! byte. The test reaches the checkpoint with [`CrashCheckpointServer::reach`] and either releases
//! it, or keeps it shut and restarts the executor while the component waits inside the window.
//!
//! A component reports a promise checkpoint with `POST /{name}` and the oplog index of its
//! `create_promise` call as the body.

use anyhow::{Context, anyhow};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use golem_common::model::oplog::OplogIndex;
use golem_test_framework::dsl::SETTLE_TIMEOUT;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

/// The crash-checkpoint server of one test. Dropping it stops both listeners.
pub struct CrashCheckpointServer {
    port: u16,
    gate_port: u16,
    arrivals: mpsc::UnboundedReceiver<CrashCheckpoint>,
    _task: AbortOnDropHandle<()>,
}

/// A component that waits at a crash checkpoint. [`Self::release`] lets it continue. Dropping it
/// without a release closes the gate connection, so the component never continues past the
/// checkpoint; a test does this to crash inside the window.
#[derive(Debug)]
pub struct CrashCheckpoint {
    pub name: String,
    release: oneshot::Sender<()>,
}

impl CrashCheckpoint {
    /// Lets the component continue past the checkpoint.
    pub fn release(self) -> anyhow::Result<()> {
        self.release
            .send(())
            .map_err(|_| anyhow!("the gate of crash checkpoint `{}` was dropped", self.name))
    }
}

impl CrashCheckpointServer {
    /// Binds the announcement and gate listeners on `127.0.0.1:0` and starts serving them.
    pub async fn start() -> anyhow::Result<Self> {
        let announcement_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind crash checkpoint server")?;
        let port = announcement_listener.local_addr()?.port();
        let gate_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind crash checkpoint gate")?;
        let gate_port = gate_listener.local_addr()?.port();
        let (sender, arrivals) = mpsc::unbounded_channel();
        let current_name = Arc::new(RwLock::new(None));
        let app = Router::new()
            .route("/{name}", post(announce_crash_checkpoint))
            .with_state(current_name.clone());
        let task = AbortOnDropHandle::new(tokio::spawn(async move {
            let announcements = axum::serve(announcement_listener, app);
            let gates = serve_gates(gate_listener, current_name, sender);
            tokio::select! {
                result = announcements => result.expect("serve crash checkpoint announcements"),
                () = gates => unreachable!("crash checkpoint gate listener does not stop"),
            }
        }));
        Ok(Self {
            port,
            gate_port,
            arrivals,
            _task: task,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn gate_port(&self) -> u16 {
        self.gate_port
    }

    /// The agent environment that points a component at this server:
    /// `{prefix}_CRASH_CHECKPOINT_PORT` and `{prefix}_CRASH_CHECKPOINT_GATE_PORT`, or the names
    /// without a prefix for `""`.
    pub fn agent_env(&self, prefix: &str) -> HashMap<String, String> {
        HashMap::from([
            (
                env_name(prefix, "CRASH_CHECKPOINT_PORT"),
                self.port.to_string(),
            ),
            (
                env_name(prefix, "CRASH_CHECKPOINT_GATE_PORT"),
                self.gate_port.to_string(),
            ),
        ])
    }

    /// Waits up to [`SETTLE_TIMEOUT`] for the next component at a checkpoint, and asserts
    /// that it is at `name`.
    pub async fn reach(&mut self, name: &str) -> anyhow::Result<CrashCheckpoint> {
        let arrival = tokio::time::timeout(SETTLE_TIMEOUT, self.arrivals.recv())
            .await
            .map_err(|_| anyhow!("timed out waiting for `{name}` checkpoint"))?
            .ok_or_else(|| anyhow!("checkpoint server stopped before `{name}`"))?;
        assert_eq!(arrival.name, name);
        Ok(arrival)
    }

    /// The next component at a checkpoint, without a timeout or a name check.
    pub async fn recv(&mut self) -> Option<CrashCheckpoint> {
        self.arrivals.recv().await
    }

    /// A component already waiting at a checkpoint, if any.
    pub fn try_next(&mut self) -> Option<CrashCheckpoint> {
        self.arrivals.try_recv().ok()
    }
}

async fn announce_crash_checkpoint(
    Path(name): Path<String>,
    State(current_name): State<Arc<RwLock<Option<String>>>>,
) -> StatusCode {
    *current_name.write().await = Some(name);
    StatusCode::NO_CONTENT
}

async fn serve_gates(
    gate_listener: TcpListener,
    current_name: Arc<RwLock<Option<String>>>,
    arrivals: mpsc::UnboundedSender<CrashCheckpoint>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = gate_listener.accept() => {
                let (mut connection, _) = accepted.expect("accept crash checkpoint gate");
                let name = current_name
                    .read()
                    .await
                    .clone()
                    .expect("checkpoint name announced before gate connection");
                let arrivals = arrivals.clone();
                connections.spawn(async move {
                    eprintln!("crash checkpoint server received `{name}`");
                    let (release, wait) = oneshot::channel();
                    arrivals
                        .send(CrashCheckpoint { name, release })
                        .expect("record crash checkpoint arrival");
                    if wait.await.is_ok() {
                        connection
                            .write_all(&[1])
                            .await
                            .expect("release crash checkpoint gate");
                    }
                });
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                completed
                    .expect("crash checkpoint gate task exists")
                    .expect("serve crash checkpoint gate");
            }
        }
    }
}

/// The promise-checkpoint server of one test. Dropping it stops the listener.
pub struct PromiseCheckpointServer {
    port: u16,
    arrivals: mpsc::UnboundedReceiver<PromiseCheckpoint>,
    _task: AbortOnDropHandle<()>,
}

/// A promise checkpoint that a component reported: its name, and the oplog index of the
/// `create_promise` call that the component is about to wait on.
#[derive(Debug)]
pub struct PromiseCheckpoint {
    pub name: String,
    pub oplog_index: OplogIndex,
}

impl PromiseCheckpointServer {
    /// Binds the listener on `127.0.0.1:0` and starts serving it.
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind promise checkpoint server")?;
        let port = listener.local_addr()?.port();
        let (sender, arrivals) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/{name}", post(announce_promise_checkpoint))
            .with_state(sender);
        let task = AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve promise checkpoint announcements");
        }));
        Ok(Self {
            port,
            arrivals,
            _task: task,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The agent environment that points a component at this server:
    /// `{prefix}_PROMISE_CHECKPOINT_PORT`, or `PROMISE_CHECKPOINT_PORT` for `""`.
    pub fn agent_env(&self, prefix: &str) -> HashMap<String, String> {
        HashMap::from([(
            env_name(prefix, "PROMISE_CHECKPOINT_PORT"),
            self.port.to_string(),
        )])
    }

    /// Waits up to [`SETTLE_TIMEOUT`] for the next promise checkpoint, and asserts that it
    /// is `name`.
    pub async fn reach(&mut self, name: &str) -> anyhow::Result<PromiseCheckpoint> {
        let arrival = tokio::time::timeout(SETTLE_TIMEOUT, self.arrivals.recv())
            .await
            .map_err(|_| anyhow!("timed out waiting for `{name}` promise checkpoint"))?
            .ok_or_else(|| anyhow!("promise checkpoint server stopped before `{name}`"))?;
        assert_eq!(arrival.name, name);
        Ok(arrival)
    }

    /// The next promise checkpoint, without a timeout or a name check.
    pub async fn recv(&mut self) -> Option<PromiseCheckpoint> {
        self.arrivals.recv().await
    }

    /// A promise checkpoint already reported, if any.
    pub fn try_next(&mut self) -> Option<PromiseCheckpoint> {
        self.arrivals.try_recv().ok()
    }
}

async fn announce_promise_checkpoint(
    Path(name): Path<String>,
    State(arrivals): State<mpsc::UnboundedSender<PromiseCheckpoint>>,
    body: Bytes,
) -> StatusCode {
    let oplog_index = OplogIndex::from_u64(
        std::str::from_utf8(&body)
            .expect("promise checkpoint body is UTF-8")
            .parse()
            .expect("promise checkpoint body is an oplog index"),
    );
    arrivals
        .send(PromiseCheckpoint { name, oplog_index })
        .expect("record promise checkpoint arrival");
    StatusCode::NO_CONTENT
}

fn env_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}_{name}")
    }
}
