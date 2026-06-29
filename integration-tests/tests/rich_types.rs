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

//! End-to-end round-trip of the rich semantic schema types (Path, Url,
//! Datetime, Duration, Quantity) through SDK -> REST -> executor -> oplog ->
//! SDK, asserting canonical-encoding preservation (Task 30, item 4).

use crate::Tracing;
use anyhow::anyhow;
use chrono::{TimeZone, Utc};
use golem_common::model::oplog::public_oplog_entry::{
    AgentInvocationFinishedParams, AgentInvocationStartedParams,
};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicAgentInvocationResult, PublicOplogEntry,
    PublicOplogEntryWithIndex,
};
use golem_common::schema::{
    Quantity, QuantityUnit, SchemaType, SchemaValue, TypeId, TypedSchemaValue,
};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use std::path::PathBuf;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

/// Quantity unit matching the test component's `Bytes` unit (base unit `B`).
struct Bytes;

impl QuantityUnit for Bytes {
    fn type_id() -> TypeId {
        TypeId::new("golem.it.agent_sdk_rust.Bytes")
    }
    fn base_unit() -> &'static str {
        "B"
    }
}

fn canonical_of(v: &SchemaValue) -> String {
    use golem_common::schema::canonical;
    match v {
        SchemaValue::Path { path } => canonical::path::to_text(path).expect("path canonical"),
        SchemaValue::Url { url } => canonical::url::to_text(url).expect("url canonical"),
        SchemaValue::Datetime { value } => {
            canonical::datetime::to_text(value).expect("datetime canonical")
        }
        SchemaValue::Duration(p) => canonical::duration::to_text(p),
        SchemaValue::Quantity(q) => canonical::quantity::to_text(q).expect("quantity canonical"),
        other => panic!("not a rich semantic value: {other:?}"),
    }
}

fn started_input<'a>(oplog: &'a [PublicOplogEntryWithIndex], method: &str) -> &'a TypedSchemaValue {
    oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                invocation: PublicAgentInvocation::AgentMethodInvocation(params),
                ..
            }) if params.method_name == method => Some(&params.function_input),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no AgentInvocationStarted entry for {method}"))
}

fn finished_output<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    method: &str,
) -> &'a TypedSchemaValue {
    oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                result: PublicAgentInvocationResult::AgentMethod(out),
                method_name: Some(m),
                ..
            }) if m == method => Some(&out.output),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no AgentInvocationFinished entry for {method}"))
}

fn single_rich_arg(typed: &TypedSchemaValue) -> (&SchemaType, &SchemaValue) {
    let ty = match typed.root_type() {
        SchemaType::Record { fields, .. } => {
            assert_eq!(fields.len(), 1, "expected single-argument input record");
            &fields[0].body
        }
        other => panic!("expected record input type, got {other:?}"),
    };
    let value = match typed.value() {
        SchemaValue::Record { fields } => {
            assert_eq!(fields.len(), 1, "expected single-argument input record");
            &fields[0]
        }
        other => panic!("expected record input value, got {other:?}"),
    };
    (ty, value)
}

fn assert_round_trip(
    oplog: &[PublicOplogEntryWithIndex],
    method: &str,
    returned: &SchemaValue,
    is_rich_type: fn(&SchemaType) -> bool,
    is_rich_value: fn(&SchemaValue) -> bool,
) {
    assert!(
        is_rich_value(returned),
        "{method}: returned value is not the expected rich variant: {returned:?}"
    );

    let started = started_input(oplog, method);
    let (in_ty, in_val) = single_rich_arg(started);
    assert!(
        is_rich_type(in_ty),
        "{method}: oplog input type is not the expected rich variant: {in_ty:?}"
    );
    assert!(
        is_rich_value(in_val),
        "{method}: oplog input value is not the expected rich variant: {in_val:?}"
    );

    let finished = finished_output(oplog, method);
    assert!(
        is_rich_type(finished.root_type()),
        "{method}: oplog output type is not the expected rich variant: {:?}",
        finished.root_type()
    );
    assert!(
        is_rich_value(finished.value()),
        "{method}: oplog output value is not the expected rich variant: {:?}",
        finished.value()
    );

    let canonical_in = canonical_of(in_val);
    let canonical_returned = canonical_of(returned);
    let canonical_out = canonical_of(finished.value());
    assert_eq!(
        canonical_in, canonical_returned,
        "{method}: canonical encoding differs between oplog input and returned value"
    );
    assert_eq!(
        canonical_in, canonical_out,
        "{method}: canonical encoding differs between oplog input and oplog output"
    );
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn rich_semantic_types_round_trip_preserves_canonical_encoding(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let component = user
        .component(&env.id, "golem_it_agent_sdk_rust_release")
        .name("golem-it:agent-sdk-rust")
        .store()
        .await?;

    let parsed_agent_id = agent_id!("RichTypesAgent", "rich-types-1");
    let agent_id = user
        .start_agent(&component.id, parsed_agent_id.clone())
        .await?;

    let path = PathBuf::from("/tmp/golem-rich-types/α.txt");
    let url = url::Url::parse("https://example.com/a/b?x=1#frag")?;
    let datetime = Utc
        .timestamp_opt(1_700_000_000, 123_456_789)
        .single()
        .ok_or_else(|| anyhow!("failed to build test datetime"))?;
    let duration = Duration::from_nanos(93_784_005_006);
    let quantity = Quantity::<Bytes>::new(123, 1, "B").map_err(|e| anyhow!("{e:?}"))?;

    let returned_path = user
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "echo_path",
            data_value!(path.clone()),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("echo_path: expected a return value"))?;

    let returned_url = user
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "echo_url",
            data_value!(url.clone()),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("echo_url: expected a return value"))?;

    let returned_datetime = user
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "echo_datetime",
            data_value!(datetime),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("echo_datetime: expected a return value"))?;

    let returned_duration = user
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "echo_duration",
            data_value!(duration),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("echo_duration: expected a return value"))?;

    let returned_quantity = user
        .invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "echo_quantity",
            data_value!(quantity.clone()),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("echo_quantity: expected a return value"))?;

    let oplog = user.get_oplog(&agent_id, OplogIndex::INITIAL).await?;

    assert_round_trip(
        &oplog,
        "echo_path",
        &returned_path,
        |t| matches!(t, SchemaType::Path { .. }),
        |v| matches!(v, SchemaValue::Path { .. }),
    );
    assert_round_trip(
        &oplog,
        "echo_url",
        &returned_url,
        |t| matches!(t, SchemaType::Url { .. }),
        |v| matches!(v, SchemaValue::Url { .. }),
    );
    assert_round_trip(
        &oplog,
        "echo_datetime",
        &returned_datetime,
        |t| matches!(t, SchemaType::Datetime { .. }),
        |v| matches!(v, SchemaValue::Datetime { .. }),
    );
    assert_round_trip(
        &oplog,
        "echo_duration",
        &returned_duration,
        |t| matches!(t, SchemaType::Duration { .. }),
        |v| matches!(v, SchemaValue::Duration(_)),
    );
    assert_round_trip(
        &oplog,
        "echo_quantity",
        &returned_quantity,
        |t| matches!(t, SchemaType::Quantity { .. }),
        |v| matches!(v, SchemaValue::Quantity(_)),
    );

    Ok(())
}
