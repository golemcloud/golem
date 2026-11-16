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

use golem_common::error_forwarding;
use golem_common::model::plan_dimension::{
    DeclaredBounds, DeclaredMax, DimensionBoundsError, DimensionDeclaration, OperatingValue,
    PlanDimension,
};
use golem_service_base::repo::{NumericU64, RepoError, SqlDateTime};
use sqlx::FromRow;
use std::collections::BTreeMap;
use uuid::Uuid;

/// The typed declarations of one plan revision. A dimension without an entry is not declared.
pub type PlanPolicy = BTreeMap<PlanDimension, DimensionDeclaration>;

#[derive(Debug, thiserror::Error)]
pub enum PlanStorageRepoError {
    #[error("The plan definition does not exist")]
    PlanNotFound,
    #[error("The account does not exist")]
    AccountNotFound,
    #[error("The owner already has a selectable definition with this name")]
    NameConflict,
    #[error("Only the account's parent can assign it a plan, from its own definitions")]
    ForeignCatalog,
    #[error("The root account has no plan assignment")]
    RootAssignment,
    #[error("The plan definition is not selectable")]
    NotSelectable,
    #[error("The plan definition is assigned and cannot be removed")]
    Assigned,
    #[error("Dimension {dimension:?} of the definition is invalid: {error}")]
    InvalidDeclaration {
        dimension: PlanDimension,
        error: DimensionBoundsError,
    },
    #[error("Dimension {dimension:?} of the definition has an invalid default: {error}")]
    InvalidDefault {
        dimension: PlanDimension,
        error: DimensionBoundsError,
    },
    #[error(
        "Dimension {dimension:?} of the definition has an unlimited default above its finite maximum"
    )]
    UnlimitedDefaultAboveMaximum { dimension: PlanDimension },
    #[error("The change leaves account {account_id} without a valid {dimension:?} value: {error}")]
    Incompatible {
        account_id: Uuid,
        dimension: PlanDimension,
        error: DimensionBoundsError,
    },
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("The write could not acquire its guards or a stable member set; retry later")]
    Contention,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(PlanStorageRepoError, RepoError);

/// The stable storage code of a dimension.
pub fn dimension_code(dimension: PlanDimension) -> i16 {
    use PlanDimension as D;
    match dimension {
        D::ApplicationCount => 0,
        D::EnvironmentCount => 1,
        D::ComponentCount => 2,
        D::AgentConnectionCount => 3,
        D::TotalAgentCount => 4,
        D::RetainedComponentBytes => 5,
        D::RetainedBlobBytes => 6,
        D::MonthlyUploadBytes => 7,
        D::MonthlyComputeGcu => 8,
        D::MonthlyMemoryGbSeconds => 9,
        D::MonthlyDurableStorageGbMonth => 10,
        D::MonthlyEphemeralStorageGbMonth => 11,
        D::MonthlyHttpCalls => 12,
        D::MonthlyRpcCalls => 13,
        D::PerInvocationHttpCalls => 14,
        D::PerInvocationRpcCalls => 15,
        D::MaxMemoryPerAgent => 16,
        D::MaxStoragePerAgent => 17,
        D::MaxTableElementsPerAgent => 18,
        D::MaxConcurrentAgentsPerExecutor => 19,
        D::OplogWritesPerSecond => 20,
    }
}

pub fn dimension_from_code(code: i16) -> anyhow::Result<PlanDimension> {
    PlanDimension::ALL
        .into_iter()
        .find(|dimension| dimension_code(*dimension) == code)
        .ok_or_else(|| anyhow::anyhow!("Unknown plan dimension code: {code}"))
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanDefinitionRecord {
    pub plan_id: Uuid,
    pub owner_account_id: Uuid,
    pub name: String,
    pub selectable: bool,
    pub current_revision: i64,
    pub created_at: SqlDateTime,
    pub updated_at: SqlDateTime,
    pub modified_by: Uuid,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanBoundRecord {
    pub dimension: i16,
    pub min_value: Option<NumericU64>,
    pub max_value: Option<NumericU64>,
    pub max_unlimited: bool,
    pub default_value: Option<NumericU64>,
    pub default_unlimited: bool,
    pub user_configurable: Option<bool>,
}

fn declared_max(value: Option<NumericU64>, unlimited: bool) -> Option<DeclaredMax> {
    if unlimited {
        Some(DeclaredMax::Unlimited)
    } else {
        value.map(|value| DeclaredMax::Finite(value.get()))
    }
}

fn split_max(value: Option<DeclaredMax>) -> (Option<NumericU64>, bool) {
    match value {
        None => (None, false),
        Some(DeclaredMax::Unlimited) => (None, true),
        Some(DeclaredMax::Finite(value)) => (Some(value.into()), false),
    }
}

impl PlanBoundRecord {
    pub fn from_declaration(dimension: PlanDimension, declaration: &DimensionDeclaration) -> Self {
        let (max_value, max_unlimited) = split_max(declaration.bounds.max);
        let (default_value, default_unlimited) =
            split_max(declaration.operating.map(|operating| operating.default));
        Self {
            dimension: dimension_code(dimension),
            min_value: declaration.bounds.min.map(NumericU64::from),
            max_value,
            max_unlimited,
            default_value,
            default_unlimited,
            user_configurable: declaration
                .operating
                .map(|operating| operating.user_configurable),
        }
    }

    pub fn to_declaration(&self) -> anyhow::Result<(PlanDimension, DimensionDeclaration)> {
        let dimension = dimension_from_code(self.dimension)?;
        let operating = match (
            declared_max(self.default_value, self.default_unlimited),
            self.user_configurable,
        ) {
            (Some(default), Some(user_configurable)) => Some(OperatingValue {
                default,
                user_configurable,
            }),
            (None, None) => None,
            _ => {
                return Err(anyhow::anyhow!(
                    "Stored operating value of {dimension:?} is incomplete"
                ));
            }
        };
        Ok((
            dimension,
            DimensionDeclaration {
                bounds: DeclaredBounds {
                    min: self.min_value.map(|value| value.get()),
                    max: declared_max(self.max_value, self.max_unlimited),
                },
                operating,
            },
        ))
    }
}

pub fn policy_from_bounds(records: &[PlanBoundRecord]) -> anyhow::Result<PlanPolicy> {
    records
        .iter()
        .map(PlanBoundRecord::to_declaration)
        .collect()
}

/// A plan definition at its current revision.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanDefinition {
    pub record: PlanDefinitionRecord,
    pub policy: PlanPolicy,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanAssignmentRecord {
    pub account_id: Uuid,
    pub parent_id: Uuid,
    pub plan_id: Uuid,
    pub revision: i64,
    pub updated_at: SqlDateTime,
    pub modified_by: Uuid,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanAssignmentRevisionRecord {
    pub account_id: Uuid,
    pub revision: i64,
    pub plan_id: Uuid,
    pub plan_revision: i64,
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
}
