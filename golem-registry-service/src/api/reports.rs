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

use crate::api::ApiResult;
use crate::services::auth::AuthService;
use crate::services::reports::ReportsService;
use golem_common::model::Page;
use golem_common::model::reports::{AccountCountsReport, AccountSummaryReport};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ReportsApi {
    reports_service: Arc<ReportsService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/reports",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Reports
)]
impl ReportsApi {
    pub fn new(reports_service: Arc<ReportsService>, auth_service: Arc<AuthService>) -> Self {
        Self {
            reports_service,
            auth_service,
        }
    }

    #[oai(
        path = "/account_summaries",
        method = "get",
        operation_id = "get_account_summaries_report"
    )]
    async fn get_account_summaries_report(
        &self,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<AccountSummaryReport>>> {
        let record = recorded_http_api_request!("get_account_summaries_report",);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_summaries_report_internal(auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_summaries_report_internal(
        &self,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<AccountSummaryReport>>> {
        let account_summaries = self.reports_service.get_account_summaries(&auth).await?;
        Ok(Json(Page {
            values: account_summaries,
        }))
    }

    #[oai(
        path = "/account_count",
        method = "get",
        operation_id = "get_account_count_report"
    )]
    async fn get_account_count_report(
        &self,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<AccountCountsReport>> {
        let record = recorded_http_api_request!("get_account_count_report",);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_count_report_internal(auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_count_report_internal(
        &self,
        auth: AuthCtx,
    ) -> ApiResult<Json<AccountCountsReport>> {
        let report = self.reports_service.get_account_counts(&auth).await?;
        Ok(Json(report))
    }
}
