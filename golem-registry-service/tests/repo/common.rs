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

use crate::repo::Deps;
use assert2::{assert, check, let_assert};
use chrono::Utc;
use futures_util::future::join_all;
use uuid::Uuid;

// Common test cases -------------------------------------------------------------------------------

pub async fn test_application_ensure(deps: &Deps) {
    let now = Utc::now();
    let owner = deps.create_account().await;
    let user = deps.create_account().await;
    let app_name = format!("app-name-{}", Uuid::new_v4());

    let app = deps
        .application_repo
        .get_by_name(&owner.account_id, &app_name)
        .await
        .unwrap();
    assert!(app.is_none());

    let app = deps
        .application_repo
        .ensure(&user.account_id, &owner.account_id, &app_name)
        .await
        .unwrap();

    check!(app.name == app_name);
    check!(app.account_id == owner.account_id);
    check!(app.created_by == user.account_id);
    check!(app.created_at.as_utc() >= &now);

    let app_2 = deps
        .application_repo
        .ensure(&user.account_id, &owner.account_id, &app_name)
        .await
        .unwrap();

    check!(app == app_2);

    let app_3 = deps
        .application_repo
        .get_by_name(&owner.account_id, &app_name)
        .await
        .unwrap();
    let_assert!(Some(app_3) = app_3);

    check!(app == app_3);
}

pub async fn test_application_ensure_concurrent(deps: &Deps) {
    let owner = deps.create_account().await;
    let user = deps.create_account().await;
    let app_name = format!("app-name-{}", Uuid::new_v4());
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| {
                let deps = deps.clone();
                let app_name = app_name.clone();
                async move {
                    deps.application_repo
                        .ensure(&user.account_id, &owner.account_id, &app_name)
                        .await
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let app = &results[0];
    assert!(app.is_ok());

    for result in &results {
        check!(app == result);
    }
}
