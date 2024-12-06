// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use jsonwebtoken::{Algorithm, Validation};
use test_r::{inherit_test_dep, test};

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use assert2::check;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use golem_worker_executor_base::services::worker_identity::{Claims, WorkerClaims};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

#[test]
#[tracing::instrument]
async fn get_identity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component_id = executor.store_component("identity").await;
    let worker_name = "identity-1";
    let worker_id = executor.start_worker(&component_id, worker_name).await;

    let token = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{echo}",
            vec![Value::String("hello".to_string())],
        )
        .await
        .unwrap();

    let Value::String(jwt) = token.first().unwrap() else {
        panic!()
    };

    drop(executor);

    tracing::debug!("jwt {}", jwt);

    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 5;
    validation.validate_nbf = true;
    validation.set_audience(&["123"]); // a single string
    validation.set_issuer(&["123"]); // a single string

    let decoded = jsonwebtoken::decode::<Claims>(
        jwt,
        &jsonwebtoken::DecodingKey::from_secret("secret".as_ref()),
        &validation,
    )
    .expect("decoding failure");

    assert_eq!(decoded.claims.worker.component_id, component_id.to_string());
    assert_eq!(decoded.claims.worker.worker_name, worker_name.to_string());
}
