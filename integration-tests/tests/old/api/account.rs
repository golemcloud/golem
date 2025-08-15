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
use golem_test_framework::config::{
    EnvBasedTestDependencies, TestDependencies, TestDependenciesDsl,
};
use golem_test_framework::dsl::TestDslUnsafe;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
async fn get_account_of_owner_of_shared_project(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let user_1 = deps.user().await;
    let user_2 = deps.user().await;

    let project = user_1.create_project().await;
    user_1
        .grant_full_project_access(&project, &user_2.account_id)
        .await;

    let result = user_2.get_account(&user_1.account_id).await;
    assert_eq!(result.email, user_1.account_email)
}

#[test]
async fn cannot_get_unrelated_user(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let user_1 = deps.user().await;
    let user_2 = deps.user().await;

    let result = <TestDependenciesDsl<_> as golem_test_framework::dsl::TestDsl>::get_account(
        &user_2,
        &user_1.account_id,
    )
    .await;
    assert!(result.is_err())
}
