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
use chrono::{NaiveDateTime, Utc};
use golem_common::model::AccountId;
use golem_registry_service::repo::account::{AccountRecord, AccountRepo};
use golem_registry_service::repo::application::{ApplicationRepo, DbApplicationRepo};
use golem_registry_service::repo::plan::{PlanRecord, PlanRepository};
use golem_service_base::db::Pool;
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
                account_id: account_id.clone(),
                name: format!("Test Account {}", account_id),
                email: format!("test-{}@golem", account_id),
                plan_id: self.test_plan_id(),
            })
            .await
            .unwrap()
            .unwrap()
    }
}
