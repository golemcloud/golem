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

//! A recorded permission card that does not decode is corrupt history of its agent. Replay of
//! the card-liveness call that recorded it fails the agent with the read error. It does not give
//! the guest a denial, and it writes no entry.

use super::*;
use crate::unreadable_call_paths::{StoredOplogRows, decoded_after, outcome_entries};
use golem_common::model::oplog::payload::types::SerializablePermissionCardLiveness;
use golem_common::model::oplog::{HostResponsePermissionCardLiveness, OplogPayload};
use golem_worker_executor::services::golem_config::OplogConfig;
use test_r::{test, timeout};

/// A scope-card executor that keeps every entry in the primary oplog.
async fn start_primary_only_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: Arc<ScopeCardAuthority>,
) -> anyhow::Result<TestWorkerExecutor> {
    let oplog = OplogConfig {
        indexed_storage_layers: 1,
        blob_storage_layers: 0,
        ..Default::default()
    };
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(move |runtime| {
                runtime.oplog = oplog.clone();
                runtime.suspend.suspend_after = Duration::from_secs(300);
            })),
            create_card_service: Some(Arc::new(move || {
                Arc::new(ScopeCardService {
                    authority: authority.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await
}

/// The index of the last recorded `card-liveness` `End` among `rows`, the index of its `Start`,
/// and the stored bytes of the `End` with its card made undecodable.
fn last_liveness_end_with_an_undecodable_card(
    rows: &[(u64, Vec<u8>)],
) -> (u64, OplogIndex, Vec<u8>) {
    let entries = decoded_after(rows, 0);
    let liveness_starts = entries
        .iter()
        .filter_map(|(index, entry)| match entry {
            OplogEntry::Start { function_name, .. }
                if *function_name == HostFunctionName::GolemPermissionsCardLiveness =>
            {
                Some(OplogIndex::from_u64(*index))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let (index, mut end) = entries
        .into_iter()
        .rev()
        .find(|(_, entry)| {
            matches!(entry, OplogEntry::End { start_index, .. } if liveness_starts.contains(start_index))
        })
        .expect("the agent recorded a card-liveness call");
    let OplogEntry::End {
        start_index,
        response: Some(response),
        ..
    } = &mut end
    else {
        panic!("a card-liveness End has a response: {end:?}");
    };
    let OplogPayload::SerializedInline { bytes, .. } = response else {
        panic!("the card-liveness response is stored inline: {response:?}");
    };
    let HostResponse::PermissionCardLiveness(HostResponsePermissionCardLiveness {
        liveness: SerializablePermissionCardLiveness::Live(mut card),
    }) = golem_common::serialization::decode::<HostResponse>(bytes)
        .expect("the stored response decodes")
    else {
        panic!("the last card-liveness call recorded a live card");
    };
    card[0] = 255;
    *response = OplogPayload::SerializedInline {
        bytes: golem_common::serialization::serialize(&HostResponse::PermissionCardLiveness(
            HostResponsePermissionCardLiveness {
                liveness: SerializablePermissionCardLiveness::Live(card),
            },
        ))
        .expect("the response encodes"),
        cached: None,
    };
    let start_index = *start_index;
    (
        index,
        start_index,
        golem_common::serialization::serialize(&end).expect("the entry encodes"),
    )
}

/// The holder uses its kept child card, so its oplog records the live card. That recorded card
/// is then made undecodable. The next invocation replays the use: the agent fails with the read
/// error at the call, the guest does not get `card-revoked` or another denial, and the oplog gets
/// no entry that records an outcome.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_undecodable_recorded_card_fails_replay_without_a_recorded_denial(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let executor = start_primary_only_executor(deps, &context, authority.clone()).await?;
    let (component, holder, holder_worker) = start_card_handle_agent(
        &executor,
        &authority,
        &context,
        host_api_tests,
        "undecodable-card-holder",
    )
    .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &holder,
            "keep_derived_cards",
            data_value!(None::<String>),
        )
        .await?;
    assert_eq!(
        use_kept_child(&executor, &component, &holder).await?,
        vec!["ok", "ok"]
    );
    executor
        .wait_for_status(&holder_worker, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    executor.release().await?;

    let stored = StoredOplogRows::open(deps, &context, &holder_worker).await?;
    let rows = stored.rows().await?;
    let head = rows.last().expect("the holder has an oplog").0;
    let (corrupt_index, corrupt_start, corrupt_end) =
        last_liveness_end_with_an_undecodable_card(&rows);
    stored.replace(corrupt_index, &corrupt_end).await?;
    let corrupt_rows = stored.rows().await?;

    let executor = start_primary_only_executor(deps, &context, authority.clone()).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent(&component, &holder, "use_kept_child", data_value!()),
    )
    .await?;
    let error = match result {
        Ok(outcome) => panic!(
            "replay of an undecodable card must not give the guest an outcome: {:?}",
            outcome.into_typed::<Vec<String>>()
        ),
        Err(error) => format!("{error:#}"),
    };
    assert!(
        error.contains("permission card")
            && error.contains(&format!("oplog index {corrupt_start}"))
            && error.contains("unknown serialization version: 255")
            && error.contains(&holder_worker.agent_id.to_string()),
        "the agent must fail with the read error of the recorded card: {error}"
    );
    assert!(!executor.worker_is_loaded(&holder_worker).await);

    let after = stored.rows().await?;
    assert_eq!(
        after
            .iter()
            .filter(|(index, _)| *index <= head)
            .cloned()
            .collect::<Vec<_>>(),
        corrupt_rows,
        "the failed replay must not change the recorded history"
    );
    let appended = decoded_after(&after, head);
    assert!(
        outcome_entries(&appended).is_empty(),
        "the failed replay must not record an outcome: {appended:?}"
    );
    Ok(())
}
