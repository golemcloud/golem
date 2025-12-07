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

use crate::repo::reports::ReportsRepo;
use golem_common::model::reports::{AccountCountsReport, AccountSummaryReport};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::GlobalAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ReportsError {
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ReportsError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(ReportsError, RepoError);

pub struct ReportsService {
    reports_repo: Arc<dyn ReportsRepo>,
}

impl ReportsService {
    pub fn new(reports_repo: Arc<dyn ReportsRepo>) -> Self {
        Self { reports_repo }
    }
}

impl ReportsService {
    pub async fn get_account_summaries(
        &self,
        auth: &AuthCtx,
    ) -> Result<Vec<AccountSummaryReport>, ReportsError> {
        auth.authorize_global_action(GlobalAction::GetReports)?;

        let summaries = self
            .reports_repo
            .list_account_summaries()
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(summaries)
    }

    pub async fn get_account_counts(
        &self,
        auth: &AuthCtx,
    ) -> Result<AccountCountsReport, ReportsError> {
        auth.authorize_global_action(GlobalAction::GetReports)?;

        let account_counts = self.reports_repo.get_account_counts().await?.into();

        Ok(account_counts)
    }
}
