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
    MemoryLimit, StorageLimit, StorageUsage, StorageUsageHistory, StorageUsageMetrics,
    StorageUsagePeriod,
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
    pub memory_gb_seconds: u64,
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
            memory_gb_seconds: usage.memory_gb_seconds,
            durable_storage_gb_month: usage.durable_storage_gb_month,
            ephemeral_storage_gb_month: usage.ephemeral_storage_gb_month,
        }
    }
}

/// Column headings for the history table, which needs them even when there are no rows
/// to take them from. Kept in step with [`AccountUsageView::rendered_fields`] by
/// `account_usage_always_renders_the_same_labels_in_order`.
const ACCOUNT_USAGE_LABELS: [&str; 5] = [
    "Period",
    "Compute",
    "Memory",
    "Durable storage",
    "Ephemeral storage",
];

impl AccountUsageView {
    /// The single place usage values are turned into customer-visible strings. Both the
    /// detail view and the history table render through this, so the same usage can never
    /// be reported two different ways. Each label sits next to the value it names, so a
    /// figure cannot end up under the wrong heading.
    fn rendered_fields(&self) -> [(&'static str, String); 5] {
        [
            ("Period", self.period.to_string()),
            ("Compute", format!("{} GCU", self.compute_gcu)),
            ("Memory", format!("{} GB-seconds", self.memory_gb_seconds)),
            (
                "Durable storage",
                format!("{} GB-month", self.durable_storage_gb_month),
            ),
            (
                "Ephemeral storage",
                format!("{} GB-month", self.ephemeral_storage_gb_month),
            ),
        ]
    }
}

impl MessageWithFields for AccountUsageView {
    fn message(&self) -> String {
        format!("Storage usage for {}", self.period)
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        for (label, value) in self.rendered_fields() {
            fields.field(label, &value);
        }
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
        let mut table = new_table_full_condensed(
            ACCOUNT_USAGE_LABELS
                .iter()
                .map(|label| Column::new(*label))
                .collect(),
        );

        for usage in &self.usage {
            table.add_row(usage.rendered_fields().map(|(_, value)| value));
        }

        log_table(table);
    }
}

impl StructuredOutput for AccountUsageListView {
    const KIND: &'static str = "account.usage.history";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLimitsView {
    #[serde(flatten)]
    pub max_storage_per_agent: StorageLimit,
    pub max_memory_per_agent: MemoryLimit,
    pub monthly_memory_gb_seconds: MemoryLimit,
}

impl Masked for AccountLimitsView {}

impl AccountLimitsView {
    pub fn new(
        max_storage_per_agent: StorageLimit,
        max_memory_per_agent: MemoryLimit,
        monthly_memory_gb_seconds: MemoryLimit,
    ) -> Self {
        Self {
            max_storage_per_agent,
            max_memory_per_agent,
            monthly_memory_gb_seconds,
        }
    }
}

impl MessageWithFields for AccountLimitsView {
    fn message(&self) -> String {
        "Account storage limits".to_string()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let limit = &self.max_storage_per_agent;
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
        add_memory_limit_fields(
            &mut fields,
            "Max memory per agent",
            &self.max_memory_per_agent,
            "bytes",
        );
        add_memory_limit_fields(
            &mut fields,
            "Monthly memory",
            &self.monthly_memory_gb_seconds,
            "GB-seconds",
        );
        fields.build()
    }
}

fn add_memory_limit_fields(
    fields: &mut FieldsBuilder,
    label: &str,
    limit: &MemoryLimit,
    unit: &str,
) {
    fields
        .field(label, &format!("{} {unit}", limit.effective_value))
        .field(
            &format!("{label} plan default"),
            &format!("{} {unit}", limit.plan_default),
        )
        .field(
            &format!("{label} override"),
            &limit
                .override_value
                .map(|value| format!("{value} {unit}"))
                .unwrap_or_else(|| "(none)".to_string()),
        )
        .field(
            &format!("{label} ceiling"),
            &format!("{} {unit}", limit.ceiling),
        )
        .field(
            &format!("{label} user configurable"),
            &limit.user_configurable,
        );
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

#[cfg(test)]
mod tests {
    use super::{ACCOUNT_USAGE_LABELS, AccountUsageView, MessageWithFields};
    use golem_common::model::account_usage::StorageUsagePeriod;
    use proptest::prelude::*;
    use test_r::test;

    /// Usage magnitudes we actually bill on: exact zero, sub-GB fractions where a
    /// rounded format would silently truncate, and up through implausibly large.
    fn arb_usage_value() -> impl Strategy<Value = f64> {
        prop_oneof![
            Just(0.0),
            0.0f64..1.0,
            1.0f64..1_000_000.0,
            1_000_000.0f64..1e12,
        ]
    }

    fn arb_period() -> impl Strategy<Value = StorageUsagePeriod> {
        (1970i32..=9999, 1u32..=12).prop_map(|(year, month)| StorageUsagePeriod { year, month })
    }

    fn arb_usage() -> impl Strategy<Value = AccountUsageView> {
        (
            arb_usage_value(),
            any::<u64>(),
            arb_usage_value(),
            arb_usage_value(),
            arb_period(),
        )
            .prop_map(
                |(
                    compute_gcu,
                    memory_gb_seconds,
                    durable_storage_gb_month,
                    ephemeral_storage_gb_month,
                    period,
                )| {
                    AccountUsageView {
                        compute_gcu,
                        memory_gb_seconds,
                        durable_storage_gb_month,
                        ephemeral_storage_gb_month,
                        period,
                    }
                },
            )
    }

    fn sample_usage() -> AccountUsageView {
        AccountUsageView {
            compute_gcu: 1.5,
            memory_gb_seconds: 4,
            durable_storage_gb_month: 2.5,
            ephemeral_storage_gb_month: 3.5,
            period: StorageUsagePeriod {
                year: 2026,
                month: 4,
            },
        }
    }

    #[test]
    fn account_usage_renders_customer_visible_units() {
        let fields = sample_usage().fields();

        assert_eq!(
            fields,
            vec![
                ("Period".to_string(), "2026-04".to_string()),
                ("Compute".to_string(), "1.5 GCU".to_string()),
                ("Memory".to_string(), "4 GB-seconds".to_string()),
                ("Durable storage".to_string(), "2.5 GB-month".to_string()),
                ("Ephemeral storage".to_string(), "3.5 GB-month".to_string()),
            ]
        );
    }

    proptest! {
        /// The label set is a stable contract: no value may add, drop or reorder a field.
        /// Asserting against the constant the history table's headers are built from is
        /// what keeps the two commands labelled identically.
        #[test]
        fn account_usage_always_renders_the_same_labels_in_order(usage in arb_usage()) {
            let labels = usage.fields().into_iter().map(|(name, _)| name).collect::<Vec<_>>();

            prop_assert_eq!(
                labels,
                ACCOUNT_USAGE_LABELS.iter().map(|l| l.to_string()).collect::<Vec<_>>()
            );
        }

        /// The detail view and the history table must render identical strings for the
        /// same usage — this is the anti-drift guard between `usage show` and `usage history`.
        #[test]
        fn account_usage_detail_and_history_render_identically(usage in arb_usage()) {
            let detail = usage.fields().into_iter().map(|(_, value)| value).collect::<Vec<_>>();
            let history_row = usage
                .rendered_fields()
                .map(|(_, value)| value)
                .to_vec();

            prop_assert_eq!(detail, history_row);
        }

        /// Every metric carries its unit, and the number in front of that unit parses
        /// back to exactly the value we were given. This is what rules out a rounded
        /// format quietly under-reporting a small balance, and rules out a metric being
        /// rendered into the wrong row.
        #[test]
        fn account_usage_metrics_round_trip_with_their_units(usage in arb_usage()) {
            let fields = usage.fields();
            let expected: [(&str, &str, f64); 3] = [
                ("Compute", " GCU", usage.compute_gcu),
                ("Durable storage", " GB-month", usage.durable_storage_gb_month),
                ("Ephemeral storage", " GB-month", usage.ephemeral_storage_gb_month),
            ];

            for (label, unit, value) in expected {
                let rendered = fields
                    .iter()
                    .find(|(name, _)| name == label)
                    .map(|(_, rendered)| rendered.clone())
                    .expect("field must be present");

                let number = rendered
                    .strip_suffix(unit)
                    .ok_or_else(|| TestCaseError::fail(format!("{label} must end in '{unit}', got '{rendered}'")))?;

                prop_assert_eq!(
                    number.parse::<f64>().map_err(|e| TestCaseError::fail(e.to_string()))?,
                    value,
                    "{} must render losslessly, got '{}'",
                    label,
                    rendered
                );
            }
        }

        /// The period is always zero-padded YYYY-MM, for every month including single digits.
        #[test]
        fn account_usage_period_is_zero_padded(usage in arb_usage()) {
            let fields = usage.fields();
            let (_, period) = fields.first().expect("period must be the first field");

            let (year, month) = period
                .split_once('-')
                .ok_or_else(|| TestCaseError::fail(format!("period must be YYYY-MM, got '{period}'")))?;

            prop_assert_eq!(year.len(), 4, "year must be zero-padded, got '{}'", period);
            prop_assert_eq!(month.len(), 2, "month must be zero-padded, got '{}'", period);
            prop_assert_eq!(year.parse::<i32>().ok(), Some(usage.period.year));
            prop_assert_eq!(month.parse::<u32>().ok(), Some(usage.period.month));
        }
    }
}
