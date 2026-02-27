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
use golem_common::base_model::Empty;
use golem_common::model::agent::{
    AgentConstructor, AgentMode, AgentType, AgentTypeName, DataSchema, NamedElementSchemas,
    Snapshotting,
};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::component::ComponentFilePermissions;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::environment_share::EnvironmentShareId;
use golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions;
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
use golem_registry_service::repo::model::deployment::{
    DeploymentRegisteredAgentTypeRecord, DeploymentRevisionCreationRecord,
};
use golem_registry_service::repo::model::environment::EnvironmentRepoError;
use golem_registry_service::repo::model::environment_share::EnvironmentShareRevisionRecord;
use golem_registry_service::repo::model::hash::SqlBlake3Hash;
use golem_registry_service::repo::model::http_api_deployment::{
    HttpApiDeploymentData, HttpApiDeploymentRepoError, HttpApiDeploymentRevisionRecord,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::model::plugin::PluginRecord;
use golem_service_base::repo::blob::Blob;
use std::collections::{BTreeMap, BTreeSet};
use std::default::Default;
use strum::IntoEnumIterator;
use uuid::Uuid;
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
        .get_by_id(account.account_id)
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
        .get_by_name(owner.revision.account_id, &app_name)
        .await
        .unwrap();
    assert!(app.is_none());

    let app = deps
        .application_repo
        .create(
            owner.revision.account_id,
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
        .get_by_name(owner.revision.account_id, &app_name)
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
                        owner.revision.account_id,
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
    let app = deps.create_application(user.revision.account_id).await;

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
        .get_by_id(app.revision.application_id)
        .await
        .unwrap();
    assert!(get_by_id.is_none());
    let get_by_name = deps
        .application_repo
        .get_by_name(user.revision.account_id, &app.revision.name)
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
            user.revision.account_id,
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
    let app = deps.create_application(user.revision.account_id).await;

    let env_name = "local";

    assert!(
        deps.environment_repo
            .get_by_name(
                app.revision.application_id,
                env_name,
                user.revision.account_id,
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
        .create(app.revision.application_id, revision_0.clone())
        .await
        .unwrap();

    check!(env.application_id == app.revision.application_id);
    check!(env.revision == revision_0);

    let env_by_name = deps
        .environment_repo
        .get_by_name(
            app.revision.application_id,
            env_name,
            user.revision.account_id,
            false,
        )
        .await
        .unwrap();
    let_assert!(Some(env_by_name) = env_by_name);
    check!(env == env_by_name);

    let env_by_id = deps
        .environment_repo
        .get_by_id(
            env.revision.environment_id,
            user.revision.account_id,
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
    let app = deps.create_application(user.revision.account_id).await;
    let concurrency = 20;

    let results = join_all(
        (0..concurrency)
            .map(|_| async move {
                deps.environment_repo
                    .create(
                        app.revision.application_id,
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
    let app = deps.create_application(user.revision.account_id).await;
    let env_rev_0 = deps.create_env(app.revision.application_id).await;

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
            env_rev_0.application_id,
            &env_rev_0.revision.name,
            user.revision.account_id,
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
            env_rev_1.environment_id,
            user.revision.account_id,
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
            env_rev_0.application_id,
            &env_rev_0.revision.name,
            user.revision.account_id,
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
            env_rev_2.environment_id,
            user.revision.account_id,
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
    let app = deps.create_application(user.revision.account_id).await;
    let env_rev_0 = deps.create_env(app.revision.application_id).await;
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
    let app = deps.create_application(user.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let app = deps
        .application_repo
        .get_by_id(env.application_id)
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
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        size: 10.into(),
        metadata: Blob::new(ComponentMetadata::from_parts(
            vec![],
            vec![],
            Some("test".to_string()),
            Some("1.0".to_string()),
            vec![],
        )),
        env: BTreeMap::from([("X".to_string(), "value".to_string())]).into(),
        config_vars: BTreeMap::from([("WC".to_string(), "value".to_string())]).into(),
        local_agent_config: Blob::new(Vec::new()),
        object_store_key: "xys".to_string(),
        binary_hash: blake3::hash("test".as_bytes()).into(),
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
            env.revision.environment_id,
            component_name,
            revision_0.clone(),
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
            env.revision.environment_id,
            component_name,
            revision_0.clone(),
        )
        .await;
    let_assert!(Err(ComponentRepoError::ComponentViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_id(component_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == component_name);

    let get_revision_0 = deps
        .component_repo
        .get_staged_by_name(env.revision.environment_id, component_name)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.name == component_name);

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0].revision == revision_0);
    assert!(components[0].environment_id == env.revision.environment_id);
    assert!(components[0].name == component_name);

    let revision_1 = ComponentRevisionRecord {
        revision_id: 1,
        size: 12345.into(),
        env: Default::default(),
        binary_hash: SqlBlake3Hash::empty(),
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
        .update(revision_1.clone())
        .await
        .unwrap();
    let_assert!(created_revision_1 = created_revision_1);
    assert!(revision_1 == created_revision_1.revision);
    assert!(created_revision_1.environment_id == env.revision.environment_id);
    assert!(created_revision_1.name == component_name);

    let recreated_revision_1 = deps.component_repo.update(revision_1.clone()).await;
    let_assert!(Err(ComponentRepoError::ConcurrentModification) = recreated_revision_1);

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();
    assert!(components.len() == 1);
    assert!(components[0].revision == revision_1);

    let other_component_id = new_repo_uuid();
    let other_component_name = "test-component-other";
    let other_component_revision_0 = ComponentRevisionRecord {
        component_id: other_component_id,
        plugins: Default::default(),
        files: Default::default(),
        ..revision_0.clone()
    }
    .with_updated_hash();

    let created_other_component_0 = deps
        .component_repo
        .create(
            env.revision.environment_id,
            other_component_name,
            other_component_revision_0.clone(),
        )
        .await
        .unwrap();
    assert!(created_other_component_0.revision == other_component_revision_0);

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 2);
    assert!(components[0].revision == revision_1);
    assert!(components[1].revision == other_component_revision_0);

    let delete_with_old_revision = deps
        .component_repo
        .delete(user.revision.account_id, component_id, 1)
        .await;
    let_assert!(Err(ComponentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.component_repo
        .delete(user.revision.account_id, component_id, 2)
        .await
        .unwrap();

    let components = deps
        .component_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(components.len() == 1);
    assert!(components[0].revision == other_component_revision_0);

    let revision_after_delete = ComponentRevisionRecord {
        component_id: new_repo_uuid(),
        plugins: Default::default(),
        files: Default::default(),
        ..revision_0.clone()
    };
    let created_after_delete = deps
        .component_repo
        .create(
            env.revision.environment_id,
            component_name,
            revision_after_delete.clone(),
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

pub async fn test_http_api_deployment_stage(deps: &Deps) {
    let user = deps.create_account().await;
    let app = deps.create_application(user.revision.account_id).await;
    let env = deps.create_env(app.revision.application_id).await;
    let domain = "test-host-1.com";
    let deployment_id = new_repo_uuid();

    let revision_0 = HttpApiDeploymentRevisionRecord {
        http_api_deployment_id: deployment_id,
        revision_id: 0,
        hash: SqlBlake3Hash::empty(),
        audit: DeletableRevisionAuditFields::new(user.revision.account_id),
        data: Blob::new(HttpApiDeploymentData {
            agents: BTreeMap::from_iter([(
                AgentTypeName("test-agent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            )]),
            webhooks_url: "/webhooks/".to_string(),
        }),
    }
    .with_updated_hash();

    let created_revision_0 = deps
        .http_api_deployment_repo
        .create(env.revision.environment_id, domain, revision_0.clone())
        .await
        .unwrap();

    assert!(revision_0 == created_revision_0.revision);
    assert!(created_revision_0.environment_id == env.revision.environment_id);
    assert!(created_revision_0.domain == domain);

    let recreate = deps
        .http_api_deployment_repo
        .create(env.revision.environment_id, domain, revision_0.clone())
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ApiDeploymentViolatesUniqueness) = recreate);

    let get_revision_0 = deps
        .http_api_deployment_repo
        .get_staged_by_id(deployment_id)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.domain == domain);

    let get_revision_0 = deps
        .http_api_deployment_repo
        .get_staged_by_domain(env.revision.environment_id, domain)
        .await
        .unwrap();
    let_assert!(Some(get_revision_0) = get_revision_0);
    assert!(revision_0 == get_revision_0.revision);
    assert!(get_revision_0.environment_id == env.revision.environment_id);
    assert!(get_revision_0.domain == domain);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
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
        .list_staged(env.revision.environment_id)
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
            env.revision.environment_id,
            other_domain,
            other_deployment_revision_0.clone(),
        )
        .await
        .unwrap();
    assert!(created_other_deployment_0.revision == other_deployment_revision_0);

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
        .await
        .unwrap();

    assert!(deployments.len() == 2);
    assert!(deployments[0].revision == revision_1);
    assert!(deployments[1].revision == other_deployment_revision_0);

    let delete_with_old_revision = deps
        .http_api_deployment_repo
        .delete(user.revision.account_id, deployment_id, 1)
        .await;

    let_assert!(Err(HttpApiDeploymentRepoError::ConcurrentModification) = delete_with_old_revision);

    deps.http_api_deployment_repo
        .delete(user.revision.account_id, deployment_id, 2)
        .await
        .unwrap();

    let deployments = deps
        .http_api_deployment_repo
        .list_staged(env.revision.environment_id)
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
            env.revision.environment_id,
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
        .get(user.revision.account_id, &now)
        .await
        .unwrap()
        .unwrap();

    for usage_type in UsageType::iter() {
        let limit: u64 = match usage_type {
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
            .get(user.revision.account_id, &now)
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
            .get(user.revision.account_id, &now)
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
            .get(user.revision.account_id, &now)
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
                user.revision.account_id,
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
                app.revision.application_id,
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
                env.revision.environment_id,
                "component",
                ComponentRevisionRecord {
                    component_id: Default::default(),
                    revision_id: 0,
                    hash: SqlBlake3Hash::empty(),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    size: 0.into(),
                    metadata: Blob::new(ComponentMetadata::from_parts(
                        vec![],
                        vec![],
                        None,
                        None,
                        vec![],
                    )),
                    env: Default::default(),
                    config_vars: Default::default(),
                    local_agent_config: Blob::new(Vec::new()),
                    object_store_key: "".to_string(),
                    binary_hash: SqlBlake3Hash::empty(),
                    plugins: vec![],
                    files: vec![],
                },
            )
            .await
            .unwrap();

        let usage = deps
            .account_usage_repo
            .get(user.revision.account_id, &now)
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

// resolve_agent_type_by_names tests ---------------------------------------------------------------

fn make_test_agent_type(name: &str) -> AgentType {
    AgentType {
        type_name: AgentTypeName(name.to_string()),
        description: format!("Test agent {name}"),
        constructor: AgentConstructor {
            name: None,
            description: "constructor".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        },
        methods: vec![],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
    }
}

struct ResolveTestEnv {
    owner_account_id: Uuid,
    owner_email: String,
    app_name: String,
    env_name: String,
    environment_id: Uuid,
    deployment_revision_id: i64,
    agent_type_name: String,
}

/// Sets up: owner account → application → environment → deployment with one agent type.
/// Returns the environment context for use in resolve_agent_type_by_names tests.
async fn setup_resolve_env(deps: &Deps) -> ResolveTestEnv {
    let email = format!("resolve-test-{}@golem.test", new_repo_uuid());
    let owner = deps.create_account_with_email(&email).await;
    let owner_account_id = owner.revision.account_id;
    let app_name = format!("resolve-app-{}", new_repo_uuid());
    let env_name = format!("resolve-env-{}", new_repo_uuid());

    let app = deps
        .application_repo
        .create(
            owner_account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app_name.clone(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
            },
        )
        .await
        .unwrap();

    let env = deps
        .environment_repo
        .create(
            app.revision.application_id,
            EnvironmentRevisionRecord {
                environment_id: new_repo_uuid(),
                revision_id: 0,
                name: env_name.clone(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
                hash: SqlBlake3Hash::empty(),
            },
        )
        .await
        .unwrap();

    let environment_id = env.revision.environment_id;

    // Create a component (required by FK on deployment_registered_agent_types)
    let component_name = format!("test-component-{}", new_repo_uuid());
    let component = deps
        .component_repo
        .create(
            environment_id,
            &component_name,
            ComponentRevisionRecord {
                component_id: new_repo_uuid(),
                revision_id: 0,
                version: "0.1.0".to_string(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
                size: 0.into(),
                metadata: Blob::new(ComponentMetadata::from_parts(
                    vec![],
                    vec![],
                    None,
                    None,
                    vec![],
                )),
                env: Default::default(),
                original_env: Default::default(),
                config_vars: Default::default(),
                original_config_vars: Default::default(),
                object_store_key: "".to_string(),
                transformed_object_store_key: "".to_string(),
                binary_hash: SqlBlake3Hash::empty(),
                original_files: vec![],
                plugins: vec![],
                files: vec![],
            },
            false,
        )
        .await
        .unwrap();

    let component_id = component.revision.component_id;
    let component_revision_id = component.revision.revision_id;

    let agent_type_name = format!("TestAgent{}", new_repo_uuid().simple());
    let agent_type = make_test_agent_type(&agent_type_name);
    let wrapper_type_name = agent_type.wrapper_type_name();
    let deployment_revision_id: i64 = 1;

    let agent_type_record = DeploymentRegisteredAgentTypeRecord {
        environment_id,
        deployment_revision_id,
        agent_type_name: agent_type_name.clone(),
        agent_wrapper_type_name: wrapper_type_name,
        component_id,
        component_revision_id,
        webhook_prefix_authority_and_path: None,
        agent_type: Blob::new(agent_type),
    };

    let deployment_creation = DeploymentRevisionCreationRecord {
        environment_id,
        deployment_revision_id,
        version: "1.0.0".to_string(),
        hash: SqlBlake3Hash::empty(),
        components: vec![],
        http_api_deployments: vec![],
        compiled_routes: vec![],
        registered_agent_types: vec![agent_type_record],
    };

    deps.full_deployment_repo
        .deploy(owner_account_id, deployment_creation, false)
        .await
        .unwrap();

    ResolveTestEnv {
        owner_account_id,
        owner_email: email,
        app_name,
        env_name,
        environment_id,
        deployment_revision_id,
        agent_type_name,
    }
}

/// Caller owns env → works (no email)
pub async fn test_resolve_agent_type_owner_no_email(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            env.owner_account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            None, // latest deployment
            None, // no owner email → use caller's own account
        )
        .await
        .unwrap();

    let_assert!(Some(record) = result);
    check!(record.agent_type_name == env.agent_type_name);
    check!(record.environment_id == env.environment_id);
    check!(record.deployment_revision_id == env.deployment_revision_id);
    check!(record.owner_account_id == env.owner_account_id);
}

/// Caller has share (Viewer role) + email → works
pub async fn test_resolve_agent_type_shared_with_email(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    // Create a grantee account
    let grantee = deps.create_account().await;
    let grantee_account_id = grantee.revision.account_id;

    // Grant Viewer role to the grantee
    let share_id = EnvironmentShareId(new_repo_uuid());
    let mut roles = BTreeSet::new();
    roles.insert(EnvironmentRole::Viewer);

    deps.environment_share_repo
        .create(
            env.environment_id,
            EnvironmentShareRevisionRecord::creation(
                share_id,
                roles,
                golem_common::model::account::AccountId(env.owner_account_id),
            ),
            grantee_account_id,
        )
        .await
        .unwrap();

    // Grantee resolves using owner's email
    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            grantee_account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            None,
            Some(&env.owner_email),
        )
        .await
        .unwrap();

    let_assert!(Some(record) = result);
    check!(record.agent_type_name == env.agent_type_name);
    check!(record.owner_account_id == env.owner_account_id);
    // roles_bitmask should include Viewer (bit 2 = 4)
    check!(record.environment_roles_from_shares & 4 != 0);
}

/// Caller has no share + email → row returned with roles_bitmask=0
/// (service layer maps auth failure to NotFound to prevent enumeration)
pub async fn test_resolve_agent_type_no_share_returns_zero_roles(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    // Create a stranger account with no share
    let stranger = deps.create_account().await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            stranger.revision.account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            None,
            Some(&env.owner_email),
        )
        .await
        .unwrap();

    // Record returned but roles_bitmask = 0 (no share)
    // The service layer maps this to NotFound via auth check;
    // at repo level we still get the row back with roles_bitmask = 0
    let_assert!(Some(record) = result);
    check!(record.environment_roles_from_shares == 0);
    check!(record.owner_account_id == env.owner_account_id);
    check!(record.agent_type_name == env.agent_type_name);
}

/// Env exists but no current deployment (latest) → None
pub async fn test_resolve_agent_type_no_deployment_returns_none(deps: &Deps) {
    let email = format!("no-deploy-{}@golem.test", new_repo_uuid());
    let owner = deps.create_account_with_email(&email).await;
    let owner_account_id = owner.revision.account_id;
    let app_name = format!("no-deploy-app-{}", new_repo_uuid());
    let env_name = format!("no-deploy-env-{}", new_repo_uuid());

    let app = deps
        .application_repo
        .create(
            owner_account_id,
            ApplicationRevisionRecord {
                application_id: new_repo_uuid(),
                revision_id: 0,
                name: app_name.clone(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
            },
        )
        .await
        .unwrap();

    deps.environment_repo
        .create(
            app.revision.application_id,
            EnvironmentRevisionRecord {
                environment_id: new_repo_uuid(),
                revision_id: 0,
                name: env_name.clone(),
                audit: DeletableRevisionAuditFields::new(owner_account_id),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
                hash: SqlBlake3Hash::empty(),
            },
        )
        .await
        .unwrap();

    // No deployment created — resolve latest should return None
    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            owner_account_id,
            &app_name,
            &env_name,
            "SomeAgent",
            None, // latest
            None,
        )
        .await
        .unwrap();

    assert!(result.is_none());
}

/// Specific deployment revision not present → None
pub async fn test_resolve_agent_type_nonexistent_revision_returns_none(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            env.owner_account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            Some(9999), // non-existent revision
            None,
        )
        .await
        .unwrap();

    assert!(result.is_none());
}

/// Email doesn't exist → None (no existence leak)
pub async fn test_resolve_agent_type_unknown_email_returns_none(deps: &Deps) {
    let env = setup_resolve_env(deps).await;

    let grantee = deps.create_account().await;

    let result = deps
        .full_deployment_repo
        .resolve_agent_type_by_names(
            grantee.revision.account_id,
            &env.app_name,
            &env.env_name,
            &env.agent_type_name,
            None,
            Some("nonexistent-user@nowhere.example"),
        )
        .await
        .unwrap();

    assert!(result.is_none());
}
