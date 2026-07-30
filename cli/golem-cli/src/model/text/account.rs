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

use crate::model::cli_output::StructuredOutput;
use crate::model::masking::Masked;
use crate::model::text::fmt::*;
use golem_client::model::{Account, PermissionShare};
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    StorageLimit, StorageUsage, StorageUsageHistory, StorageUsageMetrics, StorageUsagePeriod,
};
use golem_common::model::permission_share::{PermissionShareData, PermissionShareId};
use serde::{Deserialize, Serialize};

fn account_fields(account: &Account) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Account ID", &account.id, format_main_id)
        .fmt_field("E-mail", &account.email, format_id)
        .field("Name", &account.name);

    fields.build()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountGetView(pub Account);

impl Masked for AccountGetView {}

impl MessageWithFields for AccountGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for account {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        account_fields(&self.0)
    }
}

impl StructuredOutput for AccountGetView {
    const KIND: &'static str = "account.get";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountNewView(pub Account);

impl Masked for AccountNewView {}

impl MessageWithFields for AccountNewView {
    fn message(&self) -> String {
        format!(
            "Created new account {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        account_fields(&self.0)
    }
}

impl StructuredOutput for AccountNewView {
    const KIND: &'static str = "account.new";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountUpdateView(pub Account);

impl Masked for AccountUpdateView {}

impl MessageWithFields for AccountUpdateView {
    fn message(&self) -> String {
        format!("Updated account {}", format_message_highlight(&self.0.id))
    }

    fn fields(&self) -> Vec<(String, String)> {
        account_fields(&self.0)
    }
}

impl StructuredOutput for AccountUpdateView {
    const KIND: &'static str = "account.update";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDeleteResult {
    pub deleted: bool,
    pub account_id: AccountId,
}

impl NoTextOutput for AccountDeleteResult {}
impl TextOutput for AccountDeleteResult {}

impl StructuredOutput for AccountDeleteResult {
    const KIND: &'static str = "account.delete";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageView {
    pub compute_gcu: f64,
    pub durable_storage_gb_month: f64,
    pub ephemeral_storage_gb_month: f64,
    pub period: StorageUsagePeriod,
}

impl Masked for AccountUsageView {}

impl From<StorageUsage> for AccountUsageView {
    fn from(usage: StorageUsage) -> Self {
        usage.usage.into()
    }
}

impl From<StorageUsageHistory> for AccountUsageView {
    fn from(usage: StorageUsageHistory) -> Self {
        usage.usage.into()
    }
}

impl From<StorageUsageMetrics> for AccountUsageView {
    fn from(usage: StorageUsageMetrics) -> Self {
        Self {
            period: usage.period,
            compute_gcu: usage.compute_gcu,
            durable_storage_gb_month: usage.durable_storage_gb_month,
            ephemeral_storage_gb_month: usage.ephemeral_storage_gb_month,
        }
    }
}

impl MessageWithFields for AccountUsageView {
    fn message(&self) -> String {
        format!("Storage usage for {}", self.period)
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        let period = self.period.to_string();
        fields
            .field("Period", &period)
            .field("Compute", &format!("{} GCU", self.compute_gcu))
            .field(
                "Durable storage",
                &format!("{} GB-month", self.durable_storage_gb_month),
            )
            .field(
                "Ephemeral storage",
                &format!("{} GB-month", self.ephemeral_storage_gb_month),
            );
        fields.build()
    }
}

impl StructuredOutput for AccountUsageView {
    const KIND: &'static str = "account.usage.show";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageListView {
    pub usage: Vec<AccountUsageView>,
}

impl TextOutput for AccountUsageListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Period"),
            Column::new("Compute"),
            Column::new("Durable storage"),
            Column::new("Ephemeral storage"),
        ]);

        for usage in &self.usage {
            table.add_row(vec![
                usage.period.to_string(),
                format!("{} GCU", usage.compute_gcu),
                format!("{} GB-month", usage.durable_storage_gb_month),
                format!("{} GB-month", usage.ephemeral_storage_gb_month),
            ]);
        }

        log_table(table);
    }
}

impl StructuredOutput for AccountUsageListView {
    const KIND: &'static str = "account.usage.history";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountLimitsView(pub StorageLimit);

impl Masked for AccountLimitsView {}

impl From<StorageLimit> for AccountLimitsView {
    fn from(limit: StorageLimit) -> Self {
        Self(limit)
    }
}

impl MessageWithFields for AccountLimitsView {
    fn message(&self) -> String {
        "Account storage limits".to_string()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let limit = &self.0;
        let mut fields = FieldsBuilder::new();
        fields
            .field(
                "Max storage per agent",
                &format!("{} bytes", limit.effective_value),
            )
            .field("Plan default", &format!("{} bytes", limit.plan_default))
            .field(
                "Override",
                &limit
                    .override_value
                    .map(|value| format!("{value} bytes"))
                    .unwrap_or_else(|| "(none)".to_string()),
            )
            .field("Ceiling", &format!("{} bytes", limit.ceiling))
            .field("User configurable", &limit.user_configurable);
        fields.build()
    }
}

impl StructuredOutput for AccountLimitsView {
    const KIND: &'static str = "account.limits.show";
}

fn permission_share_fields(share: &PermissionShare) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Permission share ID", &share.id, format_main_id)
        .field("Name", &share.name)
        .field("Revision", &share.revision)
        .fmt_field("Owner account ID", &share.owner_account_id, format_id)
        .fmt_field("Target account ID", &share.target_account_id, format_id)
        .field("Lower positive", &format_grants(&share.data.lower_positive))
        .field("Lower negative", &format_grants(&share.data.lower_negative));

    fields.build()
}

fn format_grants(grants: &[String]) -> String {
    if grants.is_empty() {
        "(none)".to_string()
    } else {
        grants.join("\n")
    }
}

fn grant_count(data: &PermissionShareData) -> usize {
    data.lower_positive.len() + data.lower_negative.len()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareGetView(pub PermissionShare);

impl Masked for PermissionShareGetView {}

impl MessageWithFields for PermissionShareGetView {
    fn message(&self) -> String {
        format!(
            "Got permission share {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        permission_share_fields(&self.0)
    }
}

impl StructuredOutput for PermissionShareGetView {
    const KIND: &'static str = "account.permission-share.get";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareNewView(pub PermissionShare);

impl Masked for PermissionShareNewView {}

impl MessageWithFields for PermissionShareNewView {
    fn message(&self) -> String {
        format!(
            "Created permission share {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        permission_share_fields(&self.0)
    }
}

impl StructuredOutput for PermissionShareNewView {
    const KIND: &'static str = "account.permission-share.new";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareUpdateView(pub PermissionShare);

impl Masked for PermissionShareUpdateView {}

impl MessageWithFields for PermissionShareUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated permission share {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        permission_share_fields(&self.0)
    }
}

impl StructuredOutput for PermissionShareUpdateView {
    const KIND: &'static str = "account.permission-share.update";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionShareDeleteResult {
    pub deleted: bool,
    pub permission_share_id: PermissionShareId,
}

impl NoTextOutput for PermissionShareDeleteResult {}
impl TextOutput for PermissionShareDeleteResult {}

impl StructuredOutput for PermissionShareDeleteResult {
    const KIND: &'static str = "account.permission-share.delete";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionShareListView {
    pub permission_shares: Vec<PermissionShare>,
}

impl TextOutput for PermissionShareListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("ID"),
            Column::new("Name"),
            Column::new("Owner").fixed(),
            Column::new("Target").fixed(),
            Column::new("Grants").fixed(),
        ]);

        for share in &self.permission_shares {
            table.add_row(vec![
                share.id.to_string(),
                share.name.to_string(),
                share.owner_account_id.to_string(),
                share.target_account_id.to_string(),
                grant_count(&share.data).to_string(),
            ]);
        }

        log_table(table);
    }
}

impl StructuredOutput for PermissionShareListView {
    const KIND: &'static str = "account.permission-share.list";
}

// TODO: atomic
/*
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct GrantGetView(pub Vec<Role>);

impl TextRender for GrantGetView {
    fn log(&self) {
        if self.0.is_empty() {
            logln("No roles granted")
        } else {
            logln("Granted roles:");
            for role in &self.0 {
                logln(format!("  - {role}"));
            }
        }
    }
}
*/
