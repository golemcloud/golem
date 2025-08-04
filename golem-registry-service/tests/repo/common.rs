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
use futures::future::join_all;
use golem_common_next::model::component_metadata::ComponentMetadata;
use golem_common_next::model::ComponentFilePermissions;
use golem_registry_service::repo::account::AccountRecord;
use golem_registry_service::repo::environment::EnvironmentRevisionRecord;
use golem_registry_service::repo::model::audit::{
    AuditFields, DeletableRevisionAuditFields, RevisionAuditFields,
};
use golem_registry_service::repo::model::component::{
    ComponentFileRecord, ComponentRevisionRecord, ComponentStatus,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_wasm_ast::analysis::AnalysedResourceMode::Borrowed;
use poem_openapi::types::Type;
use std::collections::BTreeMap;
use std::default::Default;
// Common test cases -------------------------------------------------------------------------------

pub async fn test_create_and_get_account(deps: &Deps) {
    let account = AccountRecord {
        account_id: new_repo_uuid(),
        email: new_repo_uuid().to_string(),
        audit: AuditFields::new(new_repo_uuid()),
        name: new_repo_uuid().to_string(),
        plan_id: deps.test_plan_id(),
    };

    let created_account = deps.account_repo.create(account.clone()).await.unwrap();
    let_assert!(Some(created_account) = created_account);
    assert!(account == created_account);

    let result_for_same_email = deps
        .account_repo
        .create(AccountRecord {
            account_id: new_repo_uuid(),
            email: account.email.clone(),
            audit: AuditFields::new(new_repo_uuid()),
            name: new_repo_uuid().to_string(),
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
    let app_name = format!("app-name-{}", new_repo_uuid());

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
    check!(app.audit.modified_by == user.account_id);
    check!(app.audit.created_at.as_utc() >= &now);
    check!(app.audit.created_at == app.audit.updated_at);
    check!(app.audit.deleted_at.is_none());

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
    let app_name = format!("app-name-{}", new_repo_uuid());
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| {
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

pub async fn test_application_delete(deps: &Deps) {
    let app = deps.create_application().await;
    let user = deps.create_account().await;

    deps.application_repo
        .delete(&user.account_id, &app.application_id)
        .await
        .unwrap();

    let get_by_id = deps
        .application_repo
        .get_by_id(&app.application_id)
        .await
        .unwrap();
    assert!(get_by_id.is_none());
    let get_by_name = deps
        .application_repo
        .get_by_name(&user.account_id, &app.name)
        .await
        .unwrap();
    assert!(get_by_name.is_none());

    // Delete app again, should not fail
    deps.application_repo
        .delete(&user.account_id, &app.application_id)
        .await
        .unwrap();

    let new_app_with_same_name = deps
        .application_repo
        .ensure(&user.account_id, &app.account_id, &app.name)
        .await
        .unwrap();

    check!(new_app_with_same_name.name == app.name);
    check!(new_app_with_same_name.application_id != app.application_id);
}

pub async fn test_environment_create(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application().await;
    let env_name = "local";

    assert!(deps
        .environment_repo
        .get_by_name(&app.application_id, env_name)
        .await
        .unwrap()
        .is_none());

    let revision_0 = EnvironmentRevisionRecord {
        environment_id: new_repo_uuid(),
        revision_id: 0,
        audit: DeletableRevisionAuditFields::new(user.account_id),
        compatibility_check: false,
        version_check: false,
        security_overrides: false,
        hash: blake3::hash("test".as_bytes()).into(),
    };

    let env = deps
        .environment_repo
        .create(&app.application_id, env_name, revision_0.clone())
        .await
        .unwrap();
    let_assert!(Some(env) = env);

    check!(env.name == env_name);
    check!(env.application_id == app.application_id);
    check!(env.revision == revision_0);

    let env_by_name = deps
        .environment_repo
        .get_by_name(&app.application_id, env_name)
        .await
        .unwrap();
    let_assert!(Some(env_by_name) = env_by_name);
    check!(env == env_by_name);

    let env_by_id = deps
        .environment_repo
        .get_by_id(&env.revision.environment_id)
        .await
        .unwrap();
    let_assert!(Some(env_by_id) = env_by_id);
    check!(env == env_by_id);
}

pub async fn test_environment_create_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application().await;
    let env_name = "local";
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async move {
                deps.environment_repo
                    .create(
                        &app.application_id,
                        env_name,
                        EnvironmentRevisionRecord {
                            environment_id: new_repo_uuid(),
                            revision_id: 0,
                            audit: DeletableRevisionAuditFields::new(user.account_id),
                            compatibility_check: false,
                            version_check: false,
                            security_overrides: false,
                            hash: blake3::hash("test".as_bytes()).into(),
                        },
                    )
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let created = results
        .iter()
        .filter(|result| matches!(result, Ok(Some(_))))
        .count();
    let skipped = results
        .iter()
        .filter(|result| matches!(result, Ok(None)))
        .count();
    check!(created == 1);
    check!(skipped == concurrency - 1);
}

pub async fn test_environment_update(deps: &Deps) {
    let user = deps.create_account().await;
    let env_rev_0 = deps.create_env().await;

    let env_rev_1 = EnvironmentRevisionRecord {
        environment_id: env_rev_0.revision.environment_id,
        revision_id: 1,
        audit: DeletableRevisionAuditFields::new(user.account_id),
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: blake3::hash("test".as_bytes()).into(),
    };

    let revision_1_created = deps
        .environment_repo
        .update(env_rev_0.revision.revision_id, env_rev_1.clone())
        .await
        .unwrap();
    let_assert!(Some(revision_1_created) = revision_1_created);
    assert!(env_rev_1 == revision_1_created.revision);
    assert!(env_rev_0.name == revision_1_created.name);
    assert!(env_rev_0.application_id == revision_1_created.application_id);

    let revision_1_retry = deps
        .environment_repo
        .update(env_rev_0.revision.revision_id, env_rev_1.clone())
        .await
        .unwrap();
    assert!(revision_1_retry.is_none());

    let rev_1_by_name = deps
        .environment_repo
        .get_by_name(&env_rev_0.application_id, &env_rev_0.name)
        .await
        .unwrap();
    let_assert!(Some(rev_1_by_name) = rev_1_by_name);
    assert!(env_rev_1 == rev_1_by_name.revision);
    assert!(env_rev_0.name == rev_1_by_name.name);
    assert!(env_rev_0.application_id == rev_1_by_name.application_id);

    let rev_1_by_id = deps
        .environment_repo
        .get_by_id(&env_rev_1.environment_id)
        .await
        .unwrap();
    let_assert!(Some(rev_1_by_id) = rev_1_by_id);
    assert!(env_rev_1 == rev_1_by_id.revision);
    assert!(env_rev_0.name == rev_1_by_id.name);
    assert!(env_rev_0.application_id == rev_1_by_id.application_id);

    let env_rev_2 = EnvironmentRevisionRecord {
        environment_id: env_rev_0.revision.environment_id,
        revision_id: 2,
        audit: DeletableRevisionAuditFields::new(user.account_id),
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: blake3::hash("test".as_bytes()).into(),
    };

    let revision_2_created = deps
        .environment_repo
        .update(revision_1_created.revision.revision_id, env_rev_2.clone())
        .await
        .unwrap();
    let_assert!(Some(revision_2_created) = revision_2_created);
    assert!(env_rev_2 == revision_2_created.revision);
    assert!(env_rev_0.name == revision_2_created.name);
    assert!(env_rev_0.application_id == revision_2_created.application_id);

    let revision_1_retry = deps
        .environment_repo
        .update(env_rev_0.revision.revision_id, env_rev_1.clone())
        .await
        .unwrap();
    assert!(revision_1_retry.is_none());

    let revision_2_retry = deps
        .environment_repo
        .update(env_rev_0.revision.revision_id, env_rev_2.clone())
        .await
        .unwrap();
    assert!(revision_2_retry.is_none());

    let rev_2_by_name = deps
        .environment_repo
        .get_by_name(&env_rev_0.application_id, &env_rev_0.name)
        .await
        .unwrap();
    let_assert!(Some(rev_2_by_name) = rev_2_by_name);
    assert!(env_rev_2 == rev_2_by_name.revision);
    assert!(env_rev_0.name == rev_2_by_name.name);
    assert!(env_rev_0.application_id == rev_2_by_name.application_id);

    let rev_2_by_id = deps
        .environment_repo
        .get_by_id(&env_rev_2.environment_id)
        .await
        .unwrap();
    let_assert!(Some(rev_2_by_id) = rev_2_by_id);
    assert!(env_rev_2 == rev_2_by_id.revision);
    assert!(env_rev_0.name == rev_2_by_id.name);
    assert!(env_rev_0.application_id == rev_2_by_id.application_id);
}

pub async fn test_environment_update_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let env_rev_0 = deps.create_env().await;
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async move {
                deps.environment_repo
                    .update(
                        env_rev_0.revision.revision_id,
                        EnvironmentRevisionRecord {
                            environment_id: env_rev_0.revision.environment_id,
                            revision_id: 0,
                            audit: DeletableRevisionAuditFields::new(user.account_id),
                            compatibility_check: false,
                            version_check: false,
                            security_overrides: false,
                            hash: blake3::hash("test_2".as_bytes()).into(),
                        },
                    )
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

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

pub async fn test_component_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let env = deps.create_env().await;
    let component_name = "test-component";
    let component_id = new_repo_uuid();

    let revision_0 = ComponentRevisionRecord {
        component_id,
        revision_id: 0,
        version: "1.0".to_string(),
        hash: None,
        audit: DeletableRevisionAuditFields::new(user.account_id),
        component_type: 0,
        size: 10,
        metadata: ComponentMetadata {
            exports: vec![],
            producers: vec![],
            memories: vec![],
            binary_wit: Default::default(),
            root_package_name: Some("test".to_string()),
            root_package_version: Some("1.0".to_string()),
            dynamic_linking: Default::default(),
            agent_types: vec![],
        }
        .into(),
        env: BTreeMap::from([("X".to_string(), "value".to_string())]).into(),
        status: ComponentStatus::Created,
        object_store_key: "xys".to_string(),
        binary_hash: blake3::hash("test".as_bytes()).into(),
        transformed_object_store_key: Some("xys-transformed".to_string()),
        files: vec![ComponentFileRecord {
            component_id,
            revision_id: 0,
            file_path: "file".to_string(),
            hash: blake3::hash("test-2".as_bytes()).into(),
            audit: RevisionAuditFields::new(user.account_id),
            file_key: "xdxd".to_string(),
            file_permissions: ComponentFilePermissions::ReadWrite.into(),
        }],
    };

    let created_revision_0 = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            component_name,
            revision_0.clone(),
        )
        .await
        .unwrap();
    let_assert!(Some(created_revision_0) = created_revision_0);
    assert!(revision_0 == created_revision_0);

    let recreate = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            component_name,
            revision_0.clone(),
        )
        .await
        .unwrap();
    assert!(recreate.is_none());

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_id(&component_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_name(&env.revision.environment_id, component_name)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0);

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0] == revision_0);

    let revision_1 = ComponentRevisionRecord {
        revision_id: 1,
        size: 12345,
        env: Default::default(),
        binary_hash: blake3::hash("test-222".as_bytes()).into(),
        transformed_object_store_key: None,
        files: revision_0
            .files
            .iter()
            .map(|file| ComponentFileRecord {
                revision_id: 1,
                ..file.clone()
            })
            .collect(),
        ..revision_0.clone()
    };

    let created_revision_1 = deps
        .component_repo
        .update(0, revision_1.clone())
        .await
        .unwrap();
    let_assert!(Some(created_revision_1) = created_revision_1);
    assert!(revision_1 == created_revision_1);

    let recreated_revision_1 = deps
        .component_repo
        .update(0, revision_1.clone())
        .await
        .unwrap();
    assert!(recreated_revision_1.is_none());

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0] == revision_1);

    let other_component_id = new_repo_uuid();
    let other_component_name = "test-component-other";
    let other_component_revision_0 = ComponentRevisionRecord {
        component_id: other_component_id,
        files: Default::default(),
        ..revision_0.clone()
    };

    let created_other_component_0 = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            other_component_name,
            other_component_revision_0.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(created_other_component_0 == other_component_revision_0);

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 2);
    assert!(components[0] == revision_1);
    assert!(components[1] == other_component_revision_0);

    let delete_with_old_revision = deps
        .component_repo
        .delete(&user.account_id, &component_id, 0)
        .await
        .unwrap();
    assert!(delete_with_old_revision == false);

    let delete_with_current_revision = deps
        .component_repo
        .delete(&user.account_id, &component_id, 1)
        .await
        .unwrap();
    assert!(delete_with_current_revision == true);

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 1);
    assert!(components[0] == other_component_revision_0);

    let revision_after_delete = ComponentRevisionRecord {
        component_id: new_repo_uuid(),
        files: Default::default(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            component_name,
            revision_after_delete.clone(),
        )
        .await
        .unwrap();
    let revision_after_delete = ComponentRevisionRecord {
        component_id: revision_0.component_id,
        revision_id: 3,
        ..revision_after_delete
    };
    let_assert!(Some(created_after_delete) = created_after_delete);
    assert!(created_after_delete == revision_after_delete);
}
