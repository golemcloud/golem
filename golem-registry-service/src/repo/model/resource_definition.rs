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

use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use anyhow::anyhow;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{
    EnforcementAction, ResourceCapacityLimit, ResourceConcurrencyLimit, ResourceDefinition,
    ResourceDefinitionId, ResourceDefinitionRevision, ResourceLimit, ResourceName,
    ResourceRateLimit, TimePeriod,
};
use golem_service_base::repo::NumericU64;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ResourceDefinitionRepoError {
    #[error("Resource definition violates unique index")]
    ResourceDefinitionViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(ResourceDefinitionRepoError, RepoError);

#[derive(Debug, Clone)]
pub struct ResourceDefinitionCreationArgs {
    pub environment_id: Uuid,
    pub limit_type: LimitTypeEnum,
    pub name: String,

    pub revision: ResourceDefinitionRevisionRecord,
}

impl ResourceDefinitionCreationArgs {
    pub fn new(
        resource_definition_id: ResourceDefinitionId,
        environment_id: EnvironmentId,
        limit: &ResourceLimit,
        name: ResourceName,
        enforcement_action: EnforcementAction,
        unit: String,
        units: String,
        actor: AccountId,
    ) -> Result<Self, ResourceDefinitionRepoError> {
        Ok(Self {
            environment_id: environment_id.0,
            limit_type: LimitTypeEnum::for_resource_limit(limit),
            name: name.0,
            revision: ResourceDefinitionRevisionRecord {
                resource_definition_id: resource_definition_id.0,
                revision_id: ResourceDefinitionRevision::INITIAL.into(),
                hash: SqlBlake3Hash::empty(),
                audit: DeletableRevisionAuditFields::new(actor.0),
                limit: limit.into(),
                enforcement_action: enforcement_action.into(),
                unit,
                units,
            }
            .with_updated_hash()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ResourceDefinitionRecord {
    pub resource_definition_id: Uuid,
    pub environment_id: Uuid,
    pub limit_type: LimitTypeEnum,
    pub name: String,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ResourceDefinitionRevisionRecord {
    pub resource_definition_id: Uuid,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,

    #[sqlx(flatten)]
    pub limit: ResourceLimitRecord,

    pub enforcement_action: EnforcementActionEnum,

    pub unit: String,
    pub units: String,
}

impl ResourceDefinitionRevisionRecord {
    pub fn from_model(
        value: ResourceDefinition,
        audit: DeletableRevisionAuditFields,
    ) -> Result<Self, ResourceDefinitionRepoError> {
        let mut value = Self {
            resource_definition_id: value.id.0,
            revision_id: value.revision.into(),
            hash: SqlBlake3Hash::empty(),
            audit,
            limit: (&value.limit).into(),
            enforcement_action: value.enforcement_action.into(),
            unit: value.unit,
            units: value.units,
        };
        value.update_hash()?;
        Ok(value)
    }

    pub fn to_diffable(&self) -> diff::ResourceDefinition {
        diff::ResourceDefinition {
            limit: self.limit.to_diffable(),
            enforcement_action: self.enforcement_action.into(),
            unit: self.unit.clone(),
            units: self.units.clone(),
        }
    }

    pub fn update_hash(&mut self) -> Result<(), ResourceDefinitionRepoError> {
        self.hash = self
            .to_diffable()
            .hash()
            .map_err(|err| ResourceDefinitionRepoError::InternalError(anyhow!(err)))?
            .into_blake3()
            .into();
        Ok(())
    }

    pub fn with_updated_hash(mut self) -> Result<Self, ResourceDefinitionRepoError> {
        self.update_hash()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ResourceDefinitionExtRevisionRecord {
    pub environment_id: Uuid,
    pub limit_type: LimitTypeEnum,
    pub name: String,

    #[sqlx(flatten)]
    pub revision: ResourceDefinitionRevisionRecord,
}

impl TryFrom<ResourceDefinitionExtRevisionRecord> for ResourceDefinition {
    type Error = ResourceDefinitionRepoError;
    fn try_from(value: ResourceDefinitionExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: ResourceDefinitionId(value.revision.resource_definition_id),
            revision: value.revision.revision_id.try_into()?,
            environment_id: EnvironmentId(value.environment_id),
            name: ResourceName(value.name),
            limit: value.revision.limit.into_domain(value.limit_type)?,
            enforcement_action: value.revision.enforcement_action.into(),
            unit: value.revision.unit,
            units: value.revision.units,
        })
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::Type)]
#[sqlx(type_name = "text")]
#[sqlx(rename_all = "lowercase")]
pub enum TimePeriodEnum {
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

impl From<TimePeriod> for TimePeriodEnum {
    fn from(value: TimePeriod) -> Self {
        match value {
            TimePeriod::Second => Self::Second,
            TimePeriod::Minute => Self::Minute,
            TimePeriod::Hour => Self::Hour,
            TimePeriod::Day => Self::Day,
            TimePeriod::Month => Self::Month,
            TimePeriod::Year => Self::Year,
        }
    }
}

impl From<TimePeriodEnum> for TimePeriod {
    fn from(value: TimePeriodEnum) -> Self {
        match value {
            TimePeriodEnum::Second => Self::Second,
            TimePeriodEnum::Minute => Self::Minute,
            TimePeriodEnum::Hour => Self::Hour,
            TimePeriodEnum::Day => Self::Day,
            TimePeriodEnum::Month => Self::Month,
            TimePeriodEnum::Year => Self::Year,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, sqlx::Type)]
#[sqlx(type_name = "text")]
#[sqlx(rename_all = "lowercase")]
pub enum LimitTypeEnum {
    Capacity,
    Concurrency,
    Rate,
}

impl LimitTypeEnum {
    pub fn for_resource_limit(resource_limit: &ResourceLimit) -> Self {
        match resource_limit {
            ResourceLimit::Capacity(_) => Self::Capacity,
            ResourceLimit::Concurrency(_) => Self::Concurrency,
            ResourceLimit::Rate(_) => Self::Rate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ResourceLimitRecord {
    pub limit_value: NumericU64,
    pub limit_period: Option<TimePeriodEnum>,
    pub limit_max: Option<NumericU64>,
}

impl ResourceLimitRecord {
    pub fn to_diffable(&self) -> diff::ResourceLimit {
        diff::ResourceLimit {
            value: self.limit_value.into(),
            period: self.limit_period.clone().map(Into::into),
            max: self.limit_max.map(Into::into),
        }
    }

    fn into_domain(
        self,
        limit_type: LimitTypeEnum,
    ) -> Result<ResourceLimit, ResourceDefinitionRepoError> {
        match limit_type {
            LimitTypeEnum::Capacity => Ok(ResourceLimit::Capacity(ResourceCapacityLimit {
                value: self.limit_value.into(),
            })),
            LimitTypeEnum::Concurrency => {
                Ok(ResourceLimit::Concurrency(ResourceConcurrencyLimit {
                    value: self.limit_value.into(),
                }))
            }
            LimitTypeEnum::Rate => {
                let value: u64 = self.limit_value.into();
                let max: u64 = self.limit_max.map(Into::into).unwrap_or(value);
                Ok(ResourceLimit::Rate(ResourceRateLimit {
                    value,
                    period: self
                        .limit_period
                        .ok_or_else(|| anyhow!("missing limit period for rate limit"))?
                        .into(),
                    max,
                }))
            }
        }
    }
}

impl From<&ResourceLimit> for ResourceLimitRecord {
    fn from(value: &ResourceLimit) -> Self {
        match value {
            ResourceLimit::Capacity(inner) => ResourceLimitRecord {
                limit_value: inner.value.into(),
                limit_period: None,
                limit_max: None,
            },
            ResourceLimit::Concurrency(inner) => ResourceLimitRecord {
                limit_value: inner.value.into(),
                limit_period: None,
                limit_max: None,
            },
            ResourceLimit::Rate(inner) => ResourceLimitRecord {
                limit_value: inner.value.into(),
                limit_period: Some(inner.period.into()),
                limit_max: Some(inner.max.into()),
            },
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, sqlx::Type)]
#[sqlx(type_name = "text")]
#[sqlx(rename_all = "lowercase")]
pub enum EnforcementActionEnum {
    Reject,
    Throttle,
    Terminate,
}

impl From<EnforcementAction> for EnforcementActionEnum {
    fn from(value: EnforcementAction) -> Self {
        match value {
            EnforcementAction::Reject => Self::Reject,
            EnforcementAction::Terminate => Self::Terminate,
            EnforcementAction::Throttle => Self::Throttle,
        }
    }
}

impl From<EnforcementActionEnum> for EnforcementAction {
    fn from(value: EnforcementActionEnum) -> Self {
        match value {
            EnforcementActionEnum::Reject => Self::Reject,
            EnforcementActionEnum::Terminate => Self::Terminate,
            EnforcementActionEnum::Throttle => Self::Throttle,
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ResourceDefinitionRevisionIdentityRecord {
    pub resource_definition_id: Uuid,
    pub revision_id: i64,
    pub limit_type: LimitTypeEnum,
    pub name: String,
    pub hash: SqlBlake3Hash,
}
