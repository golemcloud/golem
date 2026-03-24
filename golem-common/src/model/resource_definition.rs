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

use crate::model::diff;

pub use crate::base_model::resource_definition::*;

impl ResourceDefinition {
    pub fn to_diffable(&self) -> diff::ResourceDefinition {
        diff::ResourceDefinition {
            limit: self.limit.clone().into(),
            enforcement_action: self.enforcement_action,
            unit: self.unit.clone(),
            units: self.units.clone(),
        }
    }
}

mod protobuf {
    use super::{
        EnforcementAction, ResourceCapacityLimit, ResourceConcurrencyLimit, ResourceDefinition,
        ResourceLimit, ResourceName, ResourceRateLimit, TimePeriod,
    };

    impl From<TimePeriod> for golem_api_grpc::proto::golem::registry::TimePeriod {
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

    impl TryFrom<golem_api_grpc::proto::golem::registry::TimePeriod> for TimePeriod {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::TimePeriod,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::registry::TimePeriod as GrpcTimePeriod;

            match value {
                GrpcTimePeriod::Second => Ok(Self::Second),
                GrpcTimePeriod::Minute => Ok(Self::Minute),
                GrpcTimePeriod::Hour => Ok(Self::Hour),
                GrpcTimePeriod::Day => Ok(Self::Day),
                GrpcTimePeriod::Month => Ok(Self::Month),
                GrpcTimePeriod::Year => Ok(Self::Year),
                GrpcTimePeriod::Unknown => Err("Unknown TimePeriod type".to_string()),
            }
        }
    }

    impl From<ResourceLimit> for golem_api_grpc::proto::golem::registry::ResourceLimit {
        fn from(value: ResourceLimit) -> Self {
            use golem_api_grpc::proto::golem::registry::resource_limit as grpc;
            match value {
                ResourceLimit::Rate(inner) => Self {
                    kind: Some(grpc::Kind::Rate(grpc::Rate {
                        value: inner.value,
                        period: golem_api_grpc::proto::golem::registry::TimePeriod::from(
                            inner.period,
                        )
                        .into(),
                        max: inner.max,
                    })),
                },
                ResourceLimit::Capacity(inner) => Self {
                    kind: Some(grpc::Kind::Capacity(grpc::Capacity { value: inner.value })),
                },
                ResourceLimit::Concurrency(inner) => Self {
                    kind: Some(grpc::Kind::Concurrency(grpc::Concurrency {
                        value: inner.value,
                    })),
                },
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::ResourceLimit> for ResourceLimit {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::ResourceLimit,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::registry::resource_limit as grpc_resource_limit;

            match value.kind.ok_or("ResourceLimit.kind missing")? {
                grpc_resource_limit::Kind::Rate(inner) => {
                    let max = if inner.max > 0 {
                        inner.max
                    } else {
                        inner.value
                    };
                    Ok(ResourceLimit::Rate(ResourceRateLimit {
                        value: inner.value,
                        period: inner.period().try_into()?,
                        max,
                    }))
                }
                grpc_resource_limit::Kind::Capacity(inner) => {
                    Ok(ResourceLimit::Capacity(ResourceCapacityLimit {
                        value: inner.value,
                    }))
                }
                grpc_resource_limit::Kind::Concurrency(inner) => {
                    Ok(ResourceLimit::Concurrency(ResourceConcurrencyLimit {
                        value: inner.value,
                    }))
                }
            }
        }
    }

    impl From<EnforcementAction> for golem_api_grpc::proto::golem::registry::EnforcementAction {
        fn from(value: EnforcementAction) -> Self {
            match value {
                EnforcementAction::Reject => Self::Reject,
                EnforcementAction::Terminate => Self::Terminate,
                EnforcementAction::Throttle => Self::Throttle,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::EnforcementAction> for EnforcementAction {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::EnforcementAction,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::registry::EnforcementAction as GrpcEnforcementAction;

            match value {
                GrpcEnforcementAction::Reject => Ok(Self::Reject),
                GrpcEnforcementAction::Terminate => Ok(Self::Terminate),
                GrpcEnforcementAction::Throttle => Ok(Self::Throttle),
                GrpcEnforcementAction::Unknown => Err("Unknown EnforcementAction type".to_string()),
            }
        }
    }

    impl From<ResourceDefinition> for golem_api_grpc::proto::golem::registry::ResourceDefinition {
        fn from(value: ResourceDefinition) -> Self {
            Self {
                resource_definition_id: Some(value.id.into()),
                revision: value.revision.into(),
                environment_id: Some(value.environment_id.into()),
                name: value.name.0,

                resource_limit: Some(value.limit.into()),
                enforcement_action:
                    golem_api_grpc::proto::golem::registry::EnforcementAction::from(
                        value.enforcement_action,
                    )
                    .into(),

                unit: value.unit,
                units: value.units,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::ResourceDefinition> for ResourceDefinition {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::ResourceDefinition,
        ) -> Result<Self, Self::Error> {
            let enforcement_action = value.enforcement_action().try_into()?;

            Ok(Self {
                id: value
                    .resource_definition_id
                    .ok_or("Missing resource_definition_id field")?
                    .try_into()?,
                revision: value.revision.try_into()?,
                environment_id: value
                    .environment_id
                    .ok_or("Missing environment_id field")?
                    .try_into()?,
                name: ResourceName(value.name),

                limit: value
                    .resource_limit
                    .ok_or("missing resource_limit_field")?
                    .try_into()?,
                enforcement_action,

                unit: value.unit,
                units: value.units,
            })
        }
    }
}
