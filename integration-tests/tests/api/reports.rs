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

use super::Tracing;
use assert2::{assert, let_assert};
use golem_client::api::{
    RegistryServiceClient, RegistryServiceCreateAccountError,
    RegistryServiceGetAccountCountReportError, RegistryServiceGetAccountSummariesReportError,
    RegistryServiceUpdateAccountError,
};
use golem_client::model::AccountRole;
use golem_client::model::UpdatedAccountData;
use golem_common::model::account::{AccountRevision, NewAccountData};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use test_r::{inherit_test_dep, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn normal_user_cannot_see_reports(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = deps.registry_service().client(&user.token).await;

    {
        let result = client.get_account_summaries_report().await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetAccountSummariesReportError::Error403(_)
            )) = result
        );
    }

    {
        let result = client.get_account_count_report().await;
        assert!(
            let Err(golem_client::Error::Item(
                RegistryServiceGetAccountCountReportError::Error403(_)
            )) = result
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn marketing_admin_can_see_reports(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user_with_roles(&[AccountRole::MarketingAdmin]).await?;
    let client = deps.registry_service().client(&user.token).await;

    {
        let result = client.get_account_summaries_report().await?;
        assert!(!result.values.is_empty())
    }

    {
        let result = client.get_account_count_report().await?;
        assert!(result.total_accounts >= 1);
        assert!(result.total_active_accounts >= 1);
    }

    Ok(())
}
