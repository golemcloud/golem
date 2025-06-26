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
use golem_registry_service::repo::account::AccountRecord;
use golem_registry_service::repo::environment::EnvironmentRevisionRecord;
use golem_registry_service::repo::SqlDateTime;
use uuid::Uuid;

// Common test cases -------------------------------------------------------------------------------

pub async fn test_create_and_get_account(deps: &Deps) {
    let account = AccountRecord {
        account_id: Uuid::new_v4(),
        name: Uuid::new_v4().to_string(),
        email: Uuid::new_v4().to_string(),
        created_at: SqlDateTime::now(),
        plan_id: deps.test_plan_id(),
    };

    let created_account = deps.account_repo.create(account.clone()).await.unwrap();
    let_assert!(Some(created_account) = created_account);
    assert!(account == created_account);

    let result_for_same_email = deps
        .account_repo
        .create(AccountRecord {
            account_id: Uuid::new_v4(),
            name: Uuid::new_v4().to_string(),
            email: account.email.clone(),
            created_at: SqlDateTime::now(),
            plan_id: deps.test_plan_id(),
        })
        .await
        .unwrap();
    let_assert!(None = result_for_same_email);

    let requested_account = deps
        .account_repo
        .get_by_id(&account.account_id)
        .await
        .unwrap();
    let_assert!(Some(requested_account) = requested_account);
    assert!(account == requested_account);

    let requested_account = deps
        .account_repo
        .get_by_email(&account.email)
        .await
        .unwrap();
    let_assert!(Some(requested_account) = requested_account);
    assert!(account == requested_account);
}

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

pub async fn test_environment_ensure(deps: &Deps) {
    let now = Utc::now();
    let user = deps.create_account().await;
    let app = deps.create_application().await;
    let env_name = "local";

    let env = deps
        .environment_repo
        .ensure(&user.account_id, &app.application_id, env_name)
        .await
        .unwrap();

    check!(env.name == env_name);
    check!(env.application_id == app.application_id);
    check!(env.environment_created_at.as_utc() >= &now);
    check!(env.environment_created_by == user.account_id);
    check!(env.current_revision_id.is_none());
    check!(env.created_at.is_none());
    check!(env.created_by.is_none());
    check!(env.compatibility_check.is_none());
    check!(env.version_check.is_none());
    check!(env.security_overrides.is_none());
    check!(env.hash.is_none());

    let another_user = deps.create_account().await;
    let env_ensured_by_other_user = deps
        .environment_repo
        .ensure(&another_user.account_id, &app.application_id, env_name)
        .await
        .unwrap();

    check!(env == env_ensured_by_other_user);

    let env_by_name = deps
        .environment_repo
        .get_by_name(&app.application_id, env_name)
        .await
        .unwrap();
    let_assert!(Some(env_by_name) = env_by_name);
    check!(env == env_by_name);

    let env_by_id = deps
        .environment_repo
        .get_by_id(&env.environment_id)
        .await
        .unwrap();
    let_assert!(Some(env_by_id) = env_by_id);
    check!(env == env_by_id);
}

pub async fn test_environment_ensure_concurrent(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application().await;
    let env_name = "local";
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| {
                let deps = deps.clone();
                async move {
                    deps.environment_repo
                        .ensure(&user.account_id, &app.application_id, env_name)
                        .await
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let env = &results[0];
    assert!(env.is_ok());

    for result in &results {
        check!(env == result);
    }
}

pub async fn test_create_environment_revision(deps: &Deps) {
    let user = deps.create_account().await;
    let env_no_revision = deps.create_env().await;
    assert!(env_no_revision.current_revision_id.is_none());
    assert!(env_no_revision.to_revision().is_none());

    let revision_0 = EnvironmentRevisionRecord {
        environment_id: env_no_revision.environment_id,
        revision_id: 0,
        created_at: SqlDateTime::now(),
        created_by: user.account_id,
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: blake3::hash("test".as_bytes()).into(),
    };

    let revision_0_created = deps
        .environment_repo
        .create_revision(env_no_revision.current_revision_id, revision_0.clone())
        .await
        .unwrap();
    let_assert!(Some(revision_0_created) = revision_0_created);
    assert!(revision_0 == revision_0_created);

    let env_with_rev_0 = deps
        .environment_repo
        .ensure(
            &user.account_id,
            &env_no_revision.application_id,
            &env_no_revision.name,
        )
        .await
        .unwrap();
    let_assert!(Some(rev_0_from_ensure) = env_with_rev_0.to_revision());
    assert!(rev_0_from_ensure == revision_0_created);

    let retry_of_rev_0 = deps
        .environment_repo
        .create_revision(env_no_revision.current_revision_id, revision_0.clone())
        .await
        .unwrap();
    assert!(retry_of_rev_0.is_none());

    let mut revision_1 = EnvironmentRevisionRecord {
        environment_id: env_no_revision.environment_id,
        revision_id: 0, // NOTE: this is expected to be overwritten by the repo
        created_at: SqlDateTime::now(),
        created_by: user.account_id,
        compatibility_check: false,
        version_check: false,
        security_overrides: true,
        hash: blake3::hash("test_2".as_bytes()).into(),
    };

    let revision_1_created = deps
        .environment_repo
        .create_revision(env_with_rev_0.current_revision_id, revision_1.clone())
        .await
        .unwrap();
    let_assert!(Some(revision_1_created) = revision_1_created);
    assert!(revision_1_created.revision_id == 1);
    revision_1.revision_id = revision_1_created.revision_id;
    assert!(revision_1 == revision_1_created);

    let env_with_rev_1 = deps
        .environment_repo
        .ensure(
            &user.account_id,
            &env_no_revision.application_id,
            &env_no_revision.name,
        )
        .await
        .unwrap();
    let_assert!(Some(rev_1_from_ensure) = env_with_rev_1.to_revision());
    assert!(rev_1_from_ensure == revision_1_created);

    let retry_of_rev_1 = deps
        .environment_repo
        .create_revision(env_no_revision.current_revision_id, revision_1.clone())
        .await
        .unwrap();
    assert!(retry_of_rev_1.is_none());
}

pub async fn test_create_environment_revisions_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let env_no_revision = deps.create_env().await;
    let concurrency = 20;

    let revision_0_created = deps
        .environment_repo
        .create_revision(
            env_no_revision.current_revision_id,
            EnvironmentRevisionRecord {
                environment_id: env_no_revision.environment_id,
                revision_id: 0,
                created_at: SqlDateTime::now(),
                created_by: user.account_id,
                compatibility_check: true,
                version_check: true,
                security_overrides: false,
                hash: blake3::hash("test".as_bytes()).into(),
            },
        )
        .await
        .unwrap();
    let_assert!(Some(revision_0_created) = revision_0_created);

    let results = join_all(
        (0..concurrency)
            .map(|_| {
                let deps = deps.clone();
                async move {
                    deps.environment_repo
                        .create_revision(
                            Some(0),
                            EnvironmentRevisionRecord {
                                environment_id: env_no_revision.environment_id,
                                revision_id: 0,
                                created_at: SqlDateTime::now(),
                                created_by: user.account_id,
                                compatibility_check: false,
                                version_check: false,
                                security_overrides: false,
                                hash: blake3::hash("test_2".as_bytes()).into(),
                            },
                        )
                        .await
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;

    println!("{:#?}", results);

    let created_count = results
        .iter()
        .filter(|result| matches!(result, Ok(Some(_))))
        .count();
    let skipped_count = results
        .iter()
        .filter(|result| matches!(result, Ok(None)))
        .count();
    check!(created_count == 1);
    check!(skipped_count == concurrency - 1);
}
