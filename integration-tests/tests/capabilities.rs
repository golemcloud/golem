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

//! End-to-end round-trip of host-managed capability schema types through SDK ->
//! REST/RPC -> executor, asserting that capability material is reconstructed on
//! the receiver side while the rendering surfaces redact it (CLI display via
//! [`value_to_cli_text`], tracing/`Debug` via [`redacted_schema_value_debug`]).

use crate::Tracing;
use anyhow::anyhow;
use golem_common::model::oplog::public_oplog_entry::AgentInvocationStartedParams;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::quota::{EnforcementAction, TimePeriod};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::schema::render::value_to_cli_text;
use golem_common::schema::{
    QuotaTokenSpec, SchemaGraph, SchemaType, SchemaValue, redacted_schema_value_debug,
};
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

/// Returns the raw recorded argument record of the first
/// `AgentInvocationStarted` entry of the given agent method. The get-oplog API
/// is not a redacting surface, so capability values appear here intact.
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
/// receiver successfully reserving and making HTTP calls), while the CLI display
/// and tracing/`Debug` surfaces redact the live token snapshot.
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

    tokio::time::sleep(Duration::from_secs(1)).await;

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

    http_server.abort();

    // Reconstruction on the receiver side: the split token was rebuilt into a
    // live lease, so the receiver could reserve and issue its HTTP calls. The
    // burst capacity of 4 is shared across both agents.
    let total = received.load(Ordering::SeqCst);
    assert_eq!(
        total, 4,
        "expected 4 total HTTP calls from the shared, reconstructed quota token"
    );

    // The reconstructed token argument the receiver actually processed is
    // recorded as a real `QuotaToken` lease snapshot in its oplog. Confirm the
    // token contents were reconstructed on the receiver side...
    let receiver_oplog = user.get_oplog(&receiver, OplogIndex::INITIAL).await?;
    let input_fields = started_input_fields(&receiver_oplog, "reserve_and_call_in_loop");
    let token_value = input_fields
        .first()
        .ok_or_else(|| anyhow!("reserve_and_call_in_loop: missing token argument"))?;
    match token_value {
        SchemaValue::QuotaToken(payload) => {
            assert_eq!(payload.resource_name, "cap-rpc-rate");
            assert_eq!(payload.expected_use, 2);
        }
        other => {
            return Err(anyhow!(
                "expected a reconstructed QuotaToken, got {other:?}"
            ));
        }
    }

    // ...and that both observability surfaces redact the live token snapshot.
    let token_type = SchemaType::quota_token(QuotaTokenSpec::default());
    let graph = SchemaGraph::anonymous(token_type.clone());
    let cli_text = value_to_cli_text(&graph, &token_type, token_value)?;
    assert_eq!(
        cli_text, QUOTA_TOKEN_PLACEHOLDER,
        "reserve_and_call_in_loop: CLI rendering must redact the quota token"
    );

    let debug_text = format!("{:?}", redacted_schema_value_debug(token_value));
    assert_eq!(
        debug_text, QUOTA_TOKEN_PLACEHOLDER,
        "reserve_and_call_in_loop: tracing/Debug rendering must redact the quota token"
    );

    Ok(())
}
