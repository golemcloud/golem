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

//! End-to-end RPC round-trip of host-managed capabilities, asserting that
//! received capabilities remain usable while external operation logs redact them.

use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::oplog::public_oplog_entry::AgentInvocationStartedParams;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::quota::{EnforcementAction, TimePeriod};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::schema::SchemaValue;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

const QUOTA_TOKEN_PLACEHOLDER: &str = "<redacted: quota-token>";

/// Returns the externally redacted argument record of the first
/// `AgentInvocationStarted` entry of the given agent method.
fn started_input_fields<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    method: &str,
) -> &'a [SchemaValue] {
    let typed = oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                invocation: PublicAgentInvocation::AgentMethodInvocation(params),
                ..
            }) if params.method_name == method => Some(&params.function_input),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no AgentInvocationStarted entry for {method}"));
    match typed.value() {
        SchemaValue::Record { fields } => fields,
        other => panic!("expected record input value for {method}, got {other:?}"),
    }
}

/// A `QuotaToken` capability split off and sent to a second agent over RPC must
/// be reconstructed into a live lease on the receiver side (proven by the
/// receiver successfully reserving and making HTTP calls), while the external
/// operation log redacts the live token snapshot.
#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn quota_token_capability_round_trips_and_is_redacted(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    crate::quota::provision_rate_resource(
        &client,
        &env.id.0,
        "cap-rpc-rate",
        4,
        4,
        TimePeriod::Hour,
        EnforcementAction::Throttle,
    )
    .await?;

    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    let http_server = tokio::spawn(async move {
        use axum::{Router, routing::get};
        let route = Router::new().route(
            "/call",
            get(move || {
                let cnt = received_clone.clone();
                async move {
                    cnt.fetch_add(1, Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let component = user
        .component(&env.id, "golem_it_agent_sdk_rust_release")
        .name("golem-it:agent-sdk-rust")
        .store()
        .await?;

    let sender = agent_id!("QuotaRpcSender", "cap-rpc-sender");
    let sender_sys = user.start_agent(&component.id, sender.clone()).await?;

    user.invoke_agent(
        &component,
        &sender,
        "split_and_loop",
        data_value!(
            "cap-rpc-rate".to_string(),
            4u64,
            2u64,
            "localhost".to_string(),
            port,
            4u64
        ),
    )
    .await?;

    crate::quota::wait_for_http_request_count(received.as_ref(), 4, Duration::from_secs(60))
        .await?;

    user.wait_for_statuses(
        &sender_sys,
        &[AgentStatus::Idle, AgentStatus::Suspended],
        Duration::from_secs(60),
    )
    .await?;

    let receiver = AgentId {
        component_id: component.id,
        agent_id: "QuotaRpcReceiver(\"cap-rpc-sender-receiver\")".to_string(),
    };
    user.wait_for_statuses(
        &receiver,
        &[AgentStatus::Idle, AgentStatus::Suspended],
        Duration::from_secs(60),
    )
    .await?;

    tokio::time::timeout(Duration::from_secs(60), async {
        while received.load(Ordering::SeqCst) < 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("timed out waiting for all quota-token HTTP calls"))?;

    http_server.abort();

    // Reconstruction on the receiver side: the split token was rebuilt into a
    // live lease, so the receiver could reserve and issue its HTTP calls. The
    // burst capacity of 4 is shared across both agents.
    let total = received.load(Ordering::SeqCst);
    assert_eq!(
        total, 4,
        "expected 4 total HTTP calls from the shared, reconstructed quota token"
    );

    // The external operation log must not expose the authoritative snapshot
    // even though the receiver successfully used the transferred token.
    let receiver_oplog = user.get_oplog(&receiver, OplogIndex::INITIAL).await?;
    let input_fields = started_input_fields(&receiver_oplog, "reserve_and_call_in_loop");
    let token_value = input_fields
        .first()
        .ok_or_else(|| anyhow!("reserve_and_call_in_loop: missing token argument"))?;
    assert_eq!(
        token_value,
        &SchemaValue::String(QUOTA_TOKEN_PLACEHOLDER.to_string()),
        "reserve_and_call_in_loop: external operation log must redact the quota token"
    );

    Ok(())
}
