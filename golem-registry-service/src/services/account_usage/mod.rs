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

use crate::repo::account_usage::AccountUsageRepo;
use crate::repo::model::account_usage::{AccountUsage as RepoAccountUsage, UsageType};
use crate::repo::model::datetime::SqlDateTime;
use crate::services::account_usage::error::AccountUsageError;
use golem_common::model::account::AccountId;
use std::sync::Arc;
use tracing::error;

pub mod error;

pub struct AccountUsage {
    account_usage: RepoAccountUsage,
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    acked: bool,
}

impl AccountUsage {
    pub fn ack(&mut self) {
        self.acked = true;
    }
}

impl Drop for AccountUsage {
    fn drop(&mut self) {
        if self.acked {
            return;
        }

        let account_usage = self.account_usage.clone();
        let account_usage_repo = self.account_usage_repo.clone();
        tokio::spawn(async move {
            if let Err(err) = account_usage_repo.rollback(&account_usage).await {
                error!("Failed to rollback account usage: {account_usage:?}, {err}");
            }
        });
    }
}

pub struct AccountUsageService {
    account_usage_repo: Arc<dyn AccountUsageRepo>,
}

impl AccountUsageService {
    pub fn new(account_usage_repo: Arc<dyn AccountUsageRepo>) -> Self {
        Self { account_usage_repo }
    }

    pub async fn add_application(
        &self,
        account_id: &AccountId,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalAppCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalAppCount, 1)?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_environment(
        &self,
        account_id: &AccountId,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalEnvCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalEnvCount, 1)?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_component(
        &self,
        account_id: &AccountId,
        component_size_bytes: i64,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self.get_account_usage(account_id, None).await?;

        self.add_checked(&mut account_usage, UsageType::TotalAppCount, 1)?;
        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes,
        )?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_component_version(
        &self,
        account_id: &AccountId,
        component_size_bytes: i64,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalComponentStorageBytes))
            .await?;

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes,
        )?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    async fn get_account_usage(
        &self,
        account_id: &AccountId,
        usage_type: Option<UsageType>,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let usage = match usage_type {
            Some(usage_type) => {
                self.account_usage_repo
                    .get_for_type(&account_id.0, &SqlDateTime::now(), usage_type)
                    .await?
            }
            None => {
                self.account_usage_repo
                    .get(&account_id.0, &SqlDateTime::now())
                    .await?
            }
        };

        match usage {
            Some(usage) => Ok(usage),
            None => Err(AccountUsageError::AccountNotfound(account_id.clone())),
        }
    }

    fn add_checked(
        &self,
        account_usage: &mut RepoAccountUsage,
        usage_type: UsageType,
        value: i64,
    ) -> Result<(), AccountUsageError> {
        if !account_usage.add_checked(usage_type, value)? {
            return Err(AccountUsageError::LimitExceeded {
                limit_name: format!("{usage_type:?}"),
                limit_value: account_usage.plan.limit(usage_type)?.unwrap_or(-1),
                current_value: account_usage.usage(usage_type),
            });
        }

        Ok(())
    }

    fn wrapped_account_usage(&self, account_usage: RepoAccountUsage) -> AccountUsage {
        AccountUsage {
            account_usage,
            account_usage_repo: self.account_usage_repo.clone(),
            acked: false,
        }
    }
}
