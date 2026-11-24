// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::repo::report::ReportRepo;
use crate::services::decision::{Decisions, account_target};
use golem_common::model::authority::{CatalogOperation, Operation, Resource};
use golem_common::model::card::owner::EmptyOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, PermissionTarget, SystemResourcePattern, SystemVerb,
};
use golem_common::model::reports::{AccountCountsReport, AccountSummaryReport};
use golem_common::{SafeDisplay, error_forwarding};
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
    reports_repo: Arc<dyn ReportRepo>,
    decisions: Decisions,
}

impl ReportsService {
    pub fn new(reports_repo: Arc<dyn ReportRepo>, decisions: Decisions) -> Self {
        Self {
            reports_repo,
            decisions,
        }
    }

    /// Decides a report for the caller's own account, from today's system check.
    async fn authorize_report(
        &self,
        auth: &AuthCtx,
        operation: CatalogOperation,
        verb: SystemVerb,
    ) -> Result<(), AuthorizationError> {
        let target = account_target(auth.account_id());
        self.decisions
            .decide(
                auth,
                Operation::single(operation, Resource::None, target),
                system_permission_target(verb),
            )
            .await
            .map(|_| ())
    }
}

impl ReportsService {
    pub async fn get_account_summaries(
        &self,
        auth: &AuthCtx,
    ) -> Result<Vec<AccountSummaryReport>, ReportsError> {
        self.authorize_report(
            auth,
            CatalogOperation::ViewAccountSummariesReport,
            SystemVerb::ViewAccountSummariesReport,
        )
        .await?;

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
        self.authorize_report(
            auth,
            CatalogOperation::ViewAccountCountsReport,
            SystemVerb::ViewAccountCountsReport,
        )
        .await?;

        let account_counts = self.reports_repo.get_account_counts().await?.into();

        Ok(account_counts)
    }
}

fn system_permission_target(verb: SystemVerb) -> PermissionTarget {
    PermissionTarget::System(ClassPermissionTarget {
        verb: Some(verb),
        owner: EmptyOwnerPattern,
        resource: SystemResourcePattern,
    })
}
