use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use async_trait::async_trait;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::AgentId;
use golem_debugging_service::model::params::{
    ConnectParams, ConnectResult, ForkParams, ForkResult, PlaybackOverride, PlaybackParams,
    PlaybackResult, RewindParams, RewindResult,
};

#[async_trait]
pub trait TestDslDebugMode {
    async fn connect(&mut self, agent_id: &AgentId) -> anyhow::Result<ConnectResult>;

    async fn playback(
        &mut self,
        target_index: OplogIndex,
        overrides: Option<Vec<PlaybackOverride>>,
    ) -> anyhow::Result<PlaybackResult>;

    async fn rewind(&mut self, target_index: OplogIndex) -> anyhow::Result<RewindResult>;

    async fn fork(
        &mut self,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
    ) -> anyhow::Result<ForkResult>;

    async fn current_index(&mut self) -> anyhow::Result<OplogIndex>;
}

#[async_trait]
impl TestDslDebugMode for DebugWorkerExecutorClient {
    async fn connect(&mut self, agent_id: &AgentId) -> anyhow::Result<ConnectResult> {
        let id = self
            .send_jrpc_msg(
                "connect",
                ConnectParams {
                    agent_id: agent_id.clone(),
                },
            )
            .await?;

        self.read_jrpc_response(id).await
    }

    async fn playback(
        &mut self,
        target_index: OplogIndex,
        overrides: Option<Vec<PlaybackOverride>>,
    ) -> anyhow::Result<PlaybackResult> {
        let id = self
            .send_jrpc_msg(
                "playback",
                PlaybackParams {
                    target_index,
                    overrides,
                    ensure_invocation_boundary: None,
                },
            )
            .await?;

        self.read_jrpc_response(id).await
    }

    async fn rewind(&mut self, target_index: OplogIndex) -> anyhow::Result<RewindResult> {
        let id = self
            .send_jrpc_msg(
                "rewind",
                RewindParams {
                    target_index,
                    ensure_invocation_boundary: None,
                },
            )
            .await?;

        self.read_jrpc_response(id).await
    }

    async fn fork(
        &mut self,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
    ) -> anyhow::Result<ForkResult> {
        let id = self
            .send_jrpc_msg(
                "fork",
                ForkParams {
                    target_agent_id: target_agent_id.clone(),
                    oplog_index_cut_off,
                },
            )
            .await?;

        self.read_jrpc_response(id).await
    }

    async fn current_index(&mut self) -> anyhow::Result<OplogIndex> {
        let id = self.send_jrpc_msg("current_oplog_index", ()).await?;

        let result = self.read_jrpc_response(id).await?;

        Ok(result)
    }
}
