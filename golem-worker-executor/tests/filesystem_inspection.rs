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

use crate::Tracing;
use golem_common::model::component::CanonicalFilePath;
use golem_common::model::filesystem::{FileByteSelection, FileReadExtent, FileReadHead};
use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{AgentId, OplogIndex, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_service_base::model::FileReadResponse;
use golem_test_framework::dsl::{TestDsl, count_agent_invocation_pair_since};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
    start_with_concurrent_agent_limit,
};
use serde_json::Value;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio_stream::StreamExt;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("initial_file_system")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

fn case(id: &str) -> Value {
    let corpus: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    )))
    .unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing corpus case {id}"))
        .clone()
}

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

async fn body(mut response: FileReadResponse) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.body.next().await {
        let chunk = chunk?;
        assert!(!chunk.is_empty());
        assert!(chunk.len() <= 64 * 1024);
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[test]
#[timeout("2m")]
async fn live_file_inspection_initializes_once_and_restores_without_exports(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    for id in [
        "file-live-initializer-created",
        "lifecycle-live-concurrent-first-access",
    ] {
        let vector = case(id);
        let contents = if id == "file-live-initializer-created" {
            bytes(
                vector["input"]["initializer_files"]["/a.txt"]
                    .as_str()
                    .unwrap(),
            )
        } else {
            let event = vector["input"]["events"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|event| {
                    event
                        .as_str()
                        .unwrap()
                        .strip_prefix("initializer-creates-file:")
                })
                .unwrap();
            bytes(event)
        };
        let parsed = agent_id!("Inspection", id, "/a.txt", contents, false);
        let agent = AgentId {
            component_id: component.id,
            agent_id: parsed.to_string(),
        };
        assert!(
            executor.get_worker_metadata_opt(&agent).await?.is_none(),
            "{id}"
        );
        let (first, second) = tokio::join!(
            executor.get_file_contents(&agent, "/a.txt"),
            executor.get_file_contents(&agent, "/a.txt")
        );
        let expected: &[u8] = if id == "file-live-initializer-created" {
            b"abc"
        } else {
            b"a"
        };
        assert_eq!(first?.as_ref(), expected, "{id}");
        assert_eq!(second?.as_ref(), expected, "{id}");
        let oplog = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
        let mut initializations = 0;
        for entry in &oplog {
            if let PublicOplogEntry::AgentInvocationStarted(parameters) = &entry.entry {
                assert!(
                    matches!(
                        parameters.invocation,
                        PublicAgentInvocation::AgentInitialization(_)
                    ),
                    "{id}: inspection invoked an export"
                );
                initializations += 1;
            }
        }
        assert_eq!(initializations, 1, "{id}");

        let restore = case("file-live-restore");
        let restored = bytes(
            restore["input"]["restored_files"]["/a.txt"]
                .as_str()
                .unwrap(),
        );
        executor
            .invoke_and_await_agent(
                &component,
                &parsed,
                "replace",
                data_value!("/a.txt", restored, 0u64),
            )
            .await?;
        let before = executor.oplog_max_index(&agent).await?;
        let owned = OwnedAgentId::new(context.default_environment_id, &agent);
        let worker = executor.active_agent(&owned).await.unwrap().primary();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !worker.stop_if_idle().await {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            !executor.worker_is_loaded(&owned).await,
            "file-live-restore"
        );
        assert_eq!(
            executor.get_file_contents(&agent, "/a.txt").await?.as_ref(),
            b"bcd",
            "file-live-restore"
        );
        let oplog = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
        assert_eq!(
            count_agent_invocation_pair_since(&oplog, before),
            (0, 0),
            "file-live-restore"
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn live_file_inspection_initializer_failure_is_not_a_miss(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let id = "file-live-initializer-failure";
    let vector = case(id);
    assert_eq!(vector["input"]["initializer_error"], "trap");
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let parsed = agent_id!("Inspection", id, "/a.txt", Vec::<u8>::new(), true);
    let agent = AgentId {
        component_id: component.id,
        agent_id: parsed.to_string(),
    };
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        executor.get_file_contents(&agent, "/a.txt"),
    )
    .await?
    .unwrap_err();
    assert!(
        error.to_string().contains("FILE_READ_ERROR_LIFECYCLE"),
        "{id}: {error}"
    );
    assert_eq!(
        executor.get_worker_metadata(&agent).await?.status,
        golem_common::model::AgentStatus::Failed,
        "{id}"
    );
    let before_rejected_read = executor.oplog_max_index(&agent).await?;
    let error = executor
        .get_file_contents(&agent, "/a.txt")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("PreviousInvocationFailed")
            && error.to_string().contains("inspection initializer failed"),
        "{id}: {error}"
    );
    let oplog = executor.get_oplog(&agent, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&oplog, before_rejected_read),
        (0, 0),
        "{id}: failed status admitted another filesystem operation"
    );
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))),
        "{id}"
    );
    for entry in oplog {
        if let PublicOplogEntry::AgentInvocationStarted(parameters) = entry.entry {
            assert!(
                matches!(
                    parameters.invocation,
                    PublicAgentInvocation::AgentInitialization(_)
                ),
                "{id}"
            );
        }
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn live_file_inspection_corpus_byte_selections(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use FileByteSelection::*;
    use FileReadExtent::{Selected, Unsatisfiable};
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    // These are generic byte-selection expectations, not an HTTP Range parser or header test.
    for (id, selection, extent, expected) in [
        (
            "file-range-bounded",
            Bounded {
                start: 2,
                end_inclusive: 5,
            },
            Selected {
                offset: 2,
                length: 4,
            },
            b"2345".as_slice(),
        ),
        (
            "file-range-open-ended",
            OpenEnded { start: 7 },
            Selected {
                offset: 7,
                length: 3,
            },
            b"789",
        ),
        (
            "file-range-suffix",
            Suffix { length: 4 },
            Selected {
                offset: 6,
                length: 4,
            },
            b"6789",
        ),
        (
            "file-range-clipped",
            Bounded {
                start: 1,
                end_inclusive: 99,
            },
            Selected {
                offset: 1,
                length: 2,
            },
            b"bc",
        ),
        (
            "file-range-at-length",
            OpenEnded { start: 3 },
            Unsatisfiable,
            b"",
        ),
        (
            "file-range-empty-file",
            Bounded {
                start: 0,
                end_inclusive: 0,
            },
            Unsatisfiable,
            b"",
        ),
        (
            "file-range-zero-suffix",
            Suffix { length: 0 },
            Unsatisfiable,
            b"",
        ),
        (
            "file-head-ignores-range",
            MetadataOnly,
            Selected {
                offset: 0,
                length: 0,
            },
            b"",
        ),
        (
            "file-live-no-validators",
            Full,
            Selected {
                offset: 0,
                length: 4,
            },
            b"a\0b\xff",
        ),
    ] {
        let vector = case(id);
        let contents = bytes(vector["input"]["body_hex"].as_str().unwrap());
        let total_size = contents.len() as u64;
        let parsed = agent_id!("Inspection", id, "/a.txt", contents, false);
        let agent = executor.start_agent(&component.id, parsed).await?;
        let owned = OwnedAgentId::new(context.default_environment_id, &agent);
        let worker = executor.active_agent(&owned).await.unwrap().primary();
        let response = worker
            .read_file(
                CanonicalFilePath::from_abs_str("/a.txt").unwrap(),
                selection,
            )
            .await?;
        let FileReadHead::File(metadata) = &response.head else {
            panic!("{id}: {:?}", response.head)
        };
        assert_eq!(metadata.total_size, total_size, "{id}");
        assert_eq!(metadata.selection, extent, "{id}");
        assert_eq!(body(response).await?, expected, "{id}");
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn live_file_inspection_releases_ownership_after_production(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let id = "lifecycle-live-read-serialized";
    let vector = case(id);
    let events = vector["input"]["events"].as_array().unwrap();
    let initial = bytes(
        events[0]
            .as_str()
            .unwrap()
            .strip_prefix("initialize-file:")
            .unwrap(),
    );
    let replacement = bytes(
        events
            .iter()
            .find_map(|event| event.as_str().unwrap().strip_prefix("enqueue-write:"))
            .unwrap(),
    );
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let parsed = agent_id!("Inspection", id, "/a.txt", initial, false);
    let agent = executor.start_agent(&component.id, parsed.clone()).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &agent);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let target = CanonicalFilePath::from_abs_str("/a.txt").unwrap();
    let mut response = worker
        .read_file(target.clone(), FileByteSelection::Full)
        .await?;
    let FileReadHead::File(metadata) = &response.head else {
        panic!("{id}: {:?}", response.head)
    };
    assert_eq!(metadata.total_size, 4, "{id}");
    // Once the selected bytes have been copied into the bounded response, filesystem ownership
    // is released without waiting for consumer-observed EOF.
    assert_eq!(
        response.body.next().await.unwrap()?.as_ref(),
        b"abcd",
        "{id}"
    );
    let mut write = Box::pin(executor.invoke_and_await_agent(
        &component,
        &parsed,
        "replace",
        data_value!("/a.txt", replacement, 0u64),
    ));
    tokio::time::timeout(Duration::from_secs(10), &mut write).await??;
    let queued_read = worker.read_file(target.clone(), FileByteSelection::Full);
    assert!(response.body.next().await.is_none(), "{id}");
    let queued_response = tokio::time::timeout(Duration::from_secs(10), queued_read).await??;
    assert_eq!(
        body(queued_response).await?,
        bytes(vector["expect"]["response_bodies_hex"][1].as_str().unwrap()),
        "{id}"
    );
    let next = worker.read_file(target, FileByteSelection::Full).await?;
    let FileReadHead::File(metadata) = &next.head else {
        panic!("{id}")
    };
    assert_eq!(metadata.total_size, 2, "{id}");
    assert_eq!(body(next).await?, b"xy", "{id}");
    Ok(())
}

#[test]
#[timeout("2m")]
async fn live_file_inspection_drop_releases_update_without_changing_selected_root(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::component::AgentFilePermissions;
    use golem_test_framework::model::IFSEntry;
    use std::path::PathBuf;

    let id = "lifecycle-live-drop-releases-update";
    let vector = case(id);
    assert_eq!(vector["input"]["events"][3], "consumer-drop");
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let file = |source: &str, target: &str| IFSEntry {
        source_path: PathBuf::from(format!("initial-file-system/files/{source}.txt")),
        target_path: CanonicalFilePath::from_abs_str(target).unwrap(),
        permissions: AgentFilePermissions::ReadOnly,
    };
    let directory = tempfile::tempdir()?;
    let large_file = directory.path().join("large.txt");
    std::fs::write(&large_file, vec![42; 3 * 65536])?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .with_files(
            "P3FileSystem",
            &[IFSEntry {
                source_path: large_file,
                target_path: CanonicalFilePath::from_abs_str("/selected/a.txt").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )
        .store()
        .await?;
    let parsed = agent_id!("P3FileSystem", id);
    let agent = executor.start_agent(&component.id, parsed).await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &agent);
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let target = CanonicalFilePath::from_abs_str("/selected/a.txt").unwrap();
    let response = worker
        .read_file(target.clone(), FileByteSelection::Full)
        .await?;
    let updated = executor
        .update_component_with_files(
            &component.id,
            "P3FileSystem",
            &fixture.wasm_name,
            vec![file("foo", "/selected/a.txt")],
        )
        .await?;
    let update = {
        let executor = executor.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            executor
                .auto_update_worker(&agent, updated.revision, false)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), update).await???;
    assert!(
        executor
            .wait_for_component_revision(&agent, updated.revision, Duration::from_millis(200))
            .await
            .is_err(),
        "{id}: update passed a backpressured file producer"
    );
    drop(response);
    assert_eq!(
        executor
            .wait_for_component_revision(&agent, updated.revision, Duration::from_secs(10))
            .await?
            .component_revision,
        updated.revision,
        "{id}"
    );
    let next = worker
        .read_file(target.clone(), FileByteSelection::Full)
        .await?;
    assert_eq!(body(next).await?, b"foo\n", "{id}");
    assert_eq!(
        executor
            .get_worker_metadata(&agent)
            .await?
            .component_revision,
        updated.revision,
        "{id}"
    );

    // A later revision moving the provisioned file must not change an already selected root.
    let moved = executor
        .update_component_with_files(
            &component.id,
            "P3FileSystem",
            &fixture.wasm_name,
            vec![file("baz", "/other/a.txt")],
        )
        .await?;
    executor
        .auto_update_worker(&agent, moved.revision, false)
        .await?;
    let missing = worker.read_file(target, FileByteSelection::Full).await?;
    assert_eq!(missing.head, FileReadHead::Absent);
    assert!(body(missing).await?.is_empty());
    assert_eq!(
        executor
            .get_worker_metadata(&agent)
            .await?
            .component_revision,
        moved.revision
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn live_file_inspection_queued_before_suspend_observes_completed_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::Timestamp;
    use golem_service_base::error::worker_executor::InterruptKind;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_concurrent_agent_limit(deps, &context, 1).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let parsed = agent_id!("Inspection", "suspend", "/a.txt", b"before".to_vec(), false);
    let agent = executor.start_agent(&component.id, parsed.clone()).await?;
    assert_eq!(
        executor.get_file_contents(&agent, "/a.txt").await?.as_ref(),
        b"before"
    );
    let owned = OwnedAgentId::new(context.default_environment_id, &agent);
    let worker = executor
        .production_active_agent(&owned)
        .await
        .unwrap()
        .primary();
    let before = executor.oplog_max_index(&agent).await?;
    let write = {
        let executor = executor.clone();
        let component = component.clone();
        let parsed = parsed.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &parsed,
                    "replace",
                    data_value!("/a.txt", b"after".to_vec(), 5000u64),
                )
                .await
        })
    };
    // Wait for active execution, not just a queued invocation.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = executor.get_oplog(&agent, before.next()).await.unwrap();
            if oplog.iter().any(|entry| {
                matches!(&entry.entry,
                PublicOplogEntry::AgentInvocationStarted(parameters)
                if matches!(parameters.invocation, PublicAgentInvocation::AgentMethodInvocation(_)))
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;

    let final_write = {
        let executor = executor.clone();
        let component = component.clone();
        let parsed = parsed.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &parsed,
                    "replace",
                    data_value!("/a.txt", b"final".to_vec(), 0u64),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker
            .get_attached_last_known_status()
            .await
            .pending_invocations
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    // The pending write predates this read, but inspection must observe the current state at
    // the next invocation boundary instead of waiting for already-queued invocations.
    let mut read = Box::pin(worker.read_file(
        CanonicalFilePath::from_abs_str("/a.txt").unwrap(),
        FileByteSelection::Full,
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(1), &mut read)
            .await
            .is_err(),
        "read completed before the active write"
    );

    let permit_agent = AgentId {
        component_id: component.id,
        agent_id: agent_id!("Inspection", "inspection-permit-holder").to_string(),
    };
    let permit_waiter = executor.acquire_account_concurrent_agent_permit(permit_agent);
    tokio::pin!(permit_waiter);
    // Poll the waiter into the scheduler before Suspend releases this worker's permit. This makes
    // the holder precede reconstruction in the permit scheduler without a timing sleep.
    tokio::select! {
        biased;
        _ = &mut permit_waiter => panic!("permit waiter acquired while the worker was running"),
        _ = tokio::task::yield_now() => {}
    }
    worker
        .set_interrupting(InterruptKind::Suspend(Timestamp::now_utc()))
        .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_loaded(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let held_permit = tokio::time::timeout(Duration::from_secs(10), &mut permit_waiter)
        .await
        .map_err(|_| anyhow::anyhow!("queued permit holder did not acquire after Suspend"))?;
    // Resume is normally driven by a scheduled wakeup; inspection cannot bypass that policy.
    executor.resume(&agent, false).await?;
    tokio::select! {
        biased;
        result = &mut read => panic!("read bypassed the concurrent-agent permit: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    assert!(
        executor.worker_has_pending_startup(&owned).await,
        "reconstruction must be waiting for the held concurrent-agent permit"
    );
    worker
        .set_interrupting(InterruptKind::Suspend(Timestamp::now_utc()))
        .await;
    assert!(
        executor.worker_has_pending_startup(&owned).await,
        "Suspend waits for normal permit wakeup before startup can process it"
    );
    drop(held_permit);
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_has_pending_startup(&owned).await
            || executor.worker_is_loaded(&owned).await
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    tokio::select! {
        biased;
        result = &mut read => panic!("permit-wait Suspend discarded the read: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    executor.resume(&agent, false).await?;
    let response = tokio::time::timeout(Duration::from_secs(20), read).await??;
    assert_eq!(body(response).await?, b"after");
    write.await??;
    final_write.await??;
    assert_eq!(
        executor.get_file_contents(&agent, "/a.txt").await?.as_ref(),
        b"final"
    );
    let oplog = executor.get_oplog(&agent, before.next()).await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_))),
        "test must exercise real Suspend before inspection"
    );
    assert_eq!(count_agent_invocation_pair_since(&oplog, before), (2, 2));
    Ok(())
}

#[test]
#[timeout("2m")]
async fn grpc_read_waits_for_blocking_invocation_and_completes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        GetFileContentsRequest, get_file_contents_response,
    };
    use golem_worker_executor_test_utils::start;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let parsed = agent_id!(
        "Inspection",
        "queued-read-completion",
        "/a.txt",
        b"before".to_vec(),
        false
    );
    let agent = executor.start_agent(&component.id, parsed.clone()).await?;
    // Complete initialization before starting the invocation that blocks inspection.
    executor
        .invoke_and_await_agent(
            &component,
            &parsed,
            "replace",
            data_value!("/a.txt", b"before".to_vec(), 0u64),
        )
        .await?;
    let before = executor.oplog_max_index(&agent).await?;
    let write = {
        let executor = executor.clone();
        let component = component.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &parsed,
                    "replace",
                    data_value!("/a.txt", b"after".to_vec(), 5000u64),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = executor.get_oplog(&agent, before.next()).await.unwrap();
            if oplog.iter().any(|entry| {
                matches!(&entry.entry,
                PublicOplogEntry::AgentInvocationStarted(parameters)
                if matches!(parameters.invocation, PublicAgentInvocation::AgentMethodInvocation(_)))
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let mut client = executor.client.clone();
    let mut read = Box::pin(client.get_file_contents(GetFileContentsRequest {
        agent_id: Some(agent.clone().into()),
        component_owner_account_id: Some(component.account_id.into()),
        environment_id: Some(context.default_environment_id.into()),
        file_path: "/a.txt".into(),
        auth_ctx: Some(executor.auth_ctx().into()),
        principal: None,
        selection: Some(FileByteSelection::Full.into()),
    }));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), &mut read)
            .await
            .is_err()
    );
    assert!(
        !write.is_finished(),
        "inspection must remain pending while the write is still pending"
    );
    write.await??;
    let mut stream = tokio::time::timeout(Duration::from_secs(10), read)
        .await??
        .into_inner();
    let first = tokio::time::timeout(Duration::from_secs(10), stream.message())
        .await??
        .unwrap();
    assert!(matches!(
        first.result,
        Some(get_file_contents_response::Result::Header(_))
    ));
    let mut actual = Vec::new();
    while let Some(frame) = stream.message().await? {
        let Some(get_file_contents_response::Result::Success(bytes)) = frame.result else {
            panic!("expected file bytes, got {:?}", frame.result);
        };
        actual.extend_from_slice(&bytes);
    }
    assert_eq!(actual, b"after");
    Ok(())
}

#[test]
#[timeout("2m")]
async fn exact_file_read_grpc_rejects_unsafe_paths_before_activation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        GetFileContentsRequest, get_file_contents_response,
    };
    use golem_common::model::filesystem::FileReadError;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let parsed = agent_id!(
        "Inspection",
        "exact-path",
        "/%2e%2e",
        b"literal-percent-path".to_vec(),
        false
    );
    let agent = AgentId {
        component_id: component.id,
        agent_id: parsed.to_string(),
    };
    for path in ["", "relative", "/sub/../%2e%2e", "//%2e%2e", "/%2e%2e/"] {
        let mut stream = executor
            .client
            .clone()
            .get_file_contents(GetFileContentsRequest {
                agent_id: Some(agent.clone().into()),
                component_owner_account_id: Some(component.account_id.into()),
                environment_id: Some(context.default_environment_id.into()),
                file_path: path.into(),
                auth_ctx: Some(executor.auth_ctx().into()),
                principal: None,
                selection: Some(FileByteSelection::Full.into()),
            })
            .await?
            .into_inner();
        let first = stream.message().await?.unwrap();
        let Some(get_file_contents_response::Result::ReadFailure(error)) = first.result else {
            panic!("{path:?}: expected a typed path error, got {first:?}");
        };
        assert_eq!(
            FileReadError::try_from(error)?,
            FileReadError::InvalidTarget
        );
        assert!(stream.message().await?.is_none());
        assert!(executor.get_worker_metadata_opt(&agent).await?.is_none());
    }
    assert_eq!(
        executor
            .get_file_contents(&agent, "/%2e%2e")
            .await?
            .as_ref(),
        b"literal-percent-path"
    );
    Ok(())
}
