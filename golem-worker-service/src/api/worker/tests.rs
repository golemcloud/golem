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

use super::{WorkerApi, oplog_scan_allowance};
use crate::api::common::ApiEndpointError;
use crate::mcp::OplogRead;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::{AgentKey, AgentMode};
use golem_common::model::oplog::{OplogCursor, OplogIndex, OplogRange, OplogScanAllowance};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
use golem_service_base::model::auth::AuthCtx;
use test_r::test;

#[test]
fn a_scan_allowance_needs_both_parameters_and_a_range_read() {
    let allowance = OplogScanAllowance {
        entries: 7,
        millis: 9,
    };
    let needs_to = "The 'scanEntries' and 'scanMillis' parameters need the 'to' parameter";
    let no_query = "Cannot specify both the 'scanEntries' and 'scanMillis' parameters and the 'query' parameter";
    let together = "The 'scanEntries' and 'scanMillis' parameters go together";
    [
        ((None, None, None, None), Ok(None)),
        ((None, Some(9), None, None), Ok(None)),
        ((Some("error"), Some(9), None, None), Ok(None)),
        ((None, Some(9), Some(7), Some(9)), Ok(Some(allowance))),
        ((None, None, Some(7), Some(9)), Err(needs_to)),
        ((Some("error"), None, Some(7), Some(9)), Err(no_query)),
        ((Some("error"), Some(9), Some(7), Some(9)), Err(no_query)),
        ((None, Some(9), Some(7), None), Err(together)),
        ((None, Some(9), None, Some(9)), Err(together)),
    ]
    .into_iter()
    .for_each(|((query, to, entries, millis), expected)| {
        assert_eq!(
            oplog_scan_allowance(query, to, entries, millis).map_err(|error| match error {
                ApiEndpointError::BadRequest(body) => body.0.errors.join(" "),
                _ => "not a bad request".to_string(),
            }),
            expected.map_err(str::to_string),
            "query {query:?}, to {to:?}, scanEntries {entries:?}, scanMillis {millis:?}"
        )
    });
}

#[test]
async fn an_oplog_query_reads_its_range_and_a_read_without_a_query_reads_its_range() {
    let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
    let (worker_service, auth_service) = harness.rest_api_services();
    let api = WorkerApi::new(worker_service, auth_service, Default::default());
    let key = AgentKey::new(
        harness.agent_type.type_name.clone(),
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(vec![])),
            SchemaValue::Record { fields: vec![] },
        ),
    )
    .unwrap();
    let agent_id = OwnedAgentId::new(harness.environment_id, &harness.add_keyed_agent(&key));
    let index = OplogIndex::from_u64;
    let allowance = OplogScanAllowance {
        entries: 7,
        millis: 9,
    };
    let read = |from, to, query: Option<&str>, allowance, cursor| {
        api.get_oplog_internal(
            agent_id.clone(),
            from,
            to,
            50,
            cursor,
            query.map(str::to_string),
            None,
            allowance,
            Default::default(),
            AuthCtx::system(),
        )
    };

    read(Some(3), Some(9), Some("error"), None, None)
        .await
        .unwrap();
    read(Some(4), None, None, None, None).await.unwrap();
    // A read without `from` starts at index 1. The scan allowance of a range read goes to the
    // executor.
    read(None, Some(9), None, Some(allowance), None)
        .await
        .unwrap();
    let scanning = OplogCursor {
        next_oplog_index: 2,
        current_component_revision: 1,
        query_hash: OplogCursor::query_hash(None),
        scan_to: Some(5),
    };
    read(None, Some(9), None, None, Some(scanning))
        .await
        .unwrap();
    // A range that ends before its start is a bad request, with and without a query.
    assert!(matches!(
        read(Some(5), Some(4), Some("error"), None, None).await,
        Err(ApiEndpointError::BadRequest(_))
    ));
    assert!(matches!(
        read(Some(5), Some(4), None, None, None).await,
        Err(ApiEndpointError::BadRequest(_))
    ));

    assert_eq!(
        *harness.oplog_reads.lock().unwrap(),
        vec![
            OplogRead {
                range: OplogRange::new(Some(index(3)), Some(index(9))).unwrap(),
                query: Some("error".into()),
                allowance: None,
            },
            OplogRead {
                range: OplogRange::new(Some(index(4)), None).unwrap(),
                query: None,
                allowance: None,
            },
            OplogRead {
                range: OplogRange::new(Some(index(1)), Some(index(9))).unwrap(),
                query: None,
                allowance: Some(allowance),
            },
            OplogRead {
                range: OplogRange::new(None, Some(index(9))).unwrap(),
                query: None,
                allowance: None,
            },
        ]
    );
}
