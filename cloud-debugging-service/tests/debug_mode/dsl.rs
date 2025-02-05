use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use async_trait::async_trait;
use cloud_common::model::TokenSecret;
use cloud_debugging_service::model::params::{
    ConnectParams, ConnectResult, ForkParams, ForkResult, PlaybackOverride, PlaybackParams,
    PlaybackResult, RewindParams, RewindResult,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::WorkerId;
use uuid::Uuid;

#[async_trait]
pub trait TestDslDebugMode {
    async fn connect(&mut self, worker_id: &WorkerId) -> anyhow::Result<ConnectResult>;

    async fn playback(
        &mut self,
        target_index: OplogIndex,
        overrides: Option<Vec<PlaybackOverride>>,
    ) -> anyhow::Result<PlaybackResult>;

    async fn rewind(&mut self, target_index: OplogIndex) -> anyhow::Result<RewindResult>;

    async fn fork(
        &mut self,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> anyhow::Result<ForkResult>;
}

#[async_trait]
impl TestDslDebugMode for DebugWorkerExecutorClient {
    async fn connect(&mut self, worker_id: &WorkerId) -> anyhow::Result<ConnectResult> {
        let token = TokenSecret::new(Uuid::new_v4());

        let id = self
            .send_jrpc_msg(
                "connect",
                ConnectParams {
                    worker_id: worker_id.clone(),
                    token,
                },
            )
            .await?;

        self.read_jrpc_msg(id).await
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
                },
            )
            .await?;

        self.read_jrpc_msg(id).await
    }

    async fn rewind(&mut self, target_index: OplogIndex) -> anyhow::Result<RewindResult> {
        let id = self
            .send_jrpc_msg("rewind", RewindParams { target_index })
            .await?;

        self.read_jrpc_msg(id).await
    }

    async fn fork(
        &mut self,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> anyhow::Result<ForkResult> {
        let id = self
            .send_jrpc_msg(
                "fork",
                ForkParams {
                    target_worker_id: target_worker_id.clone(),
                    oplog_index_cut_off,
                },
            )
            .await?;

        self.read_jrpc_msg(id).await
    }
}
