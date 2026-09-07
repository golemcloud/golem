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

use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AdminResourceGrant, AdminResourceGrantChange, AdminResourceGrantDimension,
    AdminResourceGrantEventType, AdminResourceGrantReason,
};
use golem_service_base::repo::{NumericU64, RepoError, RepoResult, SqlDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountResourceOverrideDimension {
    MonthlyComputeGcu,
    MonthlyMemoryGbSeconds,
    MonthlyDurableStorageGbMonth,
    MonthlyEphemeralStorageGbMonth,
    MaxDiskSpacePerWorker,
    MaxMemoryPerWorker,
}

impl AccountResourceOverrideDimension {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MonthlyComputeGcu => "monthly_compute_gcu",
            Self::MonthlyMemoryGbSeconds => "monthly_memory_gb_seconds",
            Self::MonthlyDurableStorageGbMonth => "monthly_durable_storage_gb_month",
            Self::MonthlyEphemeralStorageGbMonth => "monthly_ephemeral_storage_gb_month",
            Self::MaxDiskSpacePerWorker => "max_disk_space_per_worker",
            Self::MaxMemoryPerWorker => "max_memory_per_worker",
        }
    }
}

impl From<AdminResourceGrantDimension> for AccountResourceOverrideDimension {
    fn from(value: AdminResourceGrantDimension) -> Self {
        match value {
            AdminResourceGrantDimension::MonthlyComputeGcu => Self::MonthlyComputeGcu,
            AdminResourceGrantDimension::MonthlyMemoryGbSeconds => Self::MonthlyMemoryGbSeconds,
            AdminResourceGrantDimension::MonthlyDurableStorageGbMonth => {
                Self::MonthlyDurableStorageGbMonth
            }
            AdminResourceGrantDimension::MonthlyEphemeralStorageGbMonth => {
                Self::MonthlyEphemeralStorageGbMonth
            }
            AdminResourceGrantDimension::MaxMemoryPerAgent => Self::MaxMemoryPerWorker,
            AdminResourceGrantDimension::MaxStoragePerAgent => Self::MaxDiskSpacePerWorker,
        }
    }
}

impl From<AccountResourceOverrideDimension> for AdminResourceGrantDimension {
    fn from(value: AccountResourceOverrideDimension) -> Self {
        match value {
            AccountResourceOverrideDimension::MonthlyComputeGcu => Self::MonthlyComputeGcu,
            AccountResourceOverrideDimension::MonthlyMemoryGbSeconds => {
                Self::MonthlyMemoryGbSeconds
            }
            AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth => {
                Self::MonthlyDurableStorageGbMonth
            }
            AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth => {
                Self::MonthlyEphemeralStorageGbMonth
            }
            AccountResourceOverrideDimension::MaxMemoryPerWorker => Self::MaxMemoryPerAgent,
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker => Self::MaxStoragePerAgent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountResourceOverrideSource {
    SelfService,
    AdminGrant,
}

impl AccountResourceOverrideSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SelfService => "self_service",
            Self::AdminGrant => "admin_grant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountResourceOverrideReason {
    UserSelfServe,
    DowngradeClamp,
    Promotional,
    Support,
}

impl AccountResourceOverrideReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserSelfServe => "user_self_serve",
            Self::DowngradeClamp => "downgrade_clamp",
            Self::Promotional => "promotional",
            Self::Support => "support",
        }
    }
}

impl From<AdminResourceGrantReason> for AccountResourceOverrideReason {
    fn from(value: AdminResourceGrantReason) -> Self {
        match value {
            AdminResourceGrantReason::Promotional => Self::Promotional,
            AdminResourceGrantReason::Support => Self::Support,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountResourceOverrideRecord {
    pub account_id: Uuid,
    pub dimension: AccountResourceOverrideDimension,
    pub source: AccountResourceOverrideSource,
    pub override_value: NumericU64,
    pub reason: AccountResourceOverrideReason,
    pub expires_at: Option<SqlDateTime>,
    pub created_by: Uuid,
    pub created_at: SqlDateTime,
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AccountResourceOverrideDbRecord {
    pub account_id: Uuid,
    pub dimension: String,
    pub override_value: NumericU64,
    pub reason: String,
    pub expires_at: Option<SqlDateTime>,
    pub created_by: Uuid,
    pub created_at: SqlDateTime,
}

impl AccountResourceOverrideDbRecord {
    pub fn into_admin_grant(self) -> RepoResult<AdminResourceGrant> {
        Ok(AdminResourceGrant {
            dimension: persisted_dimension(&self.dimension)?.into(),
            value: self.override_value.get(),
            reason: persisted_admin_reason(&self.reason)?,
            actor_account_id: AccountId(self.created_by),
            granted_at: self.created_at.into_utc(),
            expires_at: self.expires_at.map(SqlDateTime::into_utc),
        })
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AccountResourceOverrideEventRecord {
    pub account_id: Uuid,
    pub dimension: String,
    pub event_type: String,
    pub reason: String,
    pub actor_account_id: Uuid,
    pub changed_at: SqlDateTime,
    pub old_value: NumericU64,
    pub new_value: NumericU64,
    pub expires_at: Option<SqlDateTime>,
}

impl AccountResourceOverrideEventRecord {
    pub fn into_public(self) -> RepoResult<AdminResourceGrantChange> {
        Ok(AdminResourceGrantChange {
            account_id: AccountId(self.account_id),
            dimension: persisted_dimension(&self.dimension)?.into(),
            event_type: persisted_event_type(&self.event_type)?,
            reason: persisted_admin_reason(&self.reason)?,
            actor_account_id: AccountId(self.actor_account_id),
            changed_at: self.changed_at.into_utc(),
            old_value: self.old_value.get(),
            new_value: self.new_value.get(),
            expires_at: self.expires_at.map(SqlDateTime::into_utc),
        })
    }
}

pub fn persisted_dimension(value: &str) -> RepoResult<AccountResourceOverrideDimension> {
    match value {
        "monthly_compute_gcu" => Ok(AccountResourceOverrideDimension::MonthlyComputeGcu),
        "monthly_memory_gb_seconds" => Ok(AccountResourceOverrideDimension::MonthlyMemoryGbSeconds),
        "monthly_durable_storage_gb_month" => {
            Ok(AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth)
        }
        "monthly_ephemeral_storage_gb_month" => {
            Ok(AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth)
        }
        "max_disk_space_per_worker" => Ok(AccountResourceOverrideDimension::MaxDiskSpacePerWorker),
        "max_memory_per_worker" => Ok(AccountResourceOverrideDimension::MaxMemoryPerWorker),
        _ => Err(RepoError::InternalError(anyhow::anyhow!(
            "Unknown persisted resource override dimension: {value}"
        ))),
    }
}

pub fn persisted_admin_reason(value: &str) -> RepoResult<AdminResourceGrantReason> {
    match value {
        "promotional" => Ok(AdminResourceGrantReason::Promotional),
        "support" => Ok(AdminResourceGrantReason::Support),
        _ => Err(RepoError::InternalError(anyhow::anyhow!(
            "Unknown persisted admin resource grant reason: {value}"
        ))),
    }
}

pub fn event_type_str(value: AdminResourceGrantEventType) -> &'static str {
    match value {
        AdminResourceGrantEventType::OverrideGranted => "override_granted",
        AdminResourceGrantEventType::OverrideCleared => "override_cleared",
        AdminResourceGrantEventType::OverrideExpired => "override_expired",
    }
}

fn persisted_event_type(value: &str) -> RepoResult<AdminResourceGrantEventType> {
    match value {
        "override_granted" => Ok(AdminResourceGrantEventType::OverrideGranted),
        "override_cleared" => Ok(AdminResourceGrantEventType::OverrideCleared),
        "override_expired" => Ok(AdminResourceGrantEventType::OverrideExpired),
        _ => Err(RepoError::InternalError(anyhow::anyhow!(
            "Unknown persisted resource override event type: {value}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use test_r::test;

    fn timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("test timestamp is valid")
    }

    #[test]
    fn persisted_resource_override_values_round_trip() {
        let dimensions = [
            (
                AccountResourceOverrideDimension::MonthlyComputeGcu,
                AdminResourceGrantDimension::MonthlyComputeGcu,
                "monthly_compute_gcu",
            ),
            (
                AccountResourceOverrideDimension::MonthlyMemoryGbSeconds,
                AdminResourceGrantDimension::MonthlyMemoryGbSeconds,
                "monthly_memory_gb_seconds",
            ),
            (
                AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth,
                AdminResourceGrantDimension::MonthlyDurableStorageGbMonth,
                "monthly_durable_storage_gb_month",
            ),
            (
                AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth,
                AdminResourceGrantDimension::MonthlyEphemeralStorageGbMonth,
                "monthly_ephemeral_storage_gb_month",
            ),
            (
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                AdminResourceGrantDimension::MaxStoragePerAgent,
                "max_disk_space_per_worker",
            ),
            (
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                AdminResourceGrantDimension::MaxMemoryPerAgent,
                "max_memory_per_worker",
            ),
        ];

        for (internal, public, persisted) in dimensions {
            assert_eq!(internal.as_str(), persisted);
            assert_eq!(AccountResourceOverrideDimension::from(public), internal);
            assert_eq!(AdminResourceGrantDimension::from(internal), public);
            assert_eq!(persisted_dimension(persisted).unwrap(), internal);
        }
        assert!(persisted_dimension("unknown").is_err());

        assert_eq!(
            AccountResourceOverrideSource::SelfService.as_str(),
            "self_service"
        );
        assert_eq!(
            AccountResourceOverrideSource::AdminGrant.as_str(),
            "admin_grant"
        );

        assert_eq!(
            AccountResourceOverrideReason::UserSelfServe.as_str(),
            "user_self_serve"
        );
        assert_eq!(
            AccountResourceOverrideReason::DowngradeClamp.as_str(),
            "downgrade_clamp"
        );
        assert_eq!(
            AccountResourceOverrideReason::Promotional.as_str(),
            "promotional"
        );
        assert_eq!(AccountResourceOverrideReason::Support.as_str(), "support");
        assert_eq!(
            AccountResourceOverrideReason::from(AdminResourceGrantReason::Promotional),
            AccountResourceOverrideReason::Promotional
        );
        assert_eq!(
            AccountResourceOverrideReason::from(AdminResourceGrantReason::Support),
            AccountResourceOverrideReason::Support
        );
        assert_eq!(
            persisted_admin_reason("promotional").unwrap(),
            AdminResourceGrantReason::Promotional
        );
        assert_eq!(
            persisted_admin_reason("support").unwrap(),
            AdminResourceGrantReason::Support
        );
        assert!(persisted_admin_reason("unknown").is_err());

        for (event_type, persisted) in [
            (
                AdminResourceGrantEventType::OverrideGranted,
                "override_granted",
            ),
            (
                AdminResourceGrantEventType::OverrideCleared,
                "override_cleared",
            ),
            (
                AdminResourceGrantEventType::OverrideExpired,
                "override_expired",
            ),
        ] {
            assert_eq!(event_type_str(event_type), persisted);
            assert_eq!(persisted_event_type(persisted).unwrap(), event_type);
        }
        assert!(persisted_event_type("unknown").is_err());
    }

    #[test]
    fn database_records_convert_to_public_grant_models() {
        let account_id = Uuid::from_u128(1);
        let actor_account_id = Uuid::from_u128(2);
        let granted_at = timestamp(1_700_000_000);
        let expires_at = timestamp(1_700_003_600);

        let grant = AccountResourceOverrideDbRecord {
            account_id,
            dimension: "max_memory_per_worker".to_string(),
            override_value: NumericU64::new(512),
            reason: "support".to_string(),
            expires_at: Some(SqlDateTime::new(expires_at)),
            created_by: actor_account_id,
            created_at: SqlDateTime::new(granted_at),
        }
        .into_admin_grant()
        .unwrap();

        assert_eq!(
            grant.dimension,
            AdminResourceGrantDimension::MaxMemoryPerAgent
        );
        assert_eq!(grant.value, 512);
        assert_eq!(grant.reason, AdminResourceGrantReason::Support);
        assert_eq!(grant.actor_account_id, AccountId(actor_account_id));
        assert_eq!(grant.granted_at, granted_at);
        assert_eq!(grant.expires_at, Some(expires_at));

        let change = AccountResourceOverrideEventRecord {
            account_id,
            dimension: "monthly_compute_gcu".to_string(),
            event_type: "override_granted".to_string(),
            reason: "promotional".to_string(),
            actor_account_id,
            changed_at: SqlDateTime::new(granted_at),
            old_value: NumericU64::new(10),
            new_value: NumericU64::new(20),
            expires_at: Some(SqlDateTime::new(expires_at)),
        }
        .into_public()
        .unwrap();

        assert_eq!(change.account_id, AccountId(account_id));
        assert_eq!(
            change.dimension,
            AdminResourceGrantDimension::MonthlyComputeGcu
        );
        assert_eq!(
            change.event_type,
            AdminResourceGrantEventType::OverrideGranted
        );
        assert_eq!(change.reason, AdminResourceGrantReason::Promotional);
        assert_eq!(change.actor_account_id, AccountId(actor_account_id));
        assert_eq!(change.changed_at, granted_at);
        assert_eq!(change.old_value, 10);
        assert_eq!(change.new_value, 20);
        assert_eq!(change.expires_at, Some(expires_at));
    }
}
