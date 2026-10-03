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
use crate::model::grant::{format_grants, grant_count};
use crate::model::masking::Masked;
use crate::model::text_format::*;
use chrono::SecondsFormat;
use golem_client::model::{Account, PermissionShare};
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AccountResourcePolicy, AccountUsage, AccountUsageMetrics, AdminResourceGrant, MemoryLimit,
    MeteringStatus, MonthlyLimitBehavior, ResourceLimitValue, StorageResourceLimitValue,
};
use golem_common::model::permission_share::PermissionShareId;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

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
pub struct AccountDeleteView {
    pub deleted: bool,
    pub account_id: AccountId,
}

impl Masked for AccountDeleteView {}

impl MessageWithFields for AccountDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted account {}",
            format_message_highlight(&self.account_id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        fields.fmt_field("Account ID", &self.account_id, format_main_id);
        fields.build()
    }
}

impl StructuredOutput for AccountDeleteView {
    const KIND: &'static str = "account.delete";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageView {
    #[serde(flatten)]
    pub usage: AccountUsageMetrics,
}

impl Masked for AccountUsageView {}

impl From<AccountUsage> for AccountUsageView {
    fn from(usage: AccountUsage) -> Self {
        Self { usage: usage.usage }
    }
}

impl From<AccountUsageMetrics> for AccountUsageView {
    fn from(usage: AccountUsageMetrics) -> Self {
        Self { usage }
    }
}

/// Column headings for the history table, which needs them even when there are no rows
/// to take them from. Kept in step with [`AccountUsageView::rendered_fields`] by
/// `account_usage_always_renders_the_same_labels_in_order`.
const ACCOUNT_USAGE_LABELS: [&str; 6] = [
    "Period",
    "As of",
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
    fn rendered_fields(&self) -> [(&'static str, String); 6] {
        [
            ("Period", self.usage.period.to_string()),
            (
                "As of",
                self.usage
                    .as_of
                    .to_rfc3339_opts(SecondsFormat::Millis, true),
            ),
            ("Compute", self.usage.format_compute()),
            ("Memory", self.usage.format_memory()),
            ("Durable storage", self.usage.format_durable_storage()),
            ("Ephemeral storage", self.usage.format_ephemeral_storage()),
        ]
    }
}

impl MessageWithFields for AccountUsageView {
    fn message(&self) -> String {
        format!("Account usage for {}", self.usage.period)
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
    pub policy: AccountResourcePolicy,
}

impl Masked for AccountLimitsView {}

impl AccountLimitsView {
    pub fn new(policy: AccountResourcePolicy) -> Self {
        Self { policy }
    }
}

impl MessageWithFields for AccountLimitsView {
    fn message(&self) -> String {
        "Account resource policy".to_string()
    }

    fn fields(&self) -> Vec<(String, String)> {
        let limit = &self.policy.max_storage_per_agent;
        let mut fields = FieldsBuilder::new();
        fields.fmt_field("Account ID", &self.policy.account_id, format_main_id);
        fields
            .field("Monthly usage mode", &self.policy.monthly_usage_mode)
            .field(
                "Overage allowed by plan",
                &self.policy.overage_allowed_by_plan,
            );
        add_monthly_limit_fields(
            &mut fields,
            "Monthly compute",
            self.policy.monthly.compute_gcu.metering,
            self.policy.monthly.compute_gcu.plan_amount,
            self.policy.monthly.compute_gcu.resolved_monthly_amount,
            self.policy.monthly.compute_gcu.usage,
            self.policy.monthly.compute_gcu.remaining,
            self.policy.monthly.compute_gcu.allow_overage_usage,
            self.policy.monthly.compute_gcu.unit,
            self.policy.monthly.compute_gcu.behavior,
        );
        add_monthly_limit_fields(
            &mut fields,
            "Monthly memory",
            self.policy.monthly.memory_gb_seconds.metering,
            self.policy.monthly.memory_gb_seconds.plan_amount,
            self.policy
                .monthly
                .memory_gb_seconds
                .resolved_monthly_amount,
            self.policy.monthly.memory_gb_seconds.usage,
            self.policy.monthly.memory_gb_seconds.remaining,
            self.policy.monthly.memory_gb_seconds.allow_overage_usage,
            self.policy.monthly.memory_gb_seconds.unit,
            self.policy.monthly.memory_gb_seconds.behavior,
        );
        add_monthly_limit_fields(
            &mut fields,
            "Monthly durable storage",
            self.policy.monthly.durable_storage_gb_month.metering,
            self.policy.monthly.durable_storage_gb_month.plan_amount,
            self.policy
                .monthly
                .durable_storage_gb_month
                .resolved_monthly_amount,
            self.policy.monthly.durable_storage_gb_month.usage,
            self.policy.monthly.durable_storage_gb_month.remaining,
            self.policy
                .monthly
                .durable_storage_gb_month
                .allow_overage_usage,
            self.policy.monthly.durable_storage_gb_month.unit,
            self.policy.monthly.durable_storage_gb_month.behavior,
        );
        add_monthly_limit_fields(
            &mut fields,
            "Monthly ephemeral storage",
            self.policy.monthly.ephemeral_storage_gb_month.metering,
            self.policy.monthly.ephemeral_storage_gb_month.plan_amount,
            self.policy
                .monthly
                .ephemeral_storage_gb_month
                .resolved_monthly_amount,
            self.policy.monthly.ephemeral_storage_gb_month.usage,
            self.policy.monthly.ephemeral_storage_gb_month.remaining,
            self.policy
                .monthly
                .ephemeral_storage_gb_month
                .allow_overage_usage,
            self.policy.monthly.ephemeral_storage_gb_month.unit,
            self.policy.monthly.ephemeral_storage_gb_month.behavior,
        );
        if let StorageResourceLimitValue::Disabled(disabled) = limit.effective_value {
            fields.field("Max storage per agent", &"disabled").field(
                "Max storage per agent disabled reason",
                &disabled.reason.to_string(),
            );
        } else {
            fields
                .field(
                    "Max storage per agent",
                    &format_storage_limit(&limit.effective_value, &limit.unit.to_string()),
                )
                .field(
                    "Max storage per agent plan default",
                    &format_limit(&limit.plan_default, &limit.unit.to_string()),
                )
                .field(
                    "Max storage per agent override",
                    &format_optional_limit(limit.override_value, &limit.unit.to_string()),
                )
                .field(
                    "Max storage per agent ceiling",
                    &format_limit(&limit.ceiling, &limit.unit.to_string()),
                )
                .field(
                    "Max storage per agent user configurable",
                    &limit.user_configurable,
                );
        }
        add_memory_limit_fields(
            &mut fields,
            "Max memory per agent",
            &self.policy.max_memory_per_agent,
        );
        add_active_admin_grants(&mut fields, &self.policy);
        fields.build()
    }
}

fn add_monthly_limit_fields<
    Usage: std::fmt::Display,
    Remaining: std::fmt::Display,
    Overage: std::fmt::Display,
>(
    fields: &mut FieldsBuilder,
    label: &str,
    metering: MeteringStatus,
    plan_amount: Option<u64>,
    resolved_monthly_amount: Option<u64>,
    usage: Option<Usage>,
    remaining: Option<Remaining>,
    allow_overage_usage: Option<Overage>,
    unit: impl std::fmt::Display,
    behavior: Option<MonthlyLimitBehavior>,
) {
    fields.field(&format!("{label} metering"), &metering.to_string());
    if let Some(plan_amount) = plan_amount {
        fields.field(
            &format!("{label} plan amount"),
            &format!("{plan_amount} {unit}"),
        );
    }
    if let Some(resolved_monthly_amount) = resolved_monthly_amount {
        fields.field(
            &format!("{label} resolved monthly amount"),
            &format!("{resolved_monthly_amount} {unit}"),
        );
    }
    if let Some(usage) = usage {
        fields.field(&format!("{label} usage"), &format!("{usage} {unit}"));
    }
    if let Some(remaining) = remaining {
        fields.field(
            &format!("{label} remaining"),
            &format!("{remaining} {unit}"),
        );
    }
    if let Some(allow_overage_usage) = allow_overage_usage {
        fields.field(
            &format!("{label} billable excess"),
            &format!("{allow_overage_usage} {unit}"),
        );
    }
    if let Some(behavior) = behavior {
        fields.field(&format!("{label} behavior"), &behavior.to_string());
    }
}

fn add_active_admin_grants(fields: &mut FieldsBuilder, policy: &AccountResourcePolicy) {
    let grants = [
        (
            "Monthly compute",
            policy.monthly.compute_gcu.active_admin_grant.as_ref(),
            policy.monthly.compute_gcu.unit.to_string(),
        ),
        (
            "Monthly memory",
            policy.monthly.memory_gb_seconds.active_admin_grant.as_ref(),
            policy.monthly.memory_gb_seconds.unit.to_string(),
        ),
        (
            "Monthly durable storage",
            policy
                .monthly
                .durable_storage_gb_month
                .active_admin_grant
                .as_ref(),
            policy.monthly.durable_storage_gb_month.unit.to_string(),
        ),
        (
            "Monthly ephemeral storage",
            policy
                .monthly
                .ephemeral_storage_gb_month
                .active_admin_grant
                .as_ref(),
            policy.monthly.ephemeral_storage_gb_month.unit.to_string(),
        ),
        (
            "Max memory per agent",
            policy.max_memory_per_agent.active_admin_grant.as_ref(),
            policy.max_memory_per_agent.unit.to_string(),
        ),
        (
            "Max storage per agent",
            policy.max_storage_per_agent.active_admin_grant.as_ref(),
            policy.max_storage_per_agent.unit.to_string(),
        ),
    ];

    let mut rendered_any = false;
    for (dimension, grant, unit) in grants {
        if let Some(grant) = grant {
            fields.field(
                "Active admin grant",
                &format_active_admin_grant(dimension, grant, &unit),
            );
            rendered_any = true;
        }
    }
    if !rendered_any {
        fields.field("Active admin grant", &"(none)");
    }
}

fn format_active_admin_grant(dimension: &str, grant: &AdminResourceGrant, unit: &str) -> String {
    let granted_at = grant
        .granted_at
        .to_rfc3339_opts(SecondsFormat::AutoSi, true);
    let mut rendered = format!(
        "{dimension}: {} {unit}; reason: {}; actor: {}; granted at: {granted_at}",
        grant.value, grant.reason, grant.actor_account_id
    );
    if let Some(expires_at) = grant.expires_at {
        write!(
            rendered,
            "; expires at: {}",
            expires_at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
        )
        .expect("writing to a String cannot fail");
    } else {
        rendered.push_str("; no expiry");
    }
    rendered
}

fn format_optional_limit(value: Option<ResourceLimitValue>, unit: &str) -> String {
    value
        .map(|value| format_limit(&value, unit))
        .unwrap_or_else(|| "(none)".to_string())
}

fn format_limit(value: &ResourceLimitValue, unit: &str) -> String {
    match value {
        ResourceLimitValue::Finite(value) => format!("{} {unit}", value.value),
        ResourceLimitValue::Unlimited(_) => "unlimited".to_string(),
    }
}

fn format_storage_limit(value: &StorageResourceLimitValue, unit: &str) -> String {
    match value {
        StorageResourceLimitValue::Finite(value) => format!("{} {unit}", value.value),
        StorageResourceLimitValue::Unlimited(_) => "unlimited".to_string(),
        StorageResourceLimitValue::Disabled(_) => "disabled".to_string(),
    }
}

fn add_memory_limit_fields(fields: &mut FieldsBuilder, label: &str, limit: &MemoryLimit) {
    let unit = limit.unit.to_string();
    fields
        .field(label, &format_limit(&limit.effective_value, &unit))
        .field(
            &format!("{label} plan default"),
            &format_limit(&limit.plan_default, &unit),
        )
        .field(
            &format!("{label} override"),
            &format_optional_limit(limit.override_value, &unit),
        )
        .field(
            &format!("{label} ceiling"),
            &format_limit(&limit.ceiling, &unit),
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
pub struct PermissionShareDeleteView {
    pub deleted: bool,
    pub permission_share_id: PermissionShareId,
}

impl Masked for PermissionShareDeleteView {}

impl MessageWithFields for PermissionShareDeleteView {
    fn message(&self) -> String {
        format!(
            "Deleted permission share {}",
            format_message_highlight(&self.permission_share_id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();
        fields.fmt_field(
            "Permission share ID",
            &self.permission_share_id,
            format_main_id,
        );
        fields.build()
    }
}

impl StructuredOutput for PermissionShareDeleteView {
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

#[cfg(test)]
mod tests {
    use super::{
        ACCOUNT_USAGE_LABELS, AccountLimitsView, AccountUsageView, MessageWithFields,
        format_active_admin_grant, format_limit, format_optional_limit, format_storage_limit,
    };
    use chrono::{DateTime, TimeZone, Timelike, Utc};
    use golem_common::model::account_usage::{
        AccountResourcePolicy, AccountUsageMetering, AccountUsageMetrics, AccountUsagePeriod,
        AdminResourceGrant, AdminResourceGrantDimension, AdminResourceGrantReason, MemoryLimit,
        MeteringStatus, MonthlyComputeLimit, MonthlyComputeUnit, MonthlyLimitBehavior,
        MonthlyMemoryLimit, MonthlyMemoryUnit, MonthlyResourceLimits, MonthlyStorageLimit,
        MonthlyStorageUnit, MonthlyUsageMode, PerAgentLimitUnit, ResourceLimitValue, StorageLimit,
        StorageResourceLimitValue,
    };
    use proptest::prelude::*;
    use test_r::test;
    use uuid::uuid;

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

    fn arb_period() -> impl Strategy<Value = AccountUsagePeriod> {
        (1970i32..=9999, 1u32..=12).prop_map(|(year, month)| AccountUsagePeriod { year, month })
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
                        usage: AccountUsageMetrics {
                            compute_gcu,
                            memory_gb_seconds,
                            durable_storage_gb_month,
                            ephemeral_storage_gb_month,
                            period,
                            as_of: Utc.with_ymd_and_hms(2026, 4, 2, 3, 4, 5).unwrap(),
                            metering: AccountUsageMetering {
                                compute: MeteringStatus::Enabled,
                                memory: MeteringStatus::Enabled,
                                durable_storage: MeteringStatus::Enabled,
                                ephemeral_storage: MeteringStatus::Enabled,
                            },
                        },
                    }
                },
            )
    }

    fn sample_usage() -> AccountUsageView {
        AccountUsageView {
            usage: AccountUsageMetrics {
                compute_gcu: 1.5,
                memory_gb_seconds: 4,
                durable_storage_gb_month: 2.5,
                ephemeral_storage_gb_month: 3.5,
                period: AccountUsagePeriod {
                    year: 2026,
                    month: 4,
                },
                as_of: Utc.with_ymd_and_hms(2026, 4, 2, 3, 4, 5).unwrap(),
                metering: AccountUsageMetering {
                    compute: MeteringStatus::Enabled,
                    memory: MeteringStatus::Enabled,
                    durable_storage: MeteringStatus::Enabled,
                    ephemeral_storage: MeteringStatus::Enabled,
                },
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
                ("As of".to_string(), "2026-04-02T03:04:05.000Z".to_string()),
                ("Compute".to_string(), "1.5 GCU".to_string()),
                ("Memory".to_string(), "4 GB-seconds".to_string()),
                ("Durable storage".to_string(), "2.5 GB-month".to_string()),
                ("Ephemeral storage".to_string(), "3.5 GB-month".to_string()),
            ]
        );
    }

    /// The view reports compute, memory and both storage kinds, so the heading names the
    /// account rather than any single one of them.
    #[test]
    fn account_usage_heading_names_the_account_and_period() {
        assert_eq!(sample_usage().message(), "Account usage for 2026-04");
    }

    fn admin_grant(
        dimension: AdminResourceGrantDimension,
        value: u64,
        expires_at: Option<DateTime<Utc>>,
    ) -> AdminResourceGrant {
        AdminResourceGrant {
            dimension,
            value,
            reason: AdminResourceGrantReason::Support,
            actor_account_id: golem_common::model::account::AccountId(uuid!(
                "f30c30f7-c541-4386-bc8c-de92a5607ca2"
            )),
            granted_at: Utc.with_ymd_and_hms(2026, 4, 1, 2, 3, 4).unwrap(),
            expires_at,
        }
    }

    fn account_resource_policy_with_all_grants() -> AccountResourcePolicy {
        AccountResourcePolicy {
            account_id: golem_common::model::account::AccountId(uuid!(
                "e71a6160-4144-4720-9e34-e5943458d129"
            )),
            monthly_usage_mode: MonthlyUsageMode::HardLimit,
            overage_allowed_by_plan: false,
            latest_owner_transition: None,
            monthly: MonthlyResourceLimits {
                compute_gcu: MonthlyComputeLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(10),
                    active_admin_grant: Some(admin_grant(
                        AdminResourceGrantDimension::MonthlyComputeGcu,
                        12,
                        Some(Utc.with_ymd_and_hms(2026, 5, 1, 2, 3, 4).unwrap()),
                    )),
                    resolved_monthly_amount: Some(12),
                    usage: Some(3.5),
                    remaining: Some(8.5),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyComputeUnit::Gcu,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                memory_gb_seconds: MonthlyMemoryLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(20),
                    active_admin_grant: Some(admin_grant(
                        AdminResourceGrantDimension::MonthlyMemoryGbSeconds,
                        22,
                        None,
                    )),
                    resolved_monthly_amount: Some(22),
                    usage: Some(7),
                    remaining: Some(13),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyMemoryUnit::GbSeconds,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                durable_storage_gb_month: MonthlyStorageLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(30),
                    active_admin_grant: Some(admin_grant(
                        AdminResourceGrantDimension::MonthlyDurableStorageGbMonth,
                        32,
                        None,
                    )),
                    resolved_monthly_amount: Some(32),
                    usage: Some(11.25),
                    remaining: Some(18.75),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyStorageUnit::GbMonth,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                ephemeral_storage_gb_month: MonthlyStorageLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(40),
                    active_admin_grant: Some(admin_grant(
                        AdminResourceGrantDimension::MonthlyEphemeralStorageGbMonth,
                        42,
                        None,
                    )),
                    resolved_monthly_amount: Some(42),
                    usage: Some(2.5),
                    remaining: Some(37.5),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyStorageUnit::GbMonth,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
            },
            max_storage_per_agent: StorageLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: Some(admin_grant(
                    AdminResourceGrantDimension::MaxStoragePerAgent,
                    62,
                    None,
                )),
                effective_value: StorageResourceLimitValue::from_storage_value(62),
                plan_default: ResourceLimitValue::from_storage_value(5),
                override_value: None,
                ceiling: ResourceLimitValue::from_storage_value(20),
                user_configurable: true,
            },
            max_memory_per_agent: MemoryLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: Some(admin_grant(
                    AdminResourceGrantDimension::MaxMemoryPerAgent,
                    52,
                    None,
                )),
                effective_value: ResourceLimitValue::from_memory_value(52),
                plan_default: ResourceLimitValue::from_memory_value(25),
                override_value: Some(ResourceLimitValue::from_memory_value(30)),
                ceiling: ResourceLimitValue::from_memory_value(40),
                user_configurable: true,
            },
        }
    }

    #[test]
    fn account_limits_render_all_enabled_values() {
        let limits = AccountLimitsView::new(account_resource_policy_with_all_grants());

        assert_eq!(limits.message(), "Account resource policy");
        let fields = limits.fields();
        for expected in [
            ("Monthly usage mode", "hardLimit"),
            ("Overage allowed by plan", "false"),
            ("Monthly compute plan amount", "10 GCU"),
            ("Monthly compute resolved monthly amount", "12 GCU"),
            ("Monthly compute usage", "3.5 GCU"),
            ("Monthly compute remaining", "8.5 GCU"),
            ("Monthly compute billable excess", "0 GCU"),
            ("Monthly compute behavior", "hardLimit"),
            ("Monthly memory plan amount", "20 GB-seconds"),
            ("Monthly memory resolved monthly amount", "22 GB-seconds"),
            ("Monthly memory usage", "7 GB-seconds"),
            ("Monthly durable storage plan amount", "30 GB-month"),
            ("Monthly durable storage usage", "11.25 GB-month"),
            ("Monthly ephemeral storage plan amount", "40 GB-month"),
            ("Monthly ephemeral storage remaining", "37.5 GB-month"),
            ("Max storage per agent", "62 bytes"),
            ("Max memory per agent", "52 bytes"),
        ] {
            assert!(
                fields
                    .iter()
                    .any(|(name, value)| name == expected.0 && value == expected.1),
                "missing field {expected:?} in {fields:?}"
            );
        }
    }

    #[test]
    fn account_limits_render_all_admin_grants_in_policy_order() {
        let grant_rows = AccountLimitsView::new(account_resource_policy_with_all_grants())
            .fields()
            .into_iter()
            .filter(|(name, _)| name == "Active admin grant")
            .collect::<Vec<_>>();

        assert_eq!(
            grant_rows,
            vec![
                (
                    "Active admin grant".to_string(),
                    "Monthly compute: 12 GCU; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04Z; expires at: 2026-05-01T02:03:04Z".to_string(),
                ),
                (
                    "Active admin grant".to_string(),
                    "Monthly memory: 22 GB-seconds; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04Z; no expiry".to_string(),
                ),
                (
                    "Active admin grant".to_string(),
                    "Monthly durable storage: 32 GB-month; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04Z; no expiry".to_string(),
                ),
                (
                    "Active admin grant".to_string(),
                    "Monthly ephemeral storage: 42 GB-month; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04Z; no expiry".to_string(),
                ),
                (
                    "Active admin grant".to_string(),
                    "Max memory per agent: 52 bytes; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04Z; no expiry".to_string(),
                ),
                (
                    "Active admin grant".to_string(),
                    "Max storage per agent: 62 bytes; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04Z; no expiry".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn admin_grant_timestamps_preserve_fractional_precision() {
        let mut grant = admin_grant(AdminResourceGrantDimension::MonthlyComputeGcu, 12, None);
        grant.granted_at = Utc
            .with_ymd_and_hms(2026, 4, 1, 2, 3, 4)
            .unwrap()
            .with_nanosecond(123_456_789)
            .unwrap();
        grant.expires_at = Some(
            Utc.with_ymd_and_hms(2026, 5, 1, 2, 3, 4)
                .unwrap()
                .with_nanosecond(987_654_321)
                .unwrap(),
        );

        assert_eq!(
            format_active_admin_grant("Monthly compute", &grant, "GCU"),
            "Monthly compute: 12 GCU; reason: support; actor: f30c30f7-c541-4386-bc8c-de92a5607ca2; granted at: 2026-04-01T02:03:04.123456789Z; expires at: 2026-05-01T02:03:04.987654321Z"
        );
    }

    #[test]
    fn account_limits_render_one_empty_admin_grant_row() {
        let mut policy = account_resource_policy_with_all_grants();
        policy.monthly.compute_gcu.active_admin_grant = None;
        policy.monthly.memory_gb_seconds.active_admin_grant = None;
        policy.monthly.durable_storage_gb_month.active_admin_grant = None;
        policy.monthly.ephemeral_storage_gb_month.active_admin_grant = None;
        policy.max_memory_per_agent.active_admin_grant = None;
        policy.max_storage_per_agent.active_admin_grant = None;

        let grant_rows = AccountLimitsView::new(policy)
            .fields()
            .into_iter()
            .filter(|(name, _)| name == "Active admin grant")
            .collect::<Vec<_>>();

        assert_eq!(
            grant_rows,
            vec![("Active admin grant".to_string(), "(none)".to_string())]
        );
    }

    #[test]
    fn account_limits_structured_output_is_the_canonical_policy() {
        let policy = account_resource_policy_with_all_grants();
        let limits = AccountLimitsView::new(policy.clone());

        assert_eq!(
            serde_json::to_value(limits).unwrap(),
            serde_json::to_value(policy).unwrap()
        );
    }

    #[test]
    fn special_per_agent_limits_render_without_units() {
        assert_eq!(format_optional_limit(None, "bytes"), "(none)");
        assert_eq!(
            format_optional_limit(Some(ResourceLimitValue::from_memory_value(10)), "bytes"),
            "10 bytes"
        );
        assert_eq!(
            format_optional_limit(
                Some(ResourceLimitValue::from_memory_value(u64::MAX)),
                "bytes",
            ),
            "unlimited"
        );
        assert_eq!(
            format_limit(&ResourceLimitValue::from_memory_value(u64::MAX), "bytes"),
            "unlimited"
        );
        assert_eq!(
            format_storage_limit(
                &StorageResourceLimitValue::from_storage_value(
                    golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
                ),
                "bytes",
            ),
            "unlimited"
        );
        assert_eq!(
            format_storage_limit(&StorageResourceLimitValue::disabled(), "bytes"),
            "disabled"
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
                ("Compute", " GCU", usage.usage.compute_gcu),
                ("Durable storage", " GB-month", usage.usage.durable_storage_gb_month),
                ("Ephemeral storage", " GB-month", usage.usage.ephemeral_storage_gb_month),
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
            prop_assert_eq!(year.parse::<i32>().ok(), Some(usage.usage.period.year));
            prop_assert_eq!(month.parse::<u32>().ok(), Some(usage.usage.period.month));
        }
    }
}
