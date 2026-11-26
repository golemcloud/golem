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

//! Typed plan dimensions and the compatible-range calculation across an account's ancestry.
//!
//! Each [`PlanDimension`] has a [`DimensionRule`]: what it counts or measures, its scope and
//! unit, the finite values its producer can enforce, and whether it may be unbounded. A plan
//! level declares bounds for a dimension, and [`resolve_effective_bounds`] combines the levels
//! of a chain, root first. The effective minimum is the largest declared minimum and the
//! effective maximum is the smallest declared maximum. Touching bounds permit one value; only
//! a minimum above the maximum is incompatible. Usage is not an input, so usage above a
//! reduced maximum never makes a range incompatible.
//!
//! | Dimension | Kind | Scope | Unit | Producer |
//! | --- | --- | --- | --- | --- |
//! | `ApplicationCount` | exact count | account | count | registry application creation |
//! | `EnvironmentCount` | exact count | account | count | registry environment creation |
//! | `ComponentCount` | exact count | account | count | registry component creation |
//! | `AgentConnectionCount` | approximate count | account | count | worker-service connection admission |
//! | `TotalAgentCount` | approximate count | account | count | none yet; no plan field declares it |
//! | `RetainedComponentBytes` | measured total | account | bytes | registry component writes; refuses publication only |
//! | `RetainedBlobBytes` | measured total | account | bytes | executor blob storage reports |
//! | `MonthlyUploadBytes` | measured monthly | account | bytes | declared, not yet enforced |
//! | `MonthlyComputeGcu` | monthly amount | account | GCU | executor metering and monthly caps |
//! | `MonthlyMemoryGbSeconds` | monthly amount | account | GB-seconds | executor metering and monthly caps |
//! | `MonthlyDurableStorageGbMonth` | monthly amount | account | GB-month | executor metering and monthly caps |
//! | `MonthlyEphemeralStorageGbMonth` | monthly amount | account | GB-month | executor metering and monthly caps |
//! | `MonthlyHttpCalls`, `MonthlyRpcCalls` | measured monthly | account | count | executor reports; registry HTTP policy |
//! | `PerInvocationHttpCalls`, `PerInvocationRpcCalls` | invocation budget | invocation | count | executor call counters |
//! | `MaxMemoryPerAgent` | operating value | agent | bytes | executor memory limit |
//! | `MaxStoragePerAgent` | operating value | agent | bytes | executor managed filesystem quota |
//! | `MaxTableElementsPerAgent` | operating value | agent | count | executor table limit |
//! | `MaxConcurrentAgentsPerExecutor` | operating value | account per executor | count | executor concurrency semaphore |
//! | `OplogWritesPerSecond` | rate | agent | writes per second | executor oplog rate limiter |
//!
//! Only the four monthly amounts have spend-mode semantics. Monthly calls and per-executor
//! concurrency are not monthly allowances, and per-executor concurrency is not a total agent
//! count. Overage eligibility is a local plan entitlement, not a dimension.
//!
//! Metering is not part of the calculation. [`DimensionRule::metering`] names the operator
//! metering switch that governs a monthly amount; an unmetered dimension has no amount and no
//! cap, which the monthly policy applies outside this module.
//!
//! Per-agent storage is enforced only where the installation provides managed filesystem
//! quotas. That availability is an input to the calculation, never a plan level, so a chain
//! never mixes a disabled level with numeric bounds.
//!
//! Bounds that other producers add later have no dimension here yet: the detached shell idle
//! lifetime and open shell sessions per agent (Shell), retention bounds (Retention), the
//! `allow_chosen_hosts` entitlement (host serving), and open webhooks and promises per owner
//! and queued webhook requests (their owning producers).

use crate::base_model::account_usage::{
    BYTE_SECONDS_PER_GB_MONTH, EFFECTIVELY_UNLIMITED_STORAGE_LIMIT, FUEL_PER_GCU,
    ResourceLimitValue, StorageLimitDisabledReason, StorageResourceLimitValue,
};
use crate::base_model::plan::Plan;

/// The value at and above which the executor treats per-executor concurrency and the oplog
/// write rate as unlimited.
const RUNTIME_UNLIMITED_SENTINEL: u64 = 1_000_000_000_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlanDimension {
    ApplicationCount,
    EnvironmentCount,
    ComponentCount,
    AgentConnectionCount,
    TotalAgentCount,
    RetainedComponentBytes,
    RetainedBlobBytes,
    MonthlyUploadBytes,
    MonthlyComputeGcu,
    MonthlyMemoryGbSeconds,
    MonthlyDurableStorageGbMonth,
    MonthlyEphemeralStorageGbMonth,
    MonthlyHttpCalls,
    MonthlyRpcCalls,
    PerInvocationHttpCalls,
    PerInvocationRpcCalls,
    MaxMemoryPerAgent,
    MaxStoragePerAgent,
    MaxTableElementsPerAgent,
    MaxConcurrentAgentsPerExecutor,
    OplogWritesPerSecond,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionKind {
    /// Created resources counted in the creating transaction.
    ExactCount,
    /// Runtime resources counted from asynchronous reports.
    ApproximateCount,
    /// Usage that accumulates without a monthly reset.
    MeasuredTotal,
    /// Usage that resets each UTC month, without spend-mode semantics.
    MeasuredMonthly,
    /// A monthly amount that is a hard cap or an included allowance, by spend mode.
    MonthlyAmount,
    /// A budget that each invocation uses separately.
    InvocationBudget,
    /// A runtime bound applied to each agent or executor.
    OperatingValue,
    Rate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionScope {
    Account,
    AccountPerExecutor,
    Agent,
    Invocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionUnit {
    Count,
    Bytes,
    Gcu,
    GbSeconds,
    GbMonth,
    WritesPerSecond,
}

/// The operator metering switch that governs a monthly amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeteringFlag {
    Compute,
    Memory,
    Filesystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DimensionRule {
    pub kind: DimensionKind,
    pub scope: DimensionScope,
    pub unit: DimensionUnit,
    /// The smallest finite value a level may declare.
    pub finite_min: u64,
    /// The largest finite value a level may declare.
    pub finite_max: u64,
    /// Whether a level may declare no maximum, and whether a chain that declares no maximum
    /// is unbounded rather than incomplete.
    pub unbounded: bool,
    /// Whether enforcement depends on installation availability.
    pub availability_dependent: bool,
    pub metering: Option<MeteringFlag>,
}

impl PlanDimension {
    pub const ALL: [PlanDimension; 21] = [
        Self::ApplicationCount,
        Self::EnvironmentCount,
        Self::ComponentCount,
        Self::AgentConnectionCount,
        Self::TotalAgentCount,
        Self::RetainedComponentBytes,
        Self::RetainedBlobBytes,
        Self::MonthlyUploadBytes,
        Self::MonthlyComputeGcu,
        Self::MonthlyMemoryGbSeconds,
        Self::MonthlyDurableStorageGbMonth,
        Self::MonthlyEphemeralStorageGbMonth,
        Self::MonthlyHttpCalls,
        Self::MonthlyRpcCalls,
        Self::PerInvocationHttpCalls,
        Self::PerInvocationRpcCalls,
        Self::MaxMemoryPerAgent,
        Self::MaxStoragePerAgent,
        Self::MaxTableElementsPerAgent,
        Self::MaxConcurrentAgentsPerExecutor,
        Self::OplogWritesPerSecond,
    ];

    pub const fn rule(self) -> DimensionRule {
        use DimensionKind as K;
        use DimensionScope as S;
        use DimensionUnit as U;

        const fn rule(
            kind: DimensionKind,
            scope: DimensionScope,
            unit: DimensionUnit,
        ) -> DimensionRule {
            DimensionRule {
                kind,
                scope,
                unit,
                finite_min: 0,
                finite_max: u64::MAX,
                unbounded: false,
                availability_dependent: false,
                metering: None,
            }
        }

        const fn monthly(
            unit: DimensionUnit,
            finite_max: u64,
            metering: MeteringFlag,
        ) -> DimensionRule {
            DimensionRule {
                finite_max,
                metering: Some(metering),
                ..rule(K::MonthlyAmount, S::Account, unit)
            }
        }

        const fn runtime_sentinel(rule: DimensionRule) -> DimensionRule {
            DimensionRule {
                finite_min: 1,
                finite_max: RUNTIME_UNLIMITED_SENTINEL - 1,
                unbounded: true,
                ..rule
            }
        }

        match self {
            Self::ApplicationCount | Self::EnvironmentCount | Self::ComponentCount => {
                rule(K::ExactCount, S::Account, U::Count)
            }
            Self::AgentConnectionCount => rule(K::ApproximateCount, S::Account, U::Count),
            Self::TotalAgentCount => DimensionRule {
                unbounded: true,
                ..rule(K::ApproximateCount, S::Account, U::Count)
            },
            Self::RetainedComponentBytes | Self::RetainedBlobBytes => {
                rule(K::MeasuredTotal, S::Account, U::Bytes)
            }
            Self::MonthlyUploadBytes => rule(K::MeasuredMonthly, S::Account, U::Bytes),
            Self::MonthlyComputeGcu => {
                monthly(U::Gcu, u64::MAX / FUEL_PER_GCU, MeteringFlag::Compute)
            }
            Self::MonthlyMemoryGbSeconds => monthly(U::GbSeconds, u64::MAX, MeteringFlag::Memory),
            Self::MonthlyDurableStorageGbMonth | Self::MonthlyEphemeralStorageGbMonth => monthly(
                U::GbMonth,
                u64::MAX / BYTE_SECONDS_PER_GB_MONTH,
                MeteringFlag::Filesystem,
            ),
            Self::MonthlyHttpCalls | Self::MonthlyRpcCalls => {
                rule(K::MeasuredMonthly, S::Account, U::Count)
            }
            Self::PerInvocationHttpCalls | Self::PerInvocationRpcCalls => {
                rule(K::InvocationBudget, S::Invocation, U::Count)
            }
            Self::MaxMemoryPerAgent => DimensionRule {
                unbounded: true,
                ..rule(K::OperatingValue, S::Agent, U::Bytes)
            },
            Self::MaxStoragePerAgent => DimensionRule {
                finite_min: 1,
                finite_max: EFFECTIVELY_UNLIMITED_STORAGE_LIMIT - 1,
                unbounded: true,
                availability_dependent: true,
                ..rule(K::OperatingValue, S::Agent, U::Bytes)
            },
            Self::MaxTableElementsPerAgent => rule(K::OperatingValue, S::Agent, U::Count),
            Self::MaxConcurrentAgentsPerExecutor => {
                runtime_sentinel(rule(K::OperatingValue, S::AccountPerExecutor, U::Count))
            }
            Self::OplogWritesPerSecond => {
                runtime_sentinel(rule(K::Rate, S::Agent, U::WritesPerSecond))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredMax {
    Finite(u64),
    Unlimited,
}

impl DeclaredMax {
    fn is_stricter_than(self, other: DeclaredMax) -> bool {
        match (self, other) {
            (DeclaredMax::Finite(a), DeclaredMax::Finite(b)) => a < b,
            (DeclaredMax::Finite(_), DeclaredMax::Unlimited) => true,
            (DeclaredMax::Unlimited, _) => false,
        }
    }
}

impl From<ResourceLimitValue> for DeclaredMax {
    fn from(value: ResourceLimitValue) -> Self {
        match value {
            ResourceLimitValue::Finite(value) => DeclaredMax::Finite(value.value),
            ResourceLimitValue::Unlimited(_) => DeclaredMax::Unlimited,
        }
    }
}

/// The bounds one plan level declares for a dimension. `None` means the level is silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeclaredBounds {
    pub min: Option<u64>,
    pub max: Option<DeclaredMax>,
}

/// The value a dimension operates at within its bounds, unless an override selects another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatingValue {
    pub default: DeclaredMax,
    pub user_configurable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DimensionDeclaration {
    pub bounds: DeclaredBounds,
    pub operating: Option<OperatingValue>,
}

/// Whether the installation can enforce a dimension whose rule is availability-dependent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationAvailability {
    Available,
    Unavailable(StorageLimitDisabledReason),
}

/// Where an effective endpoint comes from: the dimension's domain, or a level of the chain,
/// counted from the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSource {
    Domain,
    Level(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint<T> {
    pub value: T,
    pub source: EndpointSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveRange {
    pub min: Endpoint<u64>,
    pub max: Endpoint<DeclaredMax>,
}

impl EffectiveRange {
    pub fn contains(&self, value: u64) -> bool {
        value >= self.min.value
            && match self.max.value {
                DeclaredMax::Finite(max) => value <= max,
                DeclaredMax::Unlimited => true,
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveBounds {
    Range(EffectiveRange),
    /// The installation cannot enforce the dimension.
    Disabled(StorageLimitDisabledReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DimensionBoundsError {
    #[error("the effective minimum is above the effective maximum")]
    Incompatible {
        min: Endpoint<u64>,
        max: Endpoint<DeclaredMax>,
    },
    #[error("level {level} declares {value}, which is outside the dimension's finite values")]
    OutsideDomain { level: usize, value: u64 },
    #[error("level {level} declares no maximum, which the dimension does not allow")]
    UnboundedNotAllowed { level: usize },
    #[error("no level declares a maximum, and the dimension requires one")]
    MissingMaximum,
    #[error("installation availability does not apply to this dimension")]
    AvailabilityNotApplicable,
}

/// Combines the bounds of a chain of plan levels, root first, for one dimension.
pub fn resolve_effective_bounds(
    dimension: PlanDimension,
    availability: InstallationAvailability,
    chain: &[Option<DeclaredBounds>],
) -> Result<EffectiveBounds, DimensionBoundsError> {
    let rule = dimension.rule();
    if let InstallationAvailability::Unavailable(reason) = availability {
        return if rule.availability_dependent {
            Ok(EffectiveBounds::Disabled(reason))
        } else {
            Err(DimensionBoundsError::AvailabilityNotApplicable)
        };
    }

    let in_domain = |level: usize, value: u64| {
        if (rule.finite_min..=rule.finite_max).contains(&value) {
            Ok(())
        } else {
            Err(DimensionBoundsError::OutsideDomain { level, value })
        }
    };

    let mut min = Endpoint {
        value: rule.finite_min,
        source: EndpointSource::Domain,
    };
    let mut max: Option<Endpoint<DeclaredMax>> = None;

    for (level, bounds) in chain.iter().enumerate() {
        let Some(bounds) = bounds else { continue };
        let source = EndpointSource::Level(level);
        if let Some(value) = bounds.min {
            in_domain(level, value)?;
            if value > min.value {
                min = Endpoint { value, source };
            }
        }
        if let Some(value) = bounds.max {
            match value {
                DeclaredMax::Finite(value) => in_domain(level, value)?,
                DeclaredMax::Unlimited if !rule.unbounded => {
                    return Err(DimensionBoundsError::UnboundedNotAllowed { level });
                }
                DeclaredMax::Unlimited => {}
            }
            if max.is_none_or(|current| value.is_stricter_than(current.value)) {
                max = Some(Endpoint { value, source });
            }
        }
    }

    let max = match max {
        Some(max) => max,
        None if rule.unbounded => Endpoint {
            value: DeclaredMax::Unlimited,
            source: EndpointSource::Domain,
        },
        None => return Err(DimensionBoundsError::MissingMaximum),
    };

    if let DeclaredMax::Finite(max_value) = max.value
        && min.value > max_value
    {
        return Err(DimensionBoundsError::Incompatible { min, max });
    }

    Ok(EffectiveBounds::Range(EffectiveRange { min, max }))
}

fn finite_max(value: u64) -> Option<DimensionDeclaration> {
    Some(DimensionDeclaration {
        bounds: DeclaredBounds {
            min: None,
            max: Some(DeclaredMax::Finite(value)),
        },
        operating: None,
    })
}

fn runtime_sentinel_max(value: u64) -> Option<DimensionDeclaration> {
    let max = if value >= RUNTIME_UNLIMITED_SENTINEL {
        DeclaredMax::Unlimited
    } else {
        DeclaredMax::Finite(value)
    };
    Some(DimensionDeclaration {
        bounds: DeclaredBounds {
            min: None,
            max: Some(max),
        },
        operating: None,
    })
}

impl Plan {
    /// Whether this plan's installation provides the managed filesystem quotas that per-agent
    /// storage limits need.
    pub fn storage_availability(&self) -> InstallationAvailability {
        match self.max_storage_per_agent_ceiling {
            StorageResourceLimitValue::Disabled(disabled) => {
                InstallationAvailability::Unavailable(disabled.reason)
            }
            StorageResourceLimitValue::Finite(_) | StorageResourceLimitValue::Unlimited(_) => {
                InstallationAvailability::Available
            }
        }
    }

    /// The bounds and operating value this plan declares for a dimension, or `None` when it
    /// declares none.
    pub fn dimension_declaration(&self, dimension: PlanDimension) -> Option<DimensionDeclaration> {
        match dimension {
            PlanDimension::ApplicationCount => finite_max(self.app_limit),
            PlanDimension::EnvironmentCount => finite_max(self.env_limit),
            PlanDimension::ComponentCount => finite_max(self.component_limit),
            PlanDimension::AgentConnectionCount => finite_max(self.worker_connection_limit),
            PlanDimension::TotalAgentCount => None,
            PlanDimension::RetainedComponentBytes => finite_max(self.storage_limit),
            PlanDimension::RetainedBlobBytes => finite_max(self.blob_storage_limit),
            PlanDimension::MonthlyUploadBytes => finite_max(self.monthly_upload_limit),
            PlanDimension::MonthlyComputeGcu => finite_max(self.monthly_compute_gcu),
            PlanDimension::MonthlyMemoryGbSeconds => finite_max(self.monthly_memory_gb_seconds),
            PlanDimension::MonthlyDurableStorageGbMonth => {
                finite_max(self.monthly_durable_storage_gb_month)
            }
            PlanDimension::MonthlyEphemeralStorageGbMonth => {
                finite_max(self.monthly_ephemeral_storage_gb_month)
            }
            PlanDimension::MonthlyHttpCalls => finite_max(self.monthly_http_call_limit),
            PlanDimension::MonthlyRpcCalls => finite_max(self.monthly_rpc_call_limit),
            PlanDimension::PerInvocationHttpCalls => {
                finite_max(self.per_invocation_http_call_limit)
            }
            PlanDimension::PerInvocationRpcCalls => finite_max(self.per_invocation_rpc_call_limit),
            PlanDimension::MaxMemoryPerAgent => Some(DimensionDeclaration {
                bounds: DeclaredBounds {
                    min: None,
                    max: Some(self.max_memory_per_agent_ceiling.into()),
                },
                operating: Some(OperatingValue {
                    default: self.max_memory_per_agent.into(),
                    user_configurable: self.max_memory_per_agent_user_configurable,
                }),
            }),
            PlanDimension::MaxStoragePerAgent => {
                let ceiling = storage_declared_max(self.max_storage_per_agent_ceiling)?;
                let default = storage_declared_max(self.max_storage_per_agent)?;
                Some(DimensionDeclaration {
                    bounds: DeclaredBounds {
                        min: None,
                        max: Some(ceiling),
                    },
                    operating: Some(OperatingValue {
                        default,
                        user_configurable: self.max_storage_per_agent_user_configurable,
                    }),
                })
            }
            PlanDimension::MaxTableElementsPerAgent => {
                finite_max(self.max_table_elements_per_worker)
            }
            PlanDimension::MaxConcurrentAgentsPerExecutor => {
                runtime_sentinel_max(self.max_concurrent_agents_per_executor)
            }
            PlanDimension::OplogWritesPerSecond => {
                runtime_sentinel_max(self.oplog_writes_per_second)
            }
        }
    }
}

/// Disabled storage is installation availability, reported by [`Plan::storage_availability`],
/// so it declares no bound.
fn storage_declared_max(value: StorageResourceLimitValue) -> Option<DeclaredMax> {
    match value {
        StorageResourceLimitValue::Finite(value) => Some(DeclaredMax::Finite(value.value)),
        StorageResourceLimitValue::Unlimited(_) => Some(DeclaredMax::Unlimited),
        StorageResourceLimitValue::Disabled(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeclaredBounds, DeclaredMax, DimensionBoundsError, EffectiveBounds, EffectiveRange,
        Endpoint, EndpointSource, InstallationAvailability, PlanDimension,
        resolve_effective_bounds,
    };
    use crate::base_model::account_usage::{
        FiniteResourceLimit, StorageLimitDisabledReason, StorageResourceLimitValue,
    };
    use crate::base_model::plan::Plan;
    use poem_openapi::types::Example;
    use test_r::test;

    const AVAILABLE: InstallationAvailability = InstallationAvailability::Available;
    const NO_QUOTAS: StorageLimitDisabledReason =
        StorageLimitDisabledReason::ManagedFilesystemUnavailable;

    fn range(min: u64, max: u64) -> Option<DeclaredBounds> {
        Some(DeclaredBounds {
            min: Some(min),
            max: Some(DeclaredMax::Finite(max)),
        })
    }

    fn max(value: DeclaredMax) -> Option<DeclaredBounds> {
        Some(DeclaredBounds {
            min: None,
            max: Some(value),
        })
    }

    fn finite(value: u64) -> Option<DeclaredBounds> {
        max(DeclaredMax::Finite(value))
    }

    fn resolve(
        dimension: PlanDimension,
        chain: &[Option<DeclaredBounds>],
    ) -> Result<EffectiveRange, DimensionBoundsError> {
        match resolve_effective_bounds(dimension, AVAILABLE, chain)? {
            EffectiveBounds::Range(range) => Ok(range),
            EffectiveBounds::Disabled(_) => panic!("unexpected disabled result"),
        }
    }

    fn at(value: u64, level: usize) -> Endpoint<u64> {
        Endpoint {
            value,
            source: EndpointSource::Level(level),
        }
    }

    fn max_at(value: u64, level: usize) -> Endpoint<DeclaredMax> {
        Endpoint {
            value: DeclaredMax::Finite(value),
            source: EndpointSource::Level(level),
        }
    }

    #[test]
    fn overlapping_and_touching_ranges_are_compatible() {
        let d = PlanDimension::MaxTableElementsPerAgent;
        assert_eq!(
            resolve(d, &[range(10, 30), range(1, 20)]),
            Ok(EffectiveRange {
                min: at(10, 0),
                max: max_at(20, 1)
            })
        );
        assert_eq!(
            resolve(d, &[range(10, 30), range(1, 10)]),
            Ok(EffectiveRange {
                min: at(10, 0),
                max: max_at(10, 1)
            })
        );
    }

    #[test]
    fn disjoint_ranges_are_incompatible_in_both_directions() {
        let d = PlanDimension::MaxTableElementsPerAgent;
        assert_eq!(
            resolve(d, &[range(10, 30), range(1, 7)]),
            Err(DimensionBoundsError::Incompatible {
                min: at(10, 0),
                max: max_at(7, 1)
            })
        );
        assert_eq!(
            resolve(d, &[range(1, 7), range(10, 30)]),
            Err(DimensionBoundsError::Incompatible {
                min: at(10, 1),
                max: max_at(7, 0)
            })
        );
    }

    #[test]
    fn every_ancestor_restricts_the_result() {
        let result = resolve(
            PlanDimension::ApplicationCount,
            &[finite(100), finite(30), finite(80), finite(60)],
        );
        assert_eq!(result.unwrap().max, max_at(30, 1));
    }

    #[test]
    fn the_outermost_level_supplies_an_equal_endpoint() {
        let result = resolve(
            PlanDimension::ApplicationCount,
            &[finite(50), finite(70), finite(50)],
        );
        assert_eq!(result.unwrap().max, max_at(50, 0));
    }

    #[test]
    fn absent_and_unbounded_keep_their_meanings() {
        assert_eq!(
            resolve(PlanDimension::TotalAgentCount, &[None, None])
                .unwrap()
                .max,
            Endpoint {
                value: DeclaredMax::Unlimited,
                source: EndpointSource::Domain
            }
        );
        assert_eq!(
            resolve(
                PlanDimension::TotalAgentCount,
                &[None, max(DeclaredMax::Unlimited), None]
            )
            .unwrap()
            .max,
            Endpoint {
                value: DeclaredMax::Unlimited,
                source: EndpointSource::Level(1)
            }
        );
        assert_eq!(
            resolve(PlanDimension::TotalAgentCount, &[None, finite(5)])
                .unwrap()
                .max,
            max_at(5, 1)
        );

        assert_eq!(
            resolve(PlanDimension::MonthlyComputeGcu, &[None, None]),
            Err(DimensionBoundsError::MissingMaximum)
        );
        assert_eq!(
            resolve(
                PlanDimension::MonthlyComputeGcu,
                &[finite(10), max(DeclaredMax::Unlimited)]
            ),
            Err(DimensionBoundsError::UnboundedNotAllowed { level: 1 })
        );
    }

    #[test]
    fn zero_is_a_finite_bound_where_the_domain_allows_it() {
        let apps = resolve(PlanDimension::ApplicationCount, &[finite(0)]).unwrap();
        assert_eq!(apps.max.value, DeclaredMax::Finite(0));
        assert!(apps.contains(0));
        assert!(!apps.contains(1));

        let compute = resolve(PlanDimension::MonthlyComputeGcu, &[finite(0)]).unwrap();
        assert_eq!(compute.max.value, DeclaredMax::Finite(0));

        for dimension in [
            PlanDimension::MaxConcurrentAgentsPerExecutor,
            PlanDimension::OplogWritesPerSecond,
        ] {
            assert_eq!(
                resolve(dimension, &[finite(0)]),
                Err(DimensionBoundsError::OutsideDomain { level: 0, value: 0 })
            );
            assert!(resolve(dimension, &[finite(1)]).is_ok());
        }
    }

    #[test]
    fn finite_values_are_limited_to_the_checked_conversions() {
        assert!(
            resolve(
                PlanDimension::MonthlyComputeGcu,
                &[finite(18_446_744_073_709)]
            )
            .is_ok()
        );
        assert_eq!(
            resolve(
                PlanDimension::MonthlyComputeGcu,
                &[finite(18_446_744_073_710)]
            ),
            Err(DimensionBoundsError::OutsideDomain {
                level: 0,
                value: 18_446_744_073_710
            })
        );
        for dimension in [
            PlanDimension::MonthlyDurableStorageGbMonth,
            PlanDimension::MonthlyEphemeralStorageGbMonth,
        ] {
            assert!(resolve(dimension, &[finite(6_537)]).is_ok());
            assert!(resolve(dimension, &[finite(6_538)]).is_err());
        }
        assert_eq!(
            resolve(
                PlanDimension::MaxTableElementsPerAgent,
                &[
                    finite(10),
                    Some(DeclaredBounds {
                        min: Some(u64::MAX),
                        max: None
                    })
                ]
            ),
            Err(DimensionBoundsError::Incompatible {
                min: at(u64::MAX, 1),
                max: max_at(10, 0)
            })
        );
    }

    #[test]
    fn usage_above_a_reduced_maximum_is_not_an_empty_range() {
        // A child keeps its declaration of 100 while its ancestor reduces the maximum to 50.
        // Its recorded usage of 80 stays above the range but does not make the range empty.
        let reduced = resolve(PlanDimension::ApplicationCount, &[finite(50), finite(100)]).unwrap();
        assert_eq!(reduced.min.value, 0);
        assert_eq!(reduced.max, max_at(50, 0));
        assert!(!reduced.contains(80));
    }

    #[test]
    fn storage_availability_is_an_input_not_a_level() {
        let chain = [finite(100), finite(40)];
        assert_eq!(
            resolve_effective_bounds(
                PlanDimension::MaxStoragePerAgent,
                InstallationAvailability::Unavailable(NO_QUOTAS),
                &chain
            ),
            Ok(EffectiveBounds::Disabled(NO_QUOTAS))
        );
        assert_eq!(
            resolve(PlanDimension::MaxStoragePerAgent, &chain)
                .unwrap()
                .max,
            max_at(40, 1)
        );
        assert_eq!(
            resolve_effective_bounds(
                PlanDimension::ApplicationCount,
                InstallationAvailability::Unavailable(NO_QUOTAS),
                &chain
            ),
            Err(DimensionBoundsError::AvailabilityNotApplicable)
        );
    }

    #[test]
    fn current_plan_maps_to_typed_declarations() {
        let plan = Plan::example();
        for dimension in PlanDimension::ALL {
            let declaration = plan.dimension_declaration(dimension);
            assert_eq!(
                declaration.is_none(),
                matches!(
                    dimension,
                    PlanDimension::TotalAgentCount | PlanDimension::MaxStoragePerAgent
                ),
                "{dimension:?}"
            );
        }
        assert_eq!(
            plan.storage_availability(),
            InstallationAvailability::Unavailable(NO_QUOTAS)
        );

        let memory = plan
            .dimension_declaration(PlanDimension::MaxMemoryPerAgent)
            .unwrap();
        assert_eq!(memory.bounds.max, Some(DeclaredMax::Unlimited));
        let operating = memory.operating.unwrap();
        assert_eq!(operating.default, DeclaredMax::Unlimited);
        assert!(!operating.user_configurable);
    }

    #[test]
    fn enabled_storage_keeps_its_typed_values_and_operating_default() {
        let mut plan = Plan::example();
        plan.max_storage_per_agent =
            StorageResourceLimitValue::Finite(FiniteResourceLimit { value: 1_000 });
        plan.max_storage_per_agent_ceiling =
            StorageResourceLimitValue::Finite(FiniteResourceLimit {
                value: 10_000_000_000_000_000,
            });
        plan.max_storage_per_agent_user_configurable = true;

        assert_eq!(
            plan.storage_availability(),
            InstallationAvailability::Available
        );
        let storage = plan
            .dimension_declaration(PlanDimension::MaxStoragePerAgent)
            .unwrap();
        assert_eq!(
            storage.bounds.max,
            Some(DeclaredMax::Finite(10_000_000_000_000_000))
        );
        let operating = storage.operating.unwrap();
        assert_eq!(operating.default, DeclaredMax::Finite(1_000));
        assert!(operating.user_configurable);
    }

    #[test]
    fn runtime_sentinels_apply_only_to_concurrency_and_the_oplog_rate() {
        let mut plan = Plan::example();
        let declared = |plan: &Plan, dimension| {
            plan.dimension_declaration(dimension)
                .unwrap()
                .bounds
                .max
                .unwrap()
        };

        for (raw, expected) in [
            (0, DeclaredMax::Finite(0)),
            (1, DeclaredMax::Finite(1)),
            (
                999_999_999_999_999_999,
                DeclaredMax::Finite(999_999_999_999_999_999),
            ),
            (1_000_000_000_000_000_000, DeclaredMax::Unlimited),
            (u64::MAX, DeclaredMax::Unlimited),
        ] {
            plan.max_concurrent_agents_per_executor = raw;
            plan.oplog_writes_per_second = raw;
            assert_eq!(
                declared(&plan, PlanDimension::MaxConcurrentAgentsPerExecutor),
                expected
            );
            assert_eq!(
                declared(&plan, PlanDimension::OplogWritesPerSecond),
                expected
            );
        }

        plan.app_limit = 1_000_000_000_000_000_000;
        plan.per_invocation_http_call_limit = 0;
        assert_eq!(
            declared(&plan, PlanDimension::ApplicationCount),
            DeclaredMax::Finite(1_000_000_000_000_000_000)
        );
        assert_eq!(
            declared(&plan, PlanDimension::PerInvocationHttpCalls),
            DeclaredMax::Finite(0)
        );
    }
}
