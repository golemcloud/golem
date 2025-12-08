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
use golem_common::model::component::ComponentFilePermissions;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use golem_registry_service::repo::environment::EnvironmentRevisionRecord;
use golem_registry_service::repo::model::account::{
    AccountExtRevisionRecord, AccountRepoError, AccountRevisionRecord,
};
use golem_registry_service::repo::model::account_usage::{UsageTracking, UsageType};
use golem_registry_service::repo::model::application::{
    ApplicationRepoError, ApplicationRevisionRecord,
};
use golem_registry_service::repo::model::audit::{
    DeletableRevisionAuditFields, ImmutableAuditFields, RevisionAuditFields,
};
use golem_registry_service::repo::model::component::{
    ComponentFileRecord, ComponentRepoError, ComponentRevisionRecord,
};
use golem_registry_service::repo::model::datetime::SqlDateTime;
use golem_registry_service::repo::model::environment::EnvironmentRepoError;
use golem_registry_service::repo::model::hash::SqlBlake3Hash;
use golem_registry_service::repo::model::http_api_definition::{
    HttpApiDefinitionDefinitionBlob, HttpApiDefinitionRepoError, HttpApiDefinitionRevisionRecord,
};
use golem_registry_service::repo::model::http_api_deployment::{
    HttpApiDefinitionNameList, HttpApiDeploymentRepoError, HttpApiDeploymentRevisionRecord,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::model::plugin::PluginRecord;
use golem_service_base::repo::blob::Blob;
use std::collections::{BTreeMap, HashMap};
use std::default::Default;
use strum::IntoEnumIterator;
// Common test cases -------------------------------------------------------------------------------

pub async fn test_create_and_get_account(deps: &Deps) {
    let account = AccountRevisionRecord {
        account_id: new_repo_uuid(),
        revision_id: 0,
        email: new_repo_uuid().to_string(),
        audit: DeletableRevisionAuditFields::new(new_repo_uuid()),
        name: new_repo_uuid().to_string(),
        roles: 0,
        plan_id: deps.test_plan_id(),
    };

    let created_account = deps.account_repo.create(account.clone()).await.unwrap();
    compare_created_to_requested_account(&account, &created_account);

    let result_for_same_email = deps
        .account_repo
        .create(AccountRevisionRecord {
            account_id: new_repo_uuid(),
            revision_id: 0,
            email: account.email.clone(),
            audit: DeletableRevisionAuditFields::new(new_repo_uuid()),
            name: new_repo_uuid().to_string(),
            roles: 0,
            plan_id: deps.test_plan_id(),
        })
        .await;
    let_assert!(Err(AccountRepoError::AccountViolatesUniqueness) = result_for_same_email);

    let requested_account = deps
        .account_repo
        .get_by_id(&account.account_id)
        .await
        .unwrap();
    let_assert!(Some(requested_account) = requested_account);
    compare_created_to_requested_account(&account, &requested_account);

    let requested_account = deps
        .account_repo
        .get_by_email(&account.email)
        .await
        .unwrap();
    let_assert!(Some(requested_account) = requested_account);
    compare_created_to_requested_account(&account, &requested_account);
}

pub async fn test_update(deps: &Deps) {
    let account = AccountRevisionRecord {
        account_id: new_repo_uuid(),
        revision_id: 0,
        email: new_repo_uuid().to_string(),
        audit: DeletableRevisionAuditFields::new(new_repo_uuid()),
        name: new_repo_uuid().to_string(),
        roles: 0,
        plan_id: deps.test_plan_id(),
    };

    let created_account = deps.account_repo.create(account.clone()).await.unwrap();
    compare_created_to_requested_account(&account, &created_account);

    let updated_account = AccountRevisionRecord {
        revision_id: 1,
        name: "Updated name".to_string(),
        ..account
    };

    let created_updated_account = deps
        .account_repo
        .update(updated_account.clone())
        .await
        .unwrap();

    compare_created_to_requested_account(&updated_account, &created_updated_account);
}

pub async fn test_application_create(deps: &Deps) {
    let now = Utc::now();
    let owner = deps.create_account().await;
    let user = deps.create_account().await;
    let app_name = format!("app-name-{}", new_repo_uuid());

    let app = deps
        .application_repo
        .get_by_name(&owner.revision.account_id, &app_name)
        .await
        .unwrap();
    assert!(app.is_none());

    let app = deps
        .application_repo
        .create(
            &owner.revision.account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app_name.clone(),
                audit: DeletableRevisionAuditFields::new(user.revision.account_id),
            },
        )
        .await
        .unwrap();

    check!(app.revision.name == app_name);
    check!(app.account_id == owner.revision.account_id);
    check!(app.revision.audit.created_by == user.revision.account_id);
    check!(app.revision.audit.created_at.as_utc() >= &now);
    check!(app.entity_created_at == app.revision.audit.created_at);
    check!(!app.revision.audit.deleted);

    let app_2 = deps
        .application_repo
        .get_by_name(&owner.revision.account_id, &app_name)
        .await
        .unwrap();
    let_assert!(Some(app_2) = app_2);

    check!(app == app_2);
}

pub async fn test_application_create_concurrent(deps: &Deps) {
    let owner = deps.create_account().await;
    let user = deps.create_account().await;
    let app_name = format!("app-name-{}", new_repo_uuid());
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async {
                deps.application_repo
                    .create(
                        &owner.revision.account_id,
                        ApplicationRevisionRecord {
                            application_id: new_repo_uuid(),
                            revision_id: 0,
                            name: app_name.clone(),
                            audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                        },
                    )
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let created = results.iter().filter(|result| result.is_ok()).count();
    let skipped = results
        .iter()
        .filter(|result| {
            matches!(
                result,
                Err(ApplicationRepoError::ApplicationViolatesUniqueness)
            )
        })
        .count();
    check!(created == 1);
    check!(skipped == concurrency - 1);
}

pub async fn test_application_delete(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;

    let deleted_app = ApplicationRevisionRecord {
        revision_id: app.revision.revision_id + 1,
        ..app.revision.clone()
    };

    deps.application_repo
        .delete(deleted_app.clone())
        .await
        .unwrap();

    let get_by_id = deps
        .application_repo
        .get_by_id(&app.revision.application_id)
        .await
        .unwrap();
    assert!(get_by_id.is_none());
    let get_by_name = deps
        .application_repo
        .get_by_name(&user.revision.account_id, &app.revision.name)
        .await
        .unwrap();
    assert!(get_by_name.is_none());

    // Delete app again, should fail
    {
        let result = deps.application_repo.delete(deleted_app).await;
        assert!(let Err(ApplicationRepoError::ConcurrentModification) = result);
    }

    let new_app_with_same_name = deps
        .application_repo
        .create(
            &user.revision.account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app.revision.name.clone(),
                audit: DeletableRevisionAuditFields::new(user.revision.account_id),
            },
        )
        .await
        .unwrap();

    check!(new_app_with_same_name.revision.name == app.revision.name);
    check!(new_app_with_same_name.revision.application_id != app.revision.application_id);
}

pub async fn test_environment_create(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;

    let env_name = "local";

    assert!(
        deps.environment_repo
            .get_by_name(
                &app.revision.application_id,
                env_name,
                &user.revision.account_id,
                false,
            )
            .await
            .unwrap()
            .is_none()
    );

    let revision_0 = EnvironmentRevisionRecord {
        environment_id: new_repo_uuid(),
        name: env_name.to_string(),
        revision_id: 0,
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        compatibility_check: false,
        version_check: false,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    }
    .with_updated_hash();

    let env = deps
        .environment_repo
        .create(&app.revision.application_id, revision_0.clone())
        .await
        .unwrap();

    check!(env.application_id == app.revision.application_id);
    check!(env.revision == revision_0);

    let env_by_name = deps
        .environment_repo
        .get_by_name(
            &app.revision.application_id,
            env_name,
            &user.revision.account_id,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(env_by_name) = env_by_name);
    check!(env == env_by_name);

    let env_by_id = deps
        .environment_repo
        .get_by_id(
            &env.revision.environment_id,
            &user.revision.account_id,
            false,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(env_by_id) = env_by_id);
    check!(env == env_by_id);
}

pub async fn test_environment_create_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async move {
                deps.environment_repo
                    .create(
                        &app.revision.application_id,
                        EnvironmentRevisionRecord {
                            environment_id: new_repo_uuid(),
                            revision_id: 0,
                            name: "local".to_string(),
                            audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                            compatibility_check: false,
                            version_check: false,
                            security_overrides: false,
                            hash: SqlBlake3Hash::empty(),
                        },
                    )
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    assert_eq!(results.len(), concurrency);
    let created = results.iter().filter(|result| result.is_ok()).count();
    let skipped = results
        .iter()
        .filter(|result| {
            matches!(
                result,
                Err(EnvironmentRepoError::EnvironmentViolatesUniqueness)
            )
        })
        .count();
    check!(created == 1);
    check!(skipped == concurrency - 1);
}

pub async fn test_environment_update(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;
    let env_rev_0 = deps.create_env(&app.revision.application_id).await;

    let env_rev_1 = EnvironmentRevisionRecord {
        environment_id: env_rev_0.revision.environment_id,
        revision_id: 1,
        name: env_rev_0.revision.name.clone(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    }
    .with_updated_hash();

    let revision_1_created = deps
        .environment_repo
        .update(env_rev_1.clone())
        .await
        .unwrap();

    assert!(env_rev_1 == revision_1_created.revision);
    assert!(env_rev_0.revision.name == revision_1_created.revision.name);
    assert!(env_rev_0.application_id == revision_1_created.application_id);

    let revision_1_retry = deps.environment_repo.update(env_rev_1.clone()).await;

    assert!(let Err(EnvironmentRepoError::ConcurrentModification) = revision_1_retry);

    let rev_1_by_name = deps
        .environment_repo
        .get_by_name(
            &env_rev_0.application_id,
            &env_rev_0.revision.name,
            &user.revision.account_id,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(rev_1_by_name) = rev_1_by_name);
    assert!(env_rev_1 == rev_1_by_name.revision);
    assert!(env_rev_0.revision.name == rev_1_by_name.revision.name);
    assert!(env_rev_0.application_id == rev_1_by_name.application_id);

    let rev_1_by_id = deps
        .environment_repo
        .get_by_id(
            &env_rev_1.environment_id,
            &user.revision.account_id,
            false,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(rev_1_by_id) = rev_1_by_id);
    assert!(env_rev_1 == rev_1_by_id.revision);
    assert!(env_rev_0.revision.name == rev_1_by_id.revision.name);
    assert!(env_rev_0.application_id == rev_1_by_id.application_id);

    let env_rev_2 = EnvironmentRevisionRecord {
        environment_id: env_rev_0.revision.environment_id,
        revision_id: 2,
        name: env_rev_1.name.clone(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        compatibility_check: true,
        version_check: true,
        security_overrides: false,
        hash: SqlBlake3Hash::empty(),
    }
    .with_updated_hash();

    let revision_2_created = deps
        .environment_repo
        .update(env_rev_2.clone())
        .await
        .unwrap();

    assert!(env_rev_2 == revision_2_created.revision);
    assert!(env_rev_0.revision.name == revision_2_created.revision.name);
    assert!(env_rev_0.application_id == revision_2_created.application_id);

    let revision_1_retry = deps.environment_repo.update(env_rev_1.clone()).await;
    assert!(let Err(EnvironmentRepoError::ConcurrentModification) = revision_1_retry);

    let revision_2_retry = deps.environment_repo.update(env_rev_2.clone()).await;
    assert!(let Err(EnvironmentRepoError::ConcurrentModification) = revision_2_retry);

    let rev_2_by_name = deps
        .environment_repo
        .get_by_name(
            &env_rev_0.application_id,
            &env_rev_0.revision.name,
            &user.revision.account_id,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(rev_2_by_name) = rev_2_by_name);
    assert!(env_rev_2 == rev_2_by_name.revision);
    assert!(env_rev_0.revision.name == rev_2_by_name.revision.name);
    assert!(env_rev_0.application_id == rev_2_by_name.application_id);

    let rev_2_by_id = deps
        .environment_repo
        .get_by_id(
            &env_rev_2.environment_id,
            &user.revision.account_id,
            false,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(rev_2_by_id) = rev_2_by_id);
    assert!(env_rev_2 == rev_2_by_id.revision);
    assert!(env_rev_0.revision.name == rev_2_by_id.revision.name);
    assert!(env_rev_0.application_id == rev_2_by_id.application_id);
}

pub async fn test_environment_update_concurrently(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;
    let env_rev_0 = deps.create_env(&app.revision.application_id).await;
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async {
                deps.environment_repo
                    .update(EnvironmentRevisionRecord {
                        environment_id: env_rev_0.revision.environment_id,
                        revision_id: 1,
                        name: env_rev_0.revision.name.clone(),
                        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                        compatibility_check: false,
                        version_check: false,
                        security_overrides: false,
                        hash: SqlBlake3Hash::empty(),
                    })
                    .await
            })
            .collect::<Vec<_>>(),
    )
    .await;

    let created_count = results.iter().filter(|result| result.is_ok()).count();
    let skipped_count = results
        .iter()
        .filter(|result| matches!(result, Err(EnvironmentRepoError::ConcurrentModification)))
        .count();

    check!(created_count == 1);
    check!(skipped_count == concurrency - 1);
}

pub async fn test_component_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;
    let env = deps.create_env(&app.revision.application_id).await;
    let app = deps
        .application_repo
        .get_by_id(&env.application_id)
        .await
        .unwrap()
        .unwrap();
    let component_name = "test-component";
    let component_id = new_repo_uuid();

    deps.plugin_repo
        .create(PluginRecord {
            plugin_id: new_repo_uuid(),
            account_id: app.account_id,
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            audit: ImmutableAuditFields::new(user.revision.account_id),
            description: "".to_string(),
            icon: vec![],
            homepage: "".to_string(),
            plugin_type: 0,
            provided_wit_package: None,
            json_schema: None,
            validate_url: None,
            transform_url: None,
            component_id: None,
            component_revision_id: None,
            wasm_content_hash: None,
        })
        .await
        .unwrap()
        .unwrap();

    deps.plugin_repo
        .create(PluginRecord {
            plugin_id: new_repo_uuid(),
            account_id: app.account_id,
            name: "b".to_string(),
            version: "1.0.0".to_string(),
            audit: ImmutableAuditFields::new(user.revision.account_id),
            description: "".to_string(),
            icon: vec![],
            homepage: "".to_string(),
            plugin_type: 0,
            provided_wit_package: None,
            json_schema: None,
            validate_url: None,
            transform_url: None,
            component_id: None,
            component_revision_id: None,
            wasm_content_hash: None,
        })
        .await
        .unwrap()
        .unwrap();

    let revision_0 = ComponentRevisionRecord {
        component_id,
        revision_id: 0,
        version: "1.0".to_string(),
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        size: 10,
        metadata: Blob::new(ComponentMetadata::from_parts(
            vec![],
            vec![],
            HashMap::new(),
            Some("test".to_string()),
            Some("1.0".to_string()),
            vec![],
        )),
        original_env: BTreeMap::from([("X1".to_string(), "value1".to_string())]).into(),
        env: BTreeMap::from([("X".to_string(), "value".to_string())]).into(),
        object_store_key: "xys".to_string(),
        binary_hash: blake3::hash("test".as_bytes()).into(),
        transformed_object_store_key: "xys-transformed".to_string(),
        original_files: vec![ComponentFileRecord {
            component_id,
            revision_id: 0,
            file_path: "file1".to_string(),
            file_content_hash: blake3::hash("test-2".as_bytes()).into(),
            audit: RevisionAuditFields::new(user.revision.account_id),
            file_permissions: ComponentFilePermissions::ReadWrite.into(),
        }],
        plugins: vec![],
        files: vec![ComponentFileRecord {
            component_id,
            revision_id: 0,
            file_path: "file".to_string(),
            file_content_hash: blake3::hash("test-2".as_bytes()).into(),
            audit: RevisionAuditFields::new(user.revision.account_id),
            file_permissions: ComponentFilePermissions::ReadWrite.into(),
        }],
    }
    .with_updated_hash();

    let created_revision_0 = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            component_name,
            revision_0.clone(),
            true,
        )
        .await
        .unwrap();
    let_assert!(created_revision_0 = created_revision_0);
    assert!(revision_0 == created_revision_0.revision);
    assert!(created_revision_0.environment_id == env.revision.environment_id);
    assert!(created_revision_0.name == component_name);

    let recreate = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            component_name,
            revision_0.clone(),
            true,
        )
        .await;
    let_assert!(Err(ComponentRepoError::ComponentViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_id(&component_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == component_name);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_name(&env.revision.environment_id, component_name)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == component_name);

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0].revision == revision_0);
    assert!(components[0].environment_id == env.revision.environment_id);
    assert!(components[0].name == component_name);

    let revision_1 = ComponentRevisionRecord {
        revision_id: 1,
        size: 12345,
        env: Default::default(),
        binary_hash: SqlBlake3Hash::empty(),
        transformed_object_store_key: "xys-transformed".to_string(),
        original_files: revision_0
            .original_files
            .iter()
            .map(|file| ComponentFileRecord {
                revision_id: 1,
                ..file.clone()
            })
            .collect(),
        files: revision_0
            .files
            .iter()
            .map(|file| ComponentFileRecord {
                revision_id: 1,
                ..file.clone()
            })
            .collect(),
        ..revision_0.clone()
    }
    .with_updated_hash();

    let created_revision_1 = deps
        .component_repo
        .update(revision_1.clone(), true)
        .await
        .unwrap();
    let_assert!(created_revision_1 = created_revision_1);
    assert!(revision_1 == created_revision_1.revision);
    assert!(created_revision_1.environment_id == env.revision.environment_id);
    assert!(created_revision_1.name == component_name);

    let recreated_revision_1 = deps.component_repo.update(revision_1.clone(), true).await;
    let_assert!(Err(ComponentRepoError::ConcurrentModification) = recreated_revision_1);

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0].revision == revision_1);

    let other_component_id = new_repo_uuid();
    let other_component_name = "test-component-other";
    let other_component_revision_0 = ComponentRevisionRecord {
        component_id: other_component_id,
        original_files: Default::default(),
        plugins: Default::default(),
        files: Default::default(),
        ..revision_0.clone()
    }
    .with_updated_hash();

    let created_other_component_0 = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            other_component_name,
            other_component_revision_0.clone(),
            true,
        )
        .await
        .unwrap();
    assert!(created_other_component_0.revision == other_component_revision_0);

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 2);
    assert!(components[0].revision == revision_1);
    assert!(components[1].revision == other_component_revision_0);

    let delete_with_old_revision = deps
        .component_repo
        .delete(&user.revision.account_id, &component_id, 1)
        .await;
    let_assert!(Err(ComponentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.component_repo
        .delete(&user.revision.account_id, &component_id, 2)
        .await
        .unwrap();

    let components = deps
        .component_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 1);
    assert!(components[0].revision == other_component_revision_0);

    let revision_after_delete = ComponentRevisionRecord {
        component_id: new_repo_uuid(),
        original_files: Default::default(),
        plugins: Default::default(),
        files: Default::default(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .component_repo
        .create(
            &env.revision.environment_id,
            component_name,
            revision_after_delete.clone(),
            true,
        )
        .await
        .unwrap();
    let revision_after_delete = ComponentRevisionRecord {
        component_id: revision_0.component_id,
        revision_id: 3,
        ..revision_after_delete
    }
    .with_updated_hash();
    let_assert!(created_after_delete = created_after_delete);
    assert!(created_after_delete.revision == revision_after_delete);
}

pub async fn test_http_api_definition_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;
    let env = deps.create_env(&app.revision.application_id).await;
    let definition_name = "test-api-definition";
    let definition_id = new_repo_uuid();

    let revision_0 = HttpApiDefinitionRevisionRecord {
        http_api_definition_id: definition_id,
        revision_id: 0,
        version: "1.0".to_string(),
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        definition: Blob::new(HttpApiDefinitionDefinitionBlob { routes: Vec::new() }),
    }
    .with_updated_hash();

    let created_revision_0 = deps
        .http_api_definition_repo
        .create(
            &env.revision.environment_id,
            definition_name,
            revision_0.clone(),
            true,
        )
        .await
        .unwrap();
    let_assert!(created_revision_0 = created_revision_0);
    assert!(revision_0 == created_revision_0.revision);
    assert!(created_revision_0.environment_id == env.revision.environment_id);
    assert!(created_revision_0.name == definition_name);

    let recreate = deps
        .http_api_definition_repo
        .create(
            &env.revision.environment_id,
            definition_name,
            revision_0.clone(),
            true,
        )
        .await;
    let_assert!(Err(HttpApiDefinitionRepoError::ApiDefinitionViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .http_api_definition_repo
        .get_staged_by_id(&definition_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == definition_name);

    let get_revision_0 = deps
        .http_api_definition_repo
        .get_staged_by_name(&env.revision.environment_id, definition_name)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == definition_name);

    let definitions = deps
        .http_api_definition_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(definitions.len() == 1);
    assert!(definitions[0].revision == revision_0);
    assert!(definitions[0].environment_id == env.revision.environment_id);
    assert!(definitions[0].name == definition_name);

    let revision_1 = HttpApiDefinitionRevisionRecord {
        revision_id: 1,
        version: "1.1".to_string(),
        hash: SqlBlake3Hash::empty(),
        definition: Blob::new(HttpApiDefinitionDefinitionBlob { routes: Vec::new() }),
        ..revision_0.clone()
    }
    .with_updated_hash();

    let created_revision_1 = deps
        .http_api_definition_repo
        .update(revision_1.clone(), true)
        .await
        .unwrap();
    let_assert!(created_revision_1 = created_revision_1);
    assert!(revision_1 == created_revision_1.revision);
    assert!(created_revision_1.environment_id == env.revision.environment_id);
    assert!(created_revision_1.name == definition_name);

    let recreated_revision_1 = deps
        .http_api_definition_repo
        .update(revision_1.clone(), true)
        .await;
    let_assert!(Err(HttpApiDefinitionRepoError::ConcurrentModification) = recreated_revision_1);

    let definitions = deps
        .http_api_definition_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(definitions.len() == 1);
    assert!(definitions[0].revision == revision_1);

    let other_definition_id = new_repo_uuid();
    let other_definition_name = "test-api-definition-other";
    let other_definition_revision_0 = HttpApiDefinitionRevisionRecord {
        http_api_definition_id: other_definition_id,
        ..revision_0.clone()
    };

    let created_other_definition_0 = deps
        .http_api_definition_repo
        .create(
            &env.revision.environment_id,
            other_definition_name,
            other_definition_revision_0.clone(),
            true,
        )
        .await
        .unwrap();
    assert!(created_other_definition_0.revision == other_definition_revision_0);

    let definitions = deps
        .http_api_definition_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(definitions.len() == 2);
    assert!(definitions[0].revision == revision_1);
    assert!(definitions[1].revision == other_definition_revision_0);

    let delete_with_old_revision = deps
        .http_api_definition_repo
        .delete(&user.revision.account_id, &definition_id, 1)
        .await;
    let_assert!(Err(HttpApiDefinitionRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.http_api_definition_repo
        .delete(&user.revision.account_id, &definition_id, 2)
        .await
        .unwrap();

    let definitions = deps
        .http_api_definition_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(definitions.len() == 1);
    assert!(definitions[0].revision == other_definition_revision_0);

    let revision_after_delete = HttpApiDefinitionRevisionRecord {
        http_api_definition_id: new_repo_uuid(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .http_api_definition_repo
        .create(
            &env.revision.environment_id,
            definition_name,
            revision_after_delete.clone(),
            true,
        )
        .await
        .unwrap();
    let revision_after_delete = HttpApiDefinitionRevisionRecord {
        http_api_definition_id: revision_0.http_api_definition_id,
        revision_id: 3,
        ..revision_after_delete
    };
    assert!(created_after_delete.revision == revision_after_delete);
}

pub async fn test_http_api_deployment_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(&user.revision.account_id).await;
    let env = deps.create_env(&app.revision.application_id).await;
    let domain = "test-host-1.com";
    let deployment_id = new_repo_uuid();

    let definition_id = new_repo_uuid();
    let definition_name = "test-api-definition";
    let definition_revision = HttpApiDefinitionRevisionRecord {
        http_api_definition_id: definition_id,
        revision_id: 0,
        version: "1.0".to_string(),
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        definition: Blob::new(HttpApiDefinitionDefinitionBlob { routes: Vec::new() }),
    };

    let created_definition = deps
        .http_api_definition_repo
        .create(
            &env.revision.environment_id,
            definition_name,
            definition_revision.clone(),
            true,
        )
        .await
        .unwrap();

    let revision_0 = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: deployment_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        http_api_definitions: HttpApiDefinitionNameList(vec![HttpApiDefinitionName(
            created_definition.name,
        )]),
    }
    .with_updated_hash();

    let created_revision_0 = deps
        .http_api_deployment_repo
        .create(&env.revision.environment_id, domain, revision_0.clone())
        .await
        .unwrap();

    assert!(revision_0 == created_revision_0.revision);
    assert!(created_revision_0.environment_id == env.revision.environment_id);
    assert!(created_revision_0.domain == domain);

    let recreate = deps
        .http_api_deployment_repo
        .create(&env.revision.environment_id, domain, revision_0.clone())
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ApiDeploymentViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .http_api_deployment_repo
        .get_staged_by_id(&deployment_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.domain == domain);

    let get_revision_0 = deps
        .http_api_deployment_repo
        .get_staged_by_domain(&env.revision.environment_id, domain)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.domain == domain);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();
    assert!(deployments.len() == 1);
    assert!(deployments[0].revision == revision_0);
    assert!(deployments[0].environment_id == env.revision.environment_id);
    assert!(deployments[0].domain == domain);

    let revision_1 = HttpApiDeploymentRevisionRecord {
        revision_id: 1,
        hash: SqlBlake3Hash::empty(),
        ..revision_0.clone()
    }
    .with_updated_hash();

    let created_revision_1 = deps
        .http_api_deployment_repo
        .update(revision_1.clone())
        .await
        .unwrap();

    assert!(revision_1 == created_revision_1.revision);
    assert!(created_revision_1.environment_id == env.revision.environment_id);
    assert!(created_revision_1.domain == domain);

    let recreated_revision_1 = deps
        .http_api_deployment_repo
        .update(revision_1.clone())
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ConcurrentModification) = recreated_revision_1);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);
    assert!(deployments[0].revision == revision_1);

    let other_deployment_id = new_repo_uuid();
    let other_domain = "test-host-2.com";
    let other_deployment_revision_0 = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: other_deployment_id,
        ..revision_0.clone()
    }
    .with_updated_hash();

    let created_other_deployment_0 = deps
        .http_api_deployment_repo
        .create(
            &env.revision.environment_id,
            other_domain,
            other_deployment_revision_0.clone(),
        )
        .await
        .unwrap();
    assert!(created_other_deployment_0.revision == other_deployment_revision_0);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 2);
    assert!(deployments[0].revision == revision_1);
    assert!(deployments[1].revision == other_deployment_revision_0);

    let delete_with_old_revision = deps
        .http_api_deployment_repo
        .delete(&user.revision.account_id, &deployment_id, 1)
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.http_api_deployment_repo
        .delete(&user.revision.account_id, &deployment_id, 2)
        .await
        .unwrap();

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(&env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 1);
    assert!(deployments[0].revision == other_deployment_revision_0);

    let revision_after_delete = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: new_repo_uuid(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .http_api_deployment_repo
        .create(
            &env.revision.environment_id,
            domain,
            revision_after_delete.clone(),
        )
        .await
        .unwrap();
    let revision_after_delete = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: revision_0.http_api_deployment_id,
        revision_id: 3,
        ..revision_after_delete
    };
    assert!(created_after_delete.revision == revision_after_delete);
}

pub async fn test_account_usage(deps: &Deps) {
    let user = deps.create_account().await;
    let now = SqlDateTime::now();

    let mut usage = deps
        .account_usage_repo
        .get(&user.revision.account_id, &now)
        .await
        .unwrap()
        .unwrap();

    for usage_type in UsageType::iter() {
        let limit: i64 = match usage_type {
            UsageType::TotalAppCount => 3,
            UsageType::TotalEnvCount => 10,
            UsageType::TotalComponentCount => 15,
            UsageType::TotalWorkerCount => 20,
            UsageType::TotalWorkerConnectionCount => 25,
            UsageType::TotalComponentStorageBytes => 1000,
            UsageType::MonthlyGasLimit => 2000,
            UsageType::MonthlyComponentUploadLimitBytes => 3000,
        };
        let plan_limit = usage.plan.limit(usage_type);
        assert!(plan_limit == limit);

        check!(usage.usage(usage_type) == 0, "{usage_type:?}");
        assert!(usage.add_change(usage_type, 1));
        check!(usage.change(usage_type) == 1, "{usage_type:?}");
    }

    let increased_usage = usage;

    {
        deps.account_usage_repo.add(&increased_usage).await.unwrap();
        let usage = deps
            .account_usage_repo
            .get(&user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();
        for usage_type in UsageType::iter() {
            if usage_type.tracking() == UsageTracking::Stats {
                check!(usage.usage(usage_type) == 1, "{usage_type:?}");
            } else {
                check!(usage.usage(usage_type) == 0, "{usage_type:?}");
            }
            check!(usage.change(usage_type) == 0, "{usage_type:?}");
        }
    }

    {
        deps.account_usage_repo.add(&increased_usage).await.unwrap();
        deps.account_usage_repo.add(&increased_usage).await.unwrap();
        let usage = deps
            .account_usage_repo
            .get(&user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();

        for usage_type in UsageType::iter() {
            if usage_type.tracking() == UsageTracking::Stats {
                check!(usage.usage(usage_type) == 3, "{usage_type:?}");
            } else {
                check!(usage.usage(usage_type) == 0, "{usage_type:?}");
            }
            check!(usage.change(usage_type) == 0, "{usage_type:?}");
        }
    }

    {
        let mut usage = deps
            .account_usage_repo
            .get(&user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();

        for usage_type in UsageType::iter() {
            check!(!usage.add_change(usage_type, 1000000));
        }
    }

    {
        let app = deps
            .application_repo
            .create(
                &user.revision.account_id,
                ApplicationRevisionRecord {
                    application_id: new_repo_uuid(),
                    revision_id: 0,
                    name: "test-app".to_string(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                },
            )
            .await
            .unwrap();

        let env = deps
            .environment_repo
            .create(
                &app.revision.application_id,
                EnvironmentRevisionRecord {
                    environment_id: new_repo_uuid(),
                    revision_id: 0,
                    name: "env".to_string(),
                    hash: SqlBlake3Hash::empty(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                },
            )
            .await
            .unwrap();
        let _component = deps
            .component_repo
            .create(
                &env.revision.environment_id,
                "component",
                ComponentRevisionRecord {
                    component_id: Default::default(),
                    revision_id: 0,
                    version: "".to_string(),
                    hash: SqlBlake3Hash::empty(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    size: 0,
                    metadata: Blob::new(ComponentMetadata::from_parts(
                        vec![],
                        vec![],
                        HashMap::new(),
                        None,
                        None,
                        vec![],
                    )),
                    env: Default::default(),
                    original_env: Default::default(),
                    object_store_key: "".to_string(),
                    transformed_object_store_key: "".to_string(),
                    binary_hash: SqlBlake3Hash::empty(),
                    original_files: vec![],
                    plugins: vec![],
                    files: vec![],
                },
                true,
            )
            .await
            .unwrap();

        let usage = deps
            .account_usage_repo
            .get(&user.revision.account_id, &now)
            .await
            .unwrap()
            .unwrap();
        check!(usage.usage(UsageType::TotalAppCount) == 1);
        check!(usage.usage(UsageType::TotalEnvCount) == 1);
        check!(usage.usage(UsageType::TotalComponentCount) == 1);
    }
}

fn compare_created_to_requested_account(
    requested: &AccountRevisionRecord,
    created: &AccountExtRevisionRecord,
) {
    assert!(created.revision.account_id == requested.account_id);
    assert!(created.revision.name == requested.name);
    assert!(created.revision.email == requested.email);
    assert!(created.revision.roles == requested.roles)
}
