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

use golem_common::model::retry_policy::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, WorkerExecutorTestDependencies,
};
use std::time::Duration;
use test_r::inherit_test_dep;

use crate::Tracing;

pub mod attempt_server;
pub mod checkout_v2_regressions;
pub mod manifest_status;
pub mod sdk_policy;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);


/// Builds a manifest-style status-code retry policy:
///
///   countBox(maxRetries = 1000, inner = periodic(<delay>))
///   predicate: status-code in {500, 502, 503, 504}
pub(crate) fn manifest_http_5xx_retry_policy(name: &str, delay: Duration) -> NamedRetryPolicy {
    NamedRetryPolicy {
        name: name.to_string(),
        priority: 20,
        predicate: Predicate::PropIn {
            property: "status-code".to_string(),
            values: vec![
                PredicateValue::Integer(500),
                PredicateValue::Integer(502),
                PredicateValue::Integer(503),
                PredicateValue::Integer(504),
            ],
        },
        policy: RetryPolicy::CountBox {
            max_retries: 1000,
            inner: Box::new(RetryPolicy::Periodic(delay)),
        },
    }
}
