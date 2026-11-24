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

//! A Go agent reads the divergence of a failed update from its own oplog through
//! `GetOplog` and `SearchOplog`, with the request of each host call when it is
//! present and none when it is absent.

use crate::Tracing;
use golem_common::model::oplog::public_oplog_entry::StartParams;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PublicOplogEntry, ReplayDivergence, ReplayHostCall, ReplayStep,
    StartIdentityField, UpdateFailureClass,
};
use golem_common::schema::{FromSchema, IntoTypedSchemaValue};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::HasOplog;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

#[derive(Debug, PartialEq, FromSchema)]
#[schema(rename_all = "camelCase")]
struct Call {
    function_name: String,
    carries_request: bool,
    request: Option<String>,
}

#[derive(Debug, PartialEq, FromSchema)]
#[schema(rename_all = "camelCase")]
struct Step {
    kind: u8,
    call: Option<Call>,
}

#[derive(Debug, PartialEq, FromSchema)]
#[schema(rename_all = "camelCase")]
struct Divergence {
    entry_index: u64,
    failure_class: u8,
    oplog_index: u64,
    expected: Step,
    actual: Step,
    mismatched: Vec<u8>,
    requests_differ: bool,
}

#[derive(Debug, FromSchema)]
#[schema(rename_all = "camelCase")]
struct Report {
    from_get: Vec<Divergence>,
    from_search: Vec<Divergence>,
}

/// The replay-step case tag of a host call, and the WIT case indexes of the failure class
/// `divergence` and of the identity fields `function-name` and `request-presence`.
const HOST_CALL: u8 = 0;
const CLASS_DIVERGENCE: u8 = 0;
const FIELD_FUNCTION_NAME: u8 = 0;
const FIELD_REQUEST_PRESENCE: u8 = 4;

fn host_call(
    recorded: &StartParams,
    function_name: &str,
    carries_request: bool,
    request: Option<&str>,
) -> ReplayStep {
    ReplayStep::HostCall(ReplayHostCall {
        function_name: function_name.to_string(),
        durable_function_type: recorded.durable_function_type.clone(),
        parent_start_index: None,
        invocation_id: None,
        observational_owner: None,
        driver: None,
        carries_request,
        request: request
            .map(|request| Box::new(request.to_string().into_typed_schema_value().unwrap())),
    })
}

/// Two failed updates follow a recorded `Start`. The first names that `Start` as expected,
/// without a request of its own, and a replayed call with a different request as actual. The
/// second has no request on either side. The Go agent sees the recorded request of the `Start`
/// and the replayed request in the first, none in the second, and the same through both oplog
/// readers.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_reads_the_requests_of_a_failed_update_divergence(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_key!("DivergenceReaderAgent", "go-divergence-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // The first read records the `Start` of the search, whose request holds the search text.
    let before = executor
        .invoke_and_await_agent(&component, &agent_id, "read", data_value!())
        .await?
        .into_typed::<Report>()?;
    assert!(before.from_get.is_empty() && before.from_search.is_empty());
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let (recorded_index, recorded) = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params
                    .request
                    .as_ref()
                    .is_some_and(|request| format!("{request:?}").contains("failed-update")) =>
            {
                Some((entry.oplog_index, params.clone()))
            }
            _ => None,
        })
        .expect("the oplog records the Start of the search");

    let both = ReplayDivergence {
        oplog_index: recorded_index,
        expected: host_call(&recorded, &recorded.function_name, true, None),
        actual: host_call(&recorded, "replayed::call", true, Some("replayed-request")),
        mismatched: vec![StartIdentityField::FunctionName],
    };
    let neither = ReplayDivergence {
        oplog_index: recorded_index,
        expected: host_call(&recorded, "recorded::scope", false, None),
        actual: host_call(&recorded, "recorded::scope", true, None),
        mismatched: vec![StartIdentityField::RequestPresence],
    };
    let worker_oplog = executor
        .active_agent(&worker_id)
        .await
        .expect("the agent is active")
        .primary()
        .oplog();
    let mut entry_indexes = Vec::new();
    for divergence in [both, neither] {
        entry_indexes.push(
            worker_oplog
                .add_and_commit(OplogEntry::failed_update(
                    component.revision,
                    Some("diverged".to_string()),
                    None,
                    None,
                    None,
                    UpdateFailureClass::Divergence,
                    Some(Box::new(divergence)),
                ))
                .await?,
        );
    }

    let report = executor
        .invoke_and_await_agent(&component, &agent_id, "read", data_value!())
        .await?
        .into_typed::<Report>()?;

    assert_eq!(report.from_get, report.from_search);
    let [both, neither] = report.from_get.as_slice() else {
        panic!("the agent reads both failed updates: {report:?}");
    };
    assert_eq!(both.entry_index, entry_indexes[0].as_u64());
    assert_eq!(both.failure_class, CLASS_DIVERGENCE);
    assert_eq!(both.oplog_index, recorded_index.as_u64());
    assert_eq!(both.mismatched, vec![FIELD_FUNCTION_NAME]);
    assert!(both.requests_differ);
    let expected = both.expected.call.as_ref().expect("a host call");
    let actual = both.actual.call.as_ref().expect("a host call");
    assert_eq!(
        (both.expected.kind, both.actual.kind),
        (HOST_CALL, HOST_CALL)
    );
    assert_eq!(expected.function_name, recorded.function_name);
    assert_eq!(actual.function_name, "replayed::call");
    let expected_request = expected.request.as_deref().expect("the recorded request");
    let actual_request = actual.request.as_deref().expect("the replayed request");
    assert!(
        expected_request.contains("failed-update") && !expected_request.contains("replayed"),
        "{expected_request}"
    );
    assert!(
        actual_request.contains("replayed-request") && !actual_request.contains("failed-update"),
        "{actual_request}"
    );

    assert_eq!(neither.entry_index, entry_indexes[1].as_u64());
    assert_eq!(neither.mismatched, vec![FIELD_REQUEST_PRESENCE]);
    assert!(!neither.requests_differ);
    assert_eq!(
        neither.expected.call,
        Some(Call {
            function_name: "recorded::scope".to_string(),
            carries_request: false,
            request: None,
        })
    );
    assert_eq!(
        neither.actual.call,
        Some(Call {
            function_name: "recorded::scope".to_string(),
            carries_request: true,
            request: None,
        })
    );
    Ok(())
}
