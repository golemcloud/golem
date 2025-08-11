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

use crate::repo::model::account_usage::UsageType;
use crate::repo::model::new_repo_uuid;
use golem_service_base::repo::{RepoError, RepoResult};
use sqlx::FromRow;
use std::collections::BTreeMap;
use strum::IntoEnumIterator;
use uuid::Uuid;

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanRecord {
    pub plan_id: Uuid,
    pub name: String,

    #[sqlx(skip)]
    pub limits: BTreeMap<UsageType, Option<i64>>,
}

impl PlanRecord {
    pub fn new(name: String) -> Self {
        Self {
            plan_id: new_repo_uuid(),
            name,
            limits: BTreeMap::new(),
        }
    }

    pub fn limit(&self, usage_type: UsageType) -> RepoResult<Option<i64>> {
        match self.limits.get(&usage_type) {
            Some(limit) => Ok(limit.clone()),
            None => Err(RepoError::Internal(format!(
                "illegal state error: missing limit for {usage_type:?}",
            ))),
        }
    }

    pub fn set_limit(&mut self, usage_type: UsageType, value: i64) {
        self.limits.insert(usage_type, Some(value));
    }

    pub fn with_limit(mut self, usage_type: UsageType, value: i64) -> Self {
        self.limits.insert(usage_type, Some(value));
        self
    }

    pub fn add_limit_placeholders(&mut self) {
        for usage_type in UsageType::iter() {
            self.limits.entry(usage_type).or_insert(None);
        }
    }

    pub fn with_limit_placeholders(mut self) -> Self {
        self.add_limit_placeholders();
        self
    }
}
