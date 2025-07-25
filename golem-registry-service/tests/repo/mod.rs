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
use golem_registry_service::repo::application::ApplicationRepo;
use golem_registry_service::repo::environment::EnvironmentRepo;
use golem_registry_service::repo::model::account::AccountRecord;
use golem_registry_service::repo::model::application::ApplicationRecord;
use golem_registry_service::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use golem_registry_service::repo::model::environment::{
    EnvironmentCurrentRevisionRecord, EnvironmentRevisionRecord,
};
use golem_registry_service::repo::model::plan::PlanRecord;
use golem_registry_service::repo::plan::PlanRepository;
use std::str::FromStr;
use std::sync::Arc;
use test_r::{inherit_test_dep, sequential_suite};
use uuid::Uuid;

pub mod common;
pub mod postgres;
pub mod sqlite;

inherit_test_dep!(Tracing);

sequential_suite!(postgres);
sequential_suite!(sqlite);

#[derive(Clone)]
pub struct Deps {
    pub account_repo: Arc<dyn AccountRepo>,
    pub application_repo: Arc<dyn ApplicationRepo>,
    pub environment_repo: Arc<dyn EnvironmentRepo>,
    pub plan_repo: Arc<dyn PlanRepository>,
}

impl Deps {
    pub async fn setup(&self) {
        self.plan_repo
            .create(PlanRecord {
                plan_id: self.test_plan_id(),
                name: "MAIN_TEST_PLAN".to_string(),
            })
            .await
            .unwrap();
    }

    pub fn test_plan_id(&self) -> Uuid {
        Uuid::from_str("e449dca1-cf07-4270-a8a2-6bcfc6528038").unwrap()
    }

    pub async fn create_account(&self) -> AccountRecord {
        let account_id = Uuid::new_v4();
        self.account_repo
            .create(AccountRecord {
                account_id,
                email: format!("test-{account_id}@golem"),
                audit: AuditFields::new(account_id),
                name: format!("Test Account {account_id}"),
                plan_id: self.test_plan_id(),
            })
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn create_application(&self) -> ApplicationRecord {
        let owner = self.create_account().await;
        let user = self.create_account().await;
        let app_name = format!("app-name-{}", Uuid::new_v4());

        self.application_repo
            .ensure(&user.account_id, &owner.account_id, &app_name)
            .await
            .unwrap()
    }

    pub async fn create_env(&self) -> EnvironmentCurrentRevisionRecord {
        let app = self.create_application().await;
        let env_name = format!("env-{}", Uuid::new_v4());
        self.environment_repo
            .create(
                &app.application_id,
                &env_name,
                EnvironmentRevisionRecord {
                    environment_id: Uuid::new_v4(),
                    revision_id: 0,
                    audit: DeletableRevisionAuditFields::new(app.audit.modified_by),
                    compatibility_check: false,
                    version_check: false,
                    security_overrides: false,
                    hash: blake3::hash("test".as_bytes()).into(),
                },
            )
            .await
            .unwrap()
            .unwrap()
    }
}
