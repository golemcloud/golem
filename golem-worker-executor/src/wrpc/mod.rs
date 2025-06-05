// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::wrpc::bindings::exports::golem::services::executor::{
    ComponentId, ComponentVersion, ExecutorError, FileSystemNode, GetOplogIterator,
    InvocationContext, OplogEntry, OplogIndex, PluginInstallationId, PromiseId, RevertWorkerTarget,
    SearchOplogIterator, ShardId, TargetWorkerId, UpdateMode, Uuid, WitValue, Worker,
    WorkerAnyFilter, WorkerContext, WorkerId, WorkerMetadata, WorkerMetadataIterator,
};
use anyhow::Context;
use bytes::Bytes;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::error;
use wrpc_transport::{ResourceBorrow, ResourceOwn};

mod bindings {
    wit_bindgen_wrpc::generate!({
        path: "../wit",
        world: "golem-worker-executor",
        with: {
            "golem:services/executor@1.3.0": generate,
            "wasi:io/poll@0.2.3": generate,
            "wasi:clocks/monotonic-clock@0.2.3": generate,
            "wasi:clocks/wall-clock@0.2.3": generate,
            "golem:rpc/types@0.2.1": generate,
            "golem:api/host@1.1.7": generate,
            "golem:api/context@1.1.7": generate,
            "golem:api/oplog@1.1.7": generate,
        }
    });
}

#[derive(Clone, Copy)]
struct Server;

// TODO: generic context to make it easier to switch transports?

impl bindings::exports::golem::services::executor::HandlerGetOplogIterator<SocketAddr> for Server {
    async fn get_next(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<GetOplogIterator>,
    ) -> anyhow::Result<Option<Vec<OplogEntry>>> {
        todo!()
    }
}

impl bindings::exports::golem::services::executor::HandlerSearchOplogIterator<SocketAddr>
    for Server
{
    async fn get_next(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<SearchOplogIterator>,
    ) -> anyhow::Result<Option<Vec<(OplogIndex, OplogEntry)>>> {
        todo!()
    }
}

impl bindings::exports::golem::services::executor::HandlerWorker<SocketAddr> for Server {
    async fn metadata(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
    ) -> anyhow::Result<Result<WorkerMetadata, ExecutorError>> {
        todo!()
    }

    async fn invoke(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        idempotency_key: Uuid,
        function_name: String,
        args: Vec<WitValue>,
        context: WorkerContext,
        invocation_context: InvocationContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn invoke_and_await(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        idempotency_key: Uuid,
        function_name: String,
        args: Vec<WitValue>,
        context: WorkerContext,
        invocation_context: InvocationContext,
    ) -> anyhow::Result<Result<Option<WitValue>, ExecutorError>> {
        todo!()
    }

    async fn interrupt(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        recover_immediately: bool,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn resume(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn complete_promise(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        promise_id: PromiseId,
        data: Bytes,
    ) -> anyhow::Result<Result<bool, ExecutorError>> {
        todo!()
    }

    async fn update(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        target_version: ComponentVersion,
        mode: UpdateMode,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn oplog(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        start_index: OplogIndex,
        context: WorkerContext,
    ) -> anyhow::Result<Result<ResourceOwn<GetOplogIterator>, ExecutorError>> {
        todo!()
    }

    async fn search_oplog(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        query: String,
        context: WorkerContext,
    ) -> anyhow::Result<Result<ResourceOwn<SearchOplogIterator>, ExecutorError>> {
        todo!()
    }

    async fn fork(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        target_worker_id: WorkerId,
        oplog_index_cutoff: OplogIndex,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn revert(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        target: RevertWorkerTarget,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn cancel_invocation(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        idempotency_key: Uuid,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn list_directory(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        path: String,
        context: WorkerContext,
    ) -> anyhow::Result<Result<Vec<FileSystemNode>, ExecutorError>> {
        todo!()
    }

    async fn get_file_contents(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        path: String,
        context: WorkerContext,
    ) -> anyhow::Result<Result<Bytes, ExecutorError>> {
        todo!()
    }

    async fn activate_plugin(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        installation_id: PluginInstallationId,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn deactivate_plugin(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        installation_id: PluginInstallationId,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn delete(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<Worker>,
        context: WorkerContext,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }
}

impl bindings::exports::golem::services::executor::HandlerWorkerMetadataIterator<SocketAddr>
    for Server
{
    async fn get_next(
        &self,
        cx: SocketAddr,
        self_: ResourceBorrow<WorkerMetadataIterator>,
    ) -> anyhow::Result<Option<Vec<WorkerMetadata>>> {
        todo!()
    }
}

impl bindings::exports::golem::services::executor::Handler<SocketAddr> for Server {
    async fn get_worker(
        &self,
        cx: SocketAddr,
        id: TargetWorkerId,
    ) -> anyhow::Result<Result<ResourceOwn<Worker>, ExecutorError>> {
        todo!()
    }

    async fn create_worker(
        &self,
        cx: SocketAddr,
        id: TargetWorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        context: WorkerContext,
    ) -> anyhow::Result<Result<ResourceOwn<Worker>, ExecutorError>> {
        todo!()
    }

    async fn revoke_shards(
        &self,
        cx: SocketAddr,
        shard_ids: Vec<ShardId>,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn assign_shards(
        &self,
        cx: SocketAddr,
        shard_ids: Vec<ShardId>,
    ) -> anyhow::Result<Result<(), ExecutorError>> {
        todo!()
    }

    async fn get_worker_metadata(
        &self,
        cx: SocketAddr,
        component_id: ComponentId,
        filter: Option<WorkerAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Result<ResourceOwn<WorkerMetadataIterator>, ExecutorError>> {
        todo!()
    }
}

async fn wprc_server() -> anyhow::Result<()> {
    let addr = "0.0.0.0:9999".into();

    let lis = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind TCP listener on `{addr}`"))?;
    let srv = Arc::new(
        wrpc_transport::Server::default();
    );
    let accept = tokio::spawn({
        let srv = Arc::clone(&srv);
        async move {
            loop {
                if let Err(err) = srv.accept(&lis).await {
                    error!(?err, "failed to accept TCP connection");
                }
            }
        }
    });

    let invocations = bindings::serve(srv.as_ref(), Server)
        .await
        .context("failed to serve `wrpc-examples.hello/handler.hello`")?;

    todo!()
}
