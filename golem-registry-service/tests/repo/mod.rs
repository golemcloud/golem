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
use golem_registry_service::repo::account::AccountRepo;
use golem_registry_service::repo::account_usage::AccountUsageRepo;
use golem_registry_service::repo::application::ApplicationRepo;
use golem_registry_service::repo::component::ComponentRepo;
use golem_registry_service::repo::environment::EnvironmentRepo;
use golem_registry_service::repo::http_api_definition::HttpApiDefinitionRepo;
use golem_registry_service::repo::http_api_deployment::HttpApiDeploymentRepo;
use golem_registry_service::repo::model::account::{
    AccountExtRevisionRecord, AccountRevisionRecord,
};
use golem_registry_service::repo::model::account_usage::UsageType;
use golem_registry_service::repo::model::application::ApplicationRecord;
use golem_registry_service::repo::model::audit::DeletableRevisionAuditFields;
use golem_registry_service::repo::model::environment::{
    EnvironmentExtRevisionRecord, EnvironmentRevisionRecord,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::model::plan::PlanRecord;
use golem_registry_service::repo::plan::PlanRepo;
use golem_registry_service::repo::plugin::PluginRepo;
use std::str::FromStr;
use test_r::{inherit_test_dep, sequential_suite};
use uuid::Uuid;

pub mod common;
pub mod postgres;
pub mod sqlite;

inherit_test_dep!(Tracing);

sequential_suite!(postgres);
sequential_suite!(sqlite);

pub struct Deps {
    pub account_repo: Box<dyn AccountRepo>,
    pub account_usage_repo: Box<dyn AccountUsageRepo>,
    pub application_repo: Box<dyn ApplicationRepo>,
    pub environment_repo: Box<dyn EnvironmentRepo>,
    pub plan_repo: Box<dyn PlanRepo>,
    pub component_repo: Box<dyn ComponentRepo>,
    pub http_api_definition_repo: Box<dyn HttpApiDefinitionRepo>,
    pub http_api_deployment_repo: Box<dyn HttpApiDeploymentRepo>,
    pub deployment_repo: Box<dyn HttpApiDeploymentRepo>,
    pub plugin_repo: Box<dyn PluginRepo>,
}

impl Deps {
    pub async fn setup(&self) {
        self.plan_repo
            .create_or_update(
                PlanRecord {
                    plan_id: self.test_plan_id(),
                    name: "MAIN_TEST_PLAN".to_string(),
                    limits: Default::default(),
                }
                .with_limit(UsageType::TotalAppCount, 3)
                .with_limit(UsageType::TotalEnvCount, 10)
                .with_limit(UsageType::TotalComponentCount, 15)
                .with_limit(UsageType::TotalWorkerCount, 20)
                .with_limit(UsageType::TotalComponentStorageBytes, 1000)
                .with_limit(UsageType::MonthlyGasLimit, 2000)
                .with_limit(UsageType::MonthlyComponentUploadLimitBytes, 3000),
            )
            .await
            .unwrap();
    }

    pub fn test_plan_id(&self) -> Uuid {
        Uuid::from_str("e449dca1-cf07-4270-a8a2-6bcfc6528038").unwrap()
    }

    pub async fn create_account(&self) -> AccountExtRevisionRecord {
        let account_id = new_repo_uuid();
        self.account_repo
            .create(AccountRevisionRecord {
                account_id,
                revision_id: 0,
                email: format!("test-{account_id}@golem"),
                audit: DeletableRevisionAuditFields::new(account_id),
                name: format!("Test Account {account_id}"),
                plan_id: self.test_plan_id(),
                roles: 0,
            })
            .await
            .unwrap()
    }

    pub async fn create_application(&self, owner_account_id: &Uuid) -> ApplicationRecord {
        let user = self.create_account().await;
        let app_name = format!("app-name-{}", new_repo_uuid());

        self.application_repo
            .ensure(&user.revision.account_id, owner_account_id, &app_name)
            .await
            .unwrap()
    }

    pub async fn create_env(&self, parent_application_id: &Uuid) -> EnvironmentExtRevisionRecord {
        let user = self.create_account().await;
        let env_name = format!("env-{}", new_repo_uuid());
        self.environment_repo
            .create(
                parent_application_id,
                &env_name,
                EnvironmentRevisionRecord {
                    environment_id: new_repo_uuid(),
                    revision_id: 0,
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    compatibility_check: true,
                    version_check: true,
                    security_overrides: true,
                    hash: blake3::hash("test".as_bytes()).into(),
                },
            )
            .await
            .unwrap()
            .unwrap()
    }
}
