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

use crate::model::component::{CanonicalFilePath, ComponentRevision};
use crate::model::environment::EnvironmentId;
use crate::model::oplog::OplogIndex;
use crate::model::worker::TypedAgentConfigEntry;
use crate::model::{
    AccountId, AgentFilter, AgentId, AgentMetadata, AgentStatus, AgentStatusRecord, ComponentId,
    FilterComparator, IdempotencyKey, StringFilterComparator, Timestamp,
};
use desert_rust::BinaryCodec;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::analysed_type::str;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::vec;
use test_r::test;
use uuid::uuid;

#[test]
fn timestamp_conversion() {
    let ts: Timestamp = Timestamp::now_utc();

    let prost_ts: prost_types::Timestamp = ts.into();

    let ts2: Timestamp = prost_ts.into();

    assert_eq!(ts2, ts);
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
struct ExampleWithAccountId {
    account_id: AccountId,
}

#[test]
fn account_id_from_json_apigateway_version() {
    let json = "{ \"account_id\": \"f935056f-e2f0-4183-a40f-d8ef3011f0bc\" }";
    let example: ExampleWithAccountId = serde_json::from_str(json).unwrap();
    assert_eq!(
        example.account_id,
        AccountId(uuid!("f935056f-e2f0-4183-a40f-d8ef3011f0bc"))
    );
}

#[test]
fn account_id_json_serialization() {
    // We want to use this variant for serialization because it is used on the public API gateway API
    let example: ExampleWithAccountId = ExampleWithAccountId {
        account_id: AccountId(uuid!("f935056f-e2f0-4183-a40f-d8ef3011f0bc")),
    };
    let json = serde_json::to_string(&example).unwrap();
    assert_eq!(
        json,
        "{\"account_id\":\"f935056f-e2f0-4183-a40f-d8ef3011f0bc\"}"
    );
}

#[test]
fn worker_filter_parse() {
    assert_eq!(
        AgentFilter::from_str(" name =  worker-1").unwrap(),
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
    );

    assert_eq!(
        AgentFilter::from_str("status == Running").unwrap(),
        AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running)
    );

    assert_eq!(
        AgentFilter::from_str("revision >= 10").unwrap(),
        AgentFilter::new_revision(
            FilterComparator::GreaterEqual,
            ComponentRevision::new(10).unwrap()
        )
    );

    assert_eq!(
        AgentFilter::from_str("env.tag1 == abc ").unwrap(),
        AgentFilter::new_env(
            "tag1".to_string(),
            StringFilterComparator::Equal,
            "abc".to_string(),
        )
    );
}

#[test]
fn worker_filter_combination() {
    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).not(),
        AgentFilter::new_not(AgentFilter::new_name(
            StringFilterComparator::Equal,
            "worker-1".to_string(),
        ))
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).and(
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running)
        ),
        AgentFilter::new_and(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_status(
                FilterComparator::Equal,
                AgentStatus::Running,
            ))
            .and(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_and(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).or(
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running)
        ),
        AgentFilter::new_or(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .or(AgentFilter::new_status(
                FilterComparator::NotEqual,
                AgentStatus::Running,
            ))
            .or(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_or(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::NotEqual, AgentStatus::Running),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_status(
                FilterComparator::NotEqual,
                AgentStatus::Running,
            ))
            .or(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_or(vec![
            AgentFilter::new_and(vec![
                AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                AgentFilter::new_status(FilterComparator::NotEqual, AgentStatus::Running),
            ]),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .or(AgentFilter::new_status(
                FilterComparator::NotEqual,
                AgentStatus::Running,
            ))
            .and(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_and(vec![
            AgentFilter::new_or(vec![
                AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                AgentFilter::new_status(FilterComparator::NotEqual, AgentStatus::Running),
            ]),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );
}

#[test]
fn worker_filter_matches() {
    let component_id = ComponentId::new();
    let worker_metadata = AgentMetadata {
        agent_id: AgentId {
            agent_id: "worker-1".to_string(),
            component_id,
        },
        env: vec![
            ("env1".to_string(), "value1".to_string()),
            ("env2".to_string(), "value2".to_string()),
        ],
        environment_id: EnvironmentId::new(),
        created_by: AccountId(uuid!("f935056f-e2f0-4183-a40f-d8ef3011f0bc")),
        config: vec![TypedAgentConfigEntry {
            path: vec!["var1".to_string()],
            value: ValueAndType::new(golem_wasm::Value::String("value1".to_string()), str()),
        }],
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord {
            component_revision: ComponentRevision::new(1).unwrap(),
            ..AgentStatusRecord::default()
        },
        original_phantom_id: None,
    };

    assert!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_status(
                FilterComparator::Equal,
                AgentStatus::Idle,
            ))
            .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_env(
            "env1".to_string(),
            StringFilterComparator::Equal,
            "value1".to_string(),
        )
        .and(AgentFilter::new_status(
            FilterComparator::Equal,
            AgentStatus::Idle,
        ))
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_env(
            "env1".to_string(),
            StringFilterComparator::Equal,
            "value2".to_string(),
        )
        .not()
        .and(
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running).or(
                AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Idle)
            )
        )
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            ))
            .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-2".to_string())
            .or(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            ))
            .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_revision(
            FilterComparator::GreaterEqual,
            ComponentRevision::new(1).unwrap()
        )
        .and(AgentFilter::new_revision(
            FilterComparator::Less,
            ComponentRevision::new(2).unwrap()
        ))
        .or(AgentFilter::new_name(
            StringFilterComparator::Equal,
            "worker-2".to_string(),
        ))
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_config(
            "var1".to_string(),
            StringFilterComparator::Equal,
            "value1".to_string(),
        )
        .matches(&worker_metadata)
    );

    assert!(
        !AgentFilter::new_config(
            "var1".to_string(),
            StringFilterComparator::Equal,
            "value2".to_string(),
        )
        .matches(&worker_metadata)
    );
}

#[test]
fn derived_idempotency_key() {
    let base1 = IdempotencyKey::fresh();
    let base2 = IdempotencyKey::fresh();
    let base3 = IdempotencyKey {
        value: "base3".to_string(),
    };

    assert_ne!(base1, base2);

    let idx1 = OplogIndex::from_u64(2);
    let idx2 = OplogIndex::from_u64(11);

    let derived11a = IdempotencyKey::derived(&base1, idx1);
    let derived12a = IdempotencyKey::derived(&base1, idx2);
    let derived21a = IdempotencyKey::derived(&base2, idx1);
    let derived22a = IdempotencyKey::derived(&base2, idx2);

    let derived11b = IdempotencyKey::derived(&base1, idx1);
    let derived12b = IdempotencyKey::derived(&base1, idx2);
    let derived21b = IdempotencyKey::derived(&base2, idx1);
    let derived22b = IdempotencyKey::derived(&base2, idx2);

    let derived31 = IdempotencyKey::derived(&base3, idx1);
    let derived32 = IdempotencyKey::derived(&base3, idx2);

    assert_eq!(derived11a, derived11b);
    assert_eq!(derived12a, derived12b);
    assert_eq!(derived21a, derived21b);
    assert_eq!(derived22a, derived22b);

    assert_ne!(derived11a, derived12a);
    assert_ne!(derived11a, derived21a);
    assert_ne!(derived11a, derived22a);
    assert_ne!(derived12a, derived21a);
    assert_ne!(derived12a, derived22a);
    assert_ne!(derived21a, derived22a);

    assert_ne!(derived11a, derived31);
    assert_ne!(derived21a, derived31);
    assert_ne!(derived12a, derived32);
    assert_ne!(derived22a, derived32);
    assert_ne!(derived31, derived32);
}

#[test]
fn canonical_file_path_from_absolute() {
    let path = CanonicalFilePath::from_abs_str("/a/b/c").unwrap();
    assert_eq!(path.to_string(), "/a/b/c");
}

#[test]
fn canonical_file_path_from_relative_is_error() {
    let path = CanonicalFilePath::from_abs_str("a/b/c");
    assert!(path.is_err());
}
