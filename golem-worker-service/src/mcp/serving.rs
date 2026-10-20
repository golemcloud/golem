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

//! What a request of the MCP endpoint is served with. The layers in front of rmcp put one
//! [`McpServing`] in the request extensions; the handler serves every request through it and
//! never needs to know which kind it is.

use async_trait::async_trait;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::mcp::CompiledMcp;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait McpServing: Send + Sync {
    /// The deployment to serve a request of this kind with. `ct` is the request's cancellation
    /// token; [`Serving::while_running`] must be polled next to the request's work.
    async fn serve(&self, kind: ServeKind, ct: &CancellationToken)
    -> Result<Serving, ServeRefusal>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeKind {
    Initialize,
    /// `tools/list`, `resources/list`, `resources/templates/list` and `prompts/list`.
    List,
    /// `tools/call`, `resources/read` and `prompts/get`.
    Invoke,
}

pub struct Serving {
    pub deployment: Arc<CompiledMcp>,
    /// Work that runs next to the request. It never completes; it may cancel the request's
    /// cancellation token.
    pub while_running: BoxFuture<'static, Infallible>,
}

/// Why a request is not served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeRefusal {
    /// The client must list again: the tool set it listed is not the one that would serve it.
    ToolSetChanged,
    /// The request's session has ended.
    Gone,
    /// What the request needs could not be read.
    Unavailable,
}

/// Serves every request with the deployment its authentication was checked against.
pub struct StatelessServing(pub Arc<CompiledMcp>);

#[async_trait]
impl McpServing for StatelessServing {
    async fn serve(
        &self,
        _kind: ServeKind,
        _ct: &CancellationToken,
    ) -> Result<Serving, ServeRefusal> {
        Ok(Serving {
            deployment: self.0.clone(),
            while_running: std::future::pending().boxed(),
        })
    }
}
