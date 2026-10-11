// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::Tracing;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::agent_id;
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{
    IdempotencyKey, OwnedAgentId, PromiseId, ScheduleId, ScheduledAction, ShardAssignment, ShardId,
};
use golem_common::schema::SchemaValue;
use golem_common::serialization::deserialize;
use golem_common::{data_value, model::AgentStatus};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::storage::scheduler::{
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError,
};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::Notify;
use uuid::Uuid;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

async fn run_retirement_during_insert(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    shard_lost: bool,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::shardmanager::ShardEpochEntry;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AssignShardsRequest, RevokeShardsRequest, assign_shards_response, revoke_shards_response,
    };
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let armed = Arc::new(AtomicBool::new(false));
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_scheduler_storage: Some(Arc::new({
                let entered = entered.clone();
                let release = release.clone();
                let armed = armed.clone();
                move |inner| {
                    Arc::new(InsertGate {
                        inner,
                        position: InsertGatePosition::Before,
                        armed: armed.clone(),
                        entered: entered.clone(),
                        release: release.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await?;
    let mut client = executor.client.clone();
    let incarnation = "5eed0000-0000-4000-8000-000000000001";
    if shard_lost {
        let response = client
            .assign_shards(AssignShardsRequest {
                number_of_shards: 1,
                shard_epochs: vec![ShardEpochEntry {
                    shard_id: Some(golem_api_grpc::proto::golem::shardmanager::ShardId {
                        value: 0,
                    }),
                    epoch: 1,
                }],
                revision: 1,
                incarnation_id: incarnation.into(),
            })
            .await?
            .into_inner();
        assert!(matches!(
            response.result,
            Some(assign_shards_response::Result::Success(_))
        ));
    }
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!(
        "Clock",
        if shard_lost {
            "retire-during-insert"
        } else {
            "interrupt-during-insert"
        }
    );
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let old = executor.active_agent(&owned).await.unwrap();
    let boundary = OplogIndex::from_u64(executor.oplog_max_index(&worker_id).await?.as_u64() + 1);
    let key = IdempotencyKey::fresh();
    armed.store(true, Ordering::Release);
    executor
        .invoke_agent_with_key(&component, &agent_id, &key, "sleep_p3", data_value!(12u64))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
    assert!(executor.worker_is_loaded(&owned).await);
    // Completion of the service request proves retirement reached the owner while the
    // automatic decision is still blocked; elapsed time is not evidence of arrival.
    if shard_lost {
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            client.revoke_shards(RevokeShardsRequest {
                shard_ids: vec![golem_api_grpc::proto::golem::shardmanager::ShardId { value: 0 }],
                revision: 1,
                incarnation_id: incarnation.into(),
            }),
        )
        .await??
        .into_inner();
        assert!(matches!(
            response.result,
            Some(revoke_shards_response::Result::Success(_))
        ));
    } else {
        tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&worker_id)).await??;
        assert_eq!(
            executor.get_worker_metadata(&worker_id).await?.status,
            AgentStatus::Interrupted
        );
    }
    assert!(!old.primary().is_loaded().await);
    if !shard_lost {
        let retired_entries = executor.get_oplog(&worker_id, boundary).await?;
        assert!(!retired_entries.iter().any(|entry| matches!(
            entry.entry,
            PublicOplogEntry::Suspend(_) | PublicOplogEntry::AgentInvocationFinished(_)
        )));
    }
    release.notify_one();
    if shard_lost {
        let response = client
            .assign_shards(AssignShardsRequest {
                number_of_shards: 1,
                shard_epochs: vec![ShardEpochEntry {
                    shard_id: Some(golem_api_grpc::proto::golem::shardmanager::ShardId {
                        value: 0,
                    }),
                    epoch: 2,
                }],
                revision: 2,
                incarnation_id: incarnation.into(),
            })
            .await?
            .into_inner();
        assert!(matches!(
            response.result,
            Some(assign_shards_response::Result::Success(_))
        ));
    } else {
        executor.resume(&worker_id, false).await?;
    }
    assert!(
        tokio::time::timeout(
            Duration::from_secs(20),
            executor.invoke_and_await_agent_with_key(
                &component,
                &agent_id,
                &key,
                "sleep_p3",
                data_value!(12u64),
            )
        )
        .await??
        .into_typed::<bool>()?
    );
    assert!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
            .await?
            .into_typed::<bool>()?
    );
    let replacement = executor.active_agent(&owned).await.unwrap();
    assert!(!Arc::ptr_eq(&old, &replacement));
    let entries = executor.get_oplog(&worker_id, boundary).await?;
    assert_eq!(entries.iter().filter(|entry| matches!(&entry.entry,
        PublicOplogEntry::AgentInvocationFinished(params)
            if params.method_name.as_deref().is_some_and(|name| name.replace('-', "_") == "sleep_p3")
    )).count(), 1);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_)))
            .count(),
        usize::from(!shard_lost)
    );
    Ok(())
}

#[test]
#[timeout("60s")]
async fn explicit_interrupt_during_automatic_schedule_persistence(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_retirement_during_insert(last_unique_id, deps, host_api_tests, false).await
}

#[test]
#[timeout("60s")]
async fn shard_retirement_during_automatic_schedule_persistence(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_retirement_during_insert(last_unique_id, deps, host_api_tests, true).await
}

#[test]
#[timeout("45s")]
async fn invocation_deadline_unwinds_parked_p3_wait(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.suspend.wait_suspend_grace = Duration::from_secs(300);
                config.limits.max_invocation_duration = Some(Duration::from_secs(5));
                config.oplog.max_operations_before_commit = 1;
                config.retry.max_attempts = 1;
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("Clock", "deadline-parked-p3");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = OplogIndex::from_u64(executor.oplog_max_index(&worker_id).await?.as_u64() + 1);
    let invocation =
        executor.invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(60u64));
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => panic!("invocation ended before durable wait was observed: {result:?}"),
        parked = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if executor.get_oplog(&worker_id, boundary).await?.iter().any(|entry| matches!(
                    &entry.entry, PublicOplogEntry::Start(start) if start.function_name.contains("wait")
                )) { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<(), anyhow::Error>(())
        }) => { parked??; }
    }
    assert!(
        executor
            .worker_is_loaded(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_id
            ))
            .await
    );
    let error = tokio::time::timeout(Duration::from_secs(10), invocation)
        .await?
        .expect_err("deadline must fail the invocation");
    assert!(
        error.to_string().contains("maximum invocation duration"),
        "{error}"
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Failed, Duration::from_secs(10))
        .await?;
    let entries = executor.get_oplog(&worker_id, boundary).await?;
    let waits: Vec<_> = entries
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
                PublicOplogEntry::Start(start) if start.function_name.contains("wait")
            )
        })
        .collect();
    assert_eq!(waits.len(), 1);
    assert!(!entries.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::End(end) if end.start_index == waits[0].oplog_index
    )));
    assert!(!entries.iter().any(|entry| matches!(
        entry.entry,
        PublicOplogEntry::AgentInvocationFinished(_)
            | PublicOplogEntry::Suspend(_)
            | PublicOplogEntry::Interrupted(_)
    )));
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}

fn promise_oplog_index(value: &SchemaValue) -> OplogIndex {
    let SchemaValue::Record { fields } = value else {
        panic!("expected promise id record")
    };
    let SchemaValue::U64(oplog_index) = fields[1] else {
        panic!("expected promise id oplog index")
    };
    OplogIndex::from_u64(oplog_index)
}

async fn assert_single_await_promise_completion(
    executor: &golem_worker_executor_test_utils::TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
) -> anyhow::Result<()> {
    let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
    let started = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(
                        &params.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name.replace('-', "_") == "await_promise"
                    )
            )
        })
        .count();
    let finished = oplog
        .iter()
        .filter(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::AgentInvocationFinished(params)
                if params.method_name.as_deref().is_some_and(|name| name.replace('-', "_") == "await_promise")
        ))
        .count();
    assert_eq!((started, finished), (1, 1));
    Ok(())
}

#[derive(Clone, Copy)]
enum InsertGatePosition {
    Before,
    After,
    AfterResume,
}

struct InsertGate {
    inner: Arc<dyn SchedulerStorage + Send + Sync>,
    position: InsertGatePosition,
    armed: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl Debug for InsertGate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertGate").finish_non_exhaustive()
    }
}

#[async_trait]
impl SchedulerStorage for InsertGate {
    async fn insert(
        &self,
        id: ScheduleId,
        due: DateTime<Utc>,
        shard: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        let is_timer = matches!(
            deserialize::<ScheduledAction>(action).unwrap(),
            ScheduledAction::CompletePromise { .. }
        );
        let gate = is_timer && self.armed.swap(false, Ordering::AcqRel);
        if gate && matches!(self.position, InsertGatePosition::Before) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        let result = self.inner.insert(id, due, shard, action).await;
        if gate && !matches!(self.position, InsertGatePosition::Before) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        result
    }

    async fn cancel(&self, id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.inner.cancel(id).await
    }
    async fn claim_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
        limit: u32,
        ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        self.inner.claim_due(now, assignment, limit, ttl).await
    }
    async fn count_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }
    async fn extend_lease(
        &self,
        id: &ScheduleId,
        owner: Uuid,
        until: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.extend_lease(id, owner, until).await
    }
    async fn ack(&self, id: &ScheduleId, owner: Uuid) -> Result<bool, SchedulerStorageError> {
        self.inner.ack(id, owner).await
    }
}

async fn run_insert_race(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    position: InsertGatePosition,
) -> anyhow::Result<()> {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let armed = Arc::new(AtomicBool::new(false));
    let overrides = TestExecutorOverrides {
        wrap_scheduler_storage: Some(Arc::new({
            let entered = entered.clone();
            let release = release.clone();
            let armed = armed.clone();
            move |inner| {
                Arc::new(InsertGate {
                    inner,
                    position,
                    armed: armed.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                })
            }
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!(
        "Clock",
        match position {
            InsertGatePosition::Before => "ready-during-persistence",
            InsertGatePosition::After => "ready-after-persistence",
            InsertGatePosition::AfterResume => "resume-before-suspend-handoff",
        }
    );
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
        .await?;
    let boundary = executor.oplog_max_index(&worker_id).await?;

    armed.store(true, Ordering::Release);
    let invocation =
        executor.invoke_and_await_agent(&component, &agent_id, "sleep_p3", data_value!(12u64));
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => panic!("timer completed before its schedule was gated: {result:?}"),
        entered = tokio::time::timeout(Duration::from_secs(10), entered.notified()) => { entered?; }
    }
    if matches!(position, InsertGatePosition::AfterResume) {
        use golem_api_grpc::proto::golem::workerexecutor::v1::{
            ResumeWorkerRequest, resume_worker_response,
        };
        let loads = executor.instance_load_count(&worker_id);
        let response = executor
            .client
            .clone()
            .resume_worker(ResumeWorkerRequest {
                agent_id: Some(worker_id.clone().into()),
                environment_id: Some(context.default_environment_id.into()),
                force: Some(true),
                auth_ctx: Some(executor.auth_ctx().into()),
                principal: None,
            })
            .await?
            .into_inner();
        assert!(matches!(
            response.result,
            Some(resume_worker_response::Result::Success(_))
        ));
        // Reconstruction before the original timer deadline proves the earlier activation
        // survived the subsequent Suspend and unload handoff.
        armed.store(true, Ordering::Release);
        release.notify_one();
        tokio::select! {
            result = &mut invocation => panic!("long timer completed before reconstruction: {result:?}"),
            reloaded = tokio::time::timeout(Duration::from_secs(5), async {
                while executor.instance_load_count(&worker_id) <= loads {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }) => { reloaded?; }
        }
        assert!(executor.instance_load_count(&worker_id) > loads);
    }
    // The real timer must complete even while its suspension policy is blocked in persistence.
    let result = tokio::time::timeout(Duration::from_secs(15), &mut invocation).await??;
    release.notify_one();

    assert!(result.into_typed::<bool>()?);
    assert!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "healthcheck", data_value!())
            .await?
            .into_typed::<bool>()?
    );
    let entries = executor.get_oplog(&worker_id, boundary).await?;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_)))
            .count(),
        usize::from(matches!(position, InsertGatePosition::AfterResume)),
        "only the attempt preceding reconstruction may suspend"
    );
    assert_ne!(
        executor.get_worker_metadata(&worker_id).await?.status,
        AgentStatus::Failed
    );
    Ok(())
}

#[test]
#[timeout("45s")]
async fn timer_ready_while_scheduler_insert_is_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_insert_race(
        last_unique_id,
        deps,
        host_api_tests,
        InsertGatePosition::Before,
    )
    .await
}

#[test]
#[timeout("45s")]
async fn timer_ready_before_persisted_schedule_returns(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_insert_race(
        last_unique_id,
        deps,
        host_api_tests,
        InsertGatePosition::After,
    )
    .await
}

#[test]
#[timeout("45s")]
async fn resume_during_persistence_survives_suspend_handoff(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_insert_race(
        last_unique_id,
        deps,
        host_api_tests,
        InsertGatePosition::AfterResume,
    )
    .await
}

#[test]
#[timeout("30s")]
async fn promise_completed_before_wait_returns_without_suspending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "promise-completed-before-wait");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let promise_id = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .expect("create_promise must return an id");
    let boundary = executor.oplog_max_index(&worker_id).await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: promise_oplog_index(&promise_id),
            },
            vec![3, 1, 4],
        )
        .await?;
    let key = IdempotencyKey::fresh();
    let params = crate::raw_params(vec![promise_id]);
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "await_promise",
            params.clone(),
        )
        .await?;
    assert_eq!(result.into_typed::<Vec<u8>>()?, vec![3, 1, 4]);
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &agent_id, &key, "await_promise", params)
            .await?
            .into_typed::<Vec<u8>>()?,
        vec![3, 1, 4]
    );
    assert!(
        executor
            .get_oplog(&worker_id, boundary)
            .await?
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Suspend(_)))
    );
    assert!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
            .await?
            .into_return_value()
            .is_some()
    );
    assert_single_await_promise_completion(&executor, &worker_id).await
}

#[test]
#[timeout("45s")]
async fn promise_completion_during_suspend_handoff_is_not_lost(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };

    let context = TestContext::new(last_unique_id);
    let faults = KeyValueStorageFaults::default();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.agent_status_flush.interval = Duration::from_secs(3600);
            })),
            wrap_key_value_storage: Some(Arc::new({
                let faults = faults.clone();
                move |inner| Arc::new(FaultInjectingKeyValueStorage::new(inner, faults.clone()))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "promise-completion-during-handoff");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let promise_id = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .expect("create_promise must return an id");
    let params = crate::raw_params(vec![promise_id.clone()]);
    let key = IdempotencyKey::fresh();
    let mut gate = faults.pause_next("update_status");
    executor
        .invoke_agent_with_key(&component, &agent_id, &key, "await_promise", params.clone())
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            gate.entered().await;
            if executor.get_worker_metadata(&worker_id).await?.status == AgentStatus::Suspended {
                break;
            }
            let next = faults.pause_next("update_status");
            gate.release();
            gate = next;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    // The suspended status is committed, but its forced cache flush holds the runtime
    // before unload. Completion must activate this stopping generation, not a cold one.
    assert!(executor.worker_is_loaded(&owned).await);
    let loads = executor.instance_load_count(&worker_id);
    let completion = tokio::time::timeout(
        Duration::from_secs(5),
        executor.complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: promise_oplog_index(&promise_id),
            },
            vec![2, 7, 1, 8],
        ),
    )
    .await;
    gate.release();
    completion??;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    assert!(executor.instance_load_count(&worker_id) > loads);
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &agent_id, &key, "await_promise", params)
            .await?
            .into_typed::<Vec<u8>>()?,
        vec![2, 7, 1, 8]
    );
    assert_single_await_promise_completion(&executor, &worker_id).await?;
    assert!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
            .await?
            .into_return_value()
            .is_some()
    );
    Ok(())
}

#[test]
#[timeout("45s")]
async fn promise_completion_after_unload_resumes_with_exact_data(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("GolemHostApi", "promise-completion-after-unload");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let promise_id = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .expect("create_promise must return an id");
    let params = crate::raw_params(vec![promise_id.clone()]);
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(&component, &agent_id, &key, "await_promise", params.clone())
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_loaded(&owned).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?;
    assert!(!executor.worker_is_loaded(&owned).await);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: promise_oplog_index(&promise_id),
            },
            vec![9, 2, 6, 5],
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "await_promise",
            params.clone(),
        )
        .await?;
    assert_eq!(result.into_typed::<Vec<u8>>()?, vec![9, 2, 6, 5]);
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &agent_id, &key, "await_promise", params)
            .await?
            .into_typed::<Vec<u8>>()?,
        vec![9, 2, 6, 5]
    );
    assert!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
            .await?
            .into_return_value()
            .is_some()
    );
    assert_single_await_promise_completion(&executor, &worker_id).await
}
