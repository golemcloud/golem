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

use anyhow::Context;
use axum::Router;
use tokio::net::TcpListener;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

/// An in-process HTTP stub that a test component calls. Dropping it stops the server.
pub struct LocalHttpServer {
    port: u16,
    _task: AbortOnDropHandle<()>,
}

impl LocalHttpServer {
    /// Binds `127.0.0.1:0` and serves `router` in the current tracing span.
    pub async fn serve(router: Router) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind local HTTP server")?;
        let port = listener.local_addr()?.port();
        let task = AbortOnDropHandle::new(tokio::spawn(
            async move {
                axum::serve(listener, router)
                    .await
                    .expect("serve local HTTP server");
            }
            .in_current_span(),
        ));
        Ok(Self { port, _task: task })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://127.0.0.1:{port}{path}`
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}
