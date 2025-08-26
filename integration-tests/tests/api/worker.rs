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

use crate::Tracing;
use assert2::{assert, check, let_assert};
use futures_concurrency::future::Join;
use golem_api_grpc::proto::golem::worker::v1::{
    invoke_and_await_response, launch_new_worker_response, ConnectWorkerRequest,
    InvokeAndAwaitResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse,
    LaunchNewWorkerSuccessResponse,
};
use golem_api_grpc::proto::golem::worker::{log_event, InvokeResult, LogEvent, WorkerId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tracing::info;
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn add_and_invoke_worker_with_args_and_env(deps: &EnvBasedTestDependencies) {
    let admin = deps.admin().await;

    let (component_id, _) = admin
        .component("environment-service")
        .unique()
        .store_and_get_name()
        .await;

    let component_version = admin
        .update_component(&component_id, "environment-service")
        .await;

    check!(component_version == 1);

    let create_result = deps
        .worker_service()
        .create_worker(
            &admin.token,
            LaunchNewWorkerRequest {
                component_id: Some(component_id.clone().into()),
                name: format!("worker-{}", Uuid::new_v4()),
                args: vec!["test-arg".to_string()],
                env: HashMap::from([
                    ("TEST_ENV_VAR_1".to_string(), "value_1".to_string()),
                    ("TEST_ENV_VAR_2".to_string(), "value_2".to_string()),
                ]),
                wasi_config_vars: Some(BTreeMap::new().into()),
                ignore_already_existing: false,
            },
        )
        .await
        .unwrap()
        .unwrap();

    check!(create_result.component_version == component_version);

    let result: Vec<Value> = deps
        .worker_service()
        .invoke_and_await(
            &admin.token,
            WorkerId {
                component_id: Some(component_id.clone().into()),
                name: create_result.worker_id.as_ref().unwrap().name.to_string(),
            },
            None,
            "golem:it/api.{get-arguments}".to_string(),
            vec![],
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .result
        .into_iter()
        .map(|v| Value::try_from(v).unwrap())
        .collect::<Vec<_>>();

    check!(
        vec![Value::Result(Ok(Some(Box::new(Value::List(vec![
            Value::String("test-arg".to_string(),)
        ])))))]
            == result
    );

    let result: Vec<Value> = deps
        .worker_service()
        .invoke_and_await(
            &admin.token,
            WorkerId {
                component_id: Some(component_id.clone().into()),
                name: create_result.worker_id.as_ref().unwrap().name.to_string(),
            },
            None,
            "golem:it/api.{get-environment}".to_string(),
            vec![],
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .result
        .into_iter()
        .map(|v| Value::try_from(v).unwrap())
        .collect::<Vec<_>>();

    assert!(result.len() == 1);

    let_assert!(Value::Result(Ok(Some(ok))) = &result[0]);
    let_assert!(Value::List(env_vars) = ok.as_ref());
    let env_vars = env_vars
        .iter()
        .map(|env_var| {
            let_assert!(Value::Tuple(elems) = env_var);
            let_assert!([Value::String(key), Value::String(value)] = elems.as_slice());
            (key.to_owned(), value.to_owned())
        })
        .collect::<HashMap<_, _>>();

    info!("env vars: {:?}", env_vars);
    check!(env_vars.get("GOLEM_COMPONENT_VERSION") == Some(&"1".to_string()));
    check!(env_vars.get("GOLEM_COMPONENT_ID") == Some(&component_id.0.to_string()));
    check!(
        env_vars.get("GOLEM_WORKER_NAME") == Some(&create_result.worker_id.as_ref().unwrap().name)
    );
    check!(env_vars.get("TEST_ENV_VAR_1") == Some(&"value_1".to_string()));
    check!(env_vars.get("TEST_ENV_VAR_2") == Some(&"value_2".to_string()));
}

#[test]
#[tracing::instrument]
async fn stream_high_volume_log_output(deps: &EnvBasedTestDependencies) {
    let user = deps.user().await;

    let component_id = user
        .component("golem_it_high_volume_logging")
        .unique()
        .store()
        .await;

    let worker = user.start_worker(&component_id, "worker-1").await;

    let mut output_stream = deps
        .worker_service()
        .connect_worker(
            &user.token,
            ConnectWorkerRequest {
                worker_id: Some(worker.clone().into()),
            },
        )
        .await
        .unwrap();

    // simulate a slow consumer
    let output_consumer = async {
        loop {
            let event = output_stream.message().await.unwrap();
            if let Some(LogEvent {
                event: Some(log_event::Event::Stdout(inner)),
            }) = event
            {
                if inner.message.contains("Iteration 100") {
                    break true;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    };

    let result_future = user.invoke_and_await(
        &worker,
        "golem-it:high-volume-logging-exports/golem-it-high-volume-logging-api.{run}",
        vec![],
    );

    let (found_log_entry, result) = (output_consumer, result_future).join().await;
    result.unwrap();

    assert!(found_log_entry)
}

trait Unwrap {
    type Inner;

    fn unwrap(self) -> Self::Inner;
}

impl Unwrap for LaunchNewWorkerResponse {
    type Inner = LaunchNewWorkerSuccessResponse;

    fn unwrap(self) -> Self::Inner {
        match self.result {
            None => {
                panic!("empty response for LaunchNewWorker");
            }
            Some(launch_new_worker_response::Result::Success(result)) => result,
            Some(launch_new_worker_response::Result::Error(error)) => {
                panic!("{error:?}");
            }
        }
    }
}

impl Unwrap for InvokeAndAwaitResponse {
    type Inner = InvokeResult;

    fn unwrap(self) -> Self::Inner {
        match self.result {
            None => panic!("empty response for InvokeAndAwait"),
            Some(invoke_and_await_response::Result::Success(result)) => result,
            Some(invoke_and_await_response::Result::Error(error)) => panic!("{error:?}"),
        }
    }
}
