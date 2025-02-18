use crate::Tracing;
use assert2::{assert, check};
use golem_api_grpc::proto::golem::worker::v1::{
    invoke_and_await_response, launch_new_worker_response, InvokeAndAwaitResponse,
    LaunchNewWorkerRequest, LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse,
};
use golem_api_grpc::proto::golem::worker::{InvokeResult, TargetWorkerId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn add_and_invoke_worker_with_args_and_env(deps: &EnvBasedTestDependencies) {
    let (component_id, _) = deps
        .component("environment-service")
        .unique()
        .store_and_get_name()
        .await;
    let component_version = deps
        .update_component(&component_id, "environment-service")
        .await;
    check!(component_version == 1);

    let create_result = deps
        .worker_service()
        .create_worker(LaunchNewWorkerRequest {
            component_id: Some(component_id.clone().into()),
            name: format!("worker-{}", Uuid::new_v4()),
            args: vec!["test-arg".to_string()],
            env: HashMap::from([
                ("TEST_ENV_VAR_1".to_string(), "value_1".to_string()),
                ("TEST_ENV_VAR_2".to_string(), "value_2".to_string()),
            ]),
        })
        .await
        .unwrap()
        .unwrap();

    check!(create_result.component_version == component_version);

    let result: Vec<Value> = deps
        .worker_service()
        .invoke_and_await(
            TargetWorkerId {
                component_id: Some(component_id.clone().into()),
                name: Some(create_result.worker_id.as_ref().unwrap().name.to_string()),
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
            TargetWorkerId {
                component_id: Some(component_id.clone().into()),
                name: Some(create_result.worker_id.as_ref().unwrap().name.to_string()),
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
    let env_vars = match &result[0] {
        Value::Result(Ok(Some(ok))) => match ok.as_ref() {
            Value::List(env_vars) => env_vars
                .iter()
                .map(|env_var| match env_var {
                    Value::Tuple(elems) => match elems.as_slice() {
                        [Value::String(key), Value::String(value)] => (key.clone(), value.clone()),
                        _ => panic!("expected Tuple(String, String), got {:?}", elems),
                    },
                    _ => panic!("expected Tuple, got {:?}", env_var),
                })
                .collect::<HashMap<_, _>>(),
            _ => panic!("expected List, got {:?}", ok),
        },
        _ => panic!("expected Result(Ok(Some(_))), got {:?}", result[0]),
    };

    println!("env vars: {:?}", env_vars);
    check!(env_vars.get("GOLEM_COMPONENT_VERSION") == Some(&"1".to_string()));
    check!(env_vars.get("GOLEM_COMPONENT_ID") == Some(&component_id.0.to_string()));
    check!(
        env_vars.get("GOLEM_WORKER_NAME") == Some(&create_result.worker_id.as_ref().unwrap().name)
    );
    check!(env_vars.get("TEST_ENV_VAR_1") == Some(&"value_1".to_string()));
    check!(env_vars.get("TEST_ENV_VAR_2") == Some(&"value_2".to_string()));
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
