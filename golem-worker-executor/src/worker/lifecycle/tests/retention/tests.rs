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

use super::*;
use test_r::test;

#[test]
fn a_revert_cut_error_keeps_an_unreadable_oplog() {
    use crate::durable_host::durable_stream::StreamStoreError;
    use crate::services::oplog::retention::test_oplog::expired;
    use crate::services::oplog::{OplogReadError, OplogReadSource, UnreadableOplog};

    let agent_id = AgentId::from_parts(7, 1);
    let unreadable = UnreadableOplog::new(&agent_id, expired(4));
    assert_eq!(
        revert_cut_error(StreamStoreError::Unreadable(unreadable.clone())),
        WorkerExecutorError::from(unreadable)
    );
    assert!(matches!(
        revert_cut_error(StreamStoreError::InvalidOffset("outside".to_string())),
        WorkerExecutorError::InvalidRequest { .. }
    ));
    assert!(matches!(
        revert_cut_error(StreamStoreError::CounterOverflow),
        WorkerExecutorError::Runtime { .. }
    ));
    let unavailable = StreamStoreError::Unreadable(UnreadableOplog::new(
        &agent_id,
        OplogReadError::source_unavailable(OplogReadSource::Archive(0), "storage outage"),
    ));
    let details = unavailable.to_string();
    assert_eq!(
        revert_cut_error(unavailable),
        WorkerExecutorError::Runtime { details }
    );
}

/// The revert walk counts the invocations back from the last index. It stops at the retained
/// start, so an invocation below the start does not count.
#[test]
async fn a_revert_walk_counts_the_invocations_back_to_the_retained_start() {
    use crate::services::golem_config::GolemConfig;
    use crate::services::oplog::OplogService;
    use crate::services::oplog::retention::test_oplog::{RetainedOplog, invocation_started, jump};
    use crate::services::shard::ShardServiceDefault;
    use crate::services::worker::session_index_tests::UnusedComponentService;
    use crate::services::worker::{DefaultWorkerService, WorkerService};
    use futures::{StreamExt, TryStreamExt};
    use golem_common::model::ShardEpoch;
    use std::sync::Arc;

    struct Deps {
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
    }
    impl HasWorkerService for Deps {
        fn worker_service(&self) -> Arc<dyn WorkerService> {
            self.worker_service.clone()
        }
    }
    impl HasOplogService for Deps {
        fn oplog_service(&self) -> Arc<dyn OplogService> {
            self.oplog_service.clone()
        }
    }

    let oplog = RetainedOplog::new().await;
    let handle = oplog.create(oplog.create_entry(), ShardEpoch(1)).await;
    futures::stream::iter([
        invocation_started(),
        jump(3),
        jump(4),
        invocation_started(),
        jump(6),
    ])
    .then(|entry| handle.add(entry))
    .try_collect::<Vec<_>>()
    .await
    .unwrap();
    handle.commit(CommitLevel::Always).await.unwrap();
    let deps = Deps {
        worker_service: Arc::new(DefaultWorkerService::new(
            oplog.key_value_storage(),
            Arc::new(ShardServiceDefault::new()),
            oplog.service.clone(),
            Arc::new(UnusedComponentService),
            Arc::new(GolemConfig::default()),
        )),
        oplog_service: oplog.service.clone(),
    };
    let resolve = |number_of_invocations| {
        Worker::<crate::workerctx::default::Context>::resolve_revert_last_invocations(
            &deps,
            &oplog.owned_agent_id,
            number_of_invocations,
        )
    };

    assert_eq!(
        resolve(2).await.unwrap(),
        ResolvedRevert {
            last_oplog_index: OplogIndex::INITIAL,
        }
    );
    oplog.advance(3).await;
    assert_eq!(
        resolve(1).await.unwrap(),
        ResolvedRevert {
            last_oplog_index: OplogIndex::from_u64(4),
        }
    );
    assert_eq!(
        resolve(2).await.unwrap_err(),
        WorkerExecutorError::invalid_request("Could not find 2 invocations to revert")
    );
}
