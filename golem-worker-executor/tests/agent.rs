// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::let_assert;
use assert2::{assert, check};
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;
use golem_api_grpc::proto::golem::worker::v1::{
    worker_execution_error, InvocationFailed, WorkerExecutionError,
};
use golem_api_grpc::proto::golem::worker::{UnknownError, WorkerError};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::IntoValueAndType;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn agent_self_rpc_is_not_allowed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap().into_admin().await;

    let component_id = executor.component("golem_it_agent_self_rpc").store().await;
    let worker_id = executor
        .start_worker(&component_id, "self-rpc-agent(\"worker-name\")")
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-self-rpc/self-rpc-agent.{self-rpc}",
            vec![],
        )
        .await;

    let_assert!(
        Err(Error::InternalError(WorkerExecutionError {
            error: Some(worker_execution_error::Error::InvocationFailed(
                InvocationFailed {
                    error: Some(WorkerError {
                        error: Some(
                            golem_api_grpc::proto::golem::worker::worker_error::Error::UnknownError(
                                UnknownError {
                                    details: error_details
                                }
                            )
                        )
                    }),
                    ..
                }
            ))
        })) = result
    );
    assert!(error_details.contains("RPC calls to the same agent are not supported"));
}

#[test]
#[tracing::instrument]
async fn agent_await_parallel_rpc_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap().into_admin().await;

    let component_id = executor
        .component("golem_it_agent_rpc")
        .name("golem-it:agent-rpc")
        .store()
        .await;
    let unique_id = context.redis_prefix();
    let worker_id = executor
        .start_worker(&component_id, &format!("test-agent(\"${unique_id}\")"))
        .await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem-it:agent-rpc/test-agent.{run}",
            vec![20f64.into_value_and_type()],
        )
        .await;

    executor.check_oplog_is_queryable(&worker_id).await;

    check!(result.is_ok());
}
