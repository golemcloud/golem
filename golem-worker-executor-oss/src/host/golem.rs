use async_trait::async_trait;
use golem_common::model::{PromiseId, TemplateId, WorkerId};
use golem_worker_executor_base::workerctx::WorkerCtx;
use uuid::Uuid;

use crate::context::Context;
use crate::preview2::golem::api::host;
use crate::preview2::golem::api::host::Host;

#[async_trait]
impl Host for Context {
    async fn golem_create_promise(&mut self) -> Result<host::PromiseId, anyhow::Error> {
        let promise_idx = self.next_promise_id();
        let promise_id = self
            .promise_service()
            .create(&self.worker_id().worker_id, promise_idx)
            .await;
        Ok(promise_id.into())
    }

    async fn golem_await_promise(
        &mut self,
        promise_id: host::PromiseId,
    ) -> Result<Vec<u8>, anyhow::Error> {
        let pid: PromiseId = promise_id.clone().into();
        match self.promise_service().poll(pid).await? {
            Some(result) => Ok(result),
            None => {
                tokio::time::sleep(self.additional_golem_config().promise_poll_interval).await;
                self.golem_await_promise(promise_id).await
            }
        }
    }

    async fn golem_complete_promise(
        &mut self,
        promise_id: host::PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, anyhow::Error> {
        Ok(self
            .promise_service()
            .complete(promise_id.into(), data)
            .await?)
    }

    async fn golem_delete_promise(
        &mut self,
        promise_id: host::PromiseId,
    ) -> Result<(), anyhow::Error> {
        self.promise_service().delete(promise_id.into()).await;
        Ok(())
    }

    async fn get_self_uri(&mut self, function_name: String) -> Result<host::Uri, anyhow::Error> {
        let uri = host::Uri {
            uri: format!("{}/{}", self.worker_id().worker_id.uri(), function_name),
        };
        Ok(uri)
    }
}

impl From<WorkerId> for host::WorkerId {
    fn from(worker_id: WorkerId) -> Self {
        host::WorkerId {
            template_id: worker_id.template_id.into(),
            worker_name: worker_id.worker_name,
        }
    }
}

impl From<host::WorkerId> for WorkerId {
    fn from(host: host::WorkerId) -> Self {
        Self {
            template_id: host.template_id.into(),
            worker_name: host.worker_name,
        }
    }
}

impl From<PromiseId> for host::PromiseId {
    fn from(promise_id: PromiseId) -> Self {
        host::PromiseId {
            worker_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx,
        }
    }
}

impl From<host::PromiseId> for PromiseId {
    fn from(host: host::PromiseId) -> Self {
        Self {
            worker_id: host.worker_id.into(),
            oplog_idx: host.oplog_idx,
        }
    }
}
