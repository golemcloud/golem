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

use crate::model::diff::{hash_from_serialized_value, Diffable, Hash, Hashable};
use crate::model::resource_definition::{EnforcementAction, TimePeriod};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceDefinition {
    pub limit: ResourceLimit,
    pub enforcement_action: EnforcementAction,
    pub unit: String,
    pub units: String,
}

impl Diffable for ResourceDefinition {
    type DiffResult = ResourceDefinitionDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let limit_changed = new.limit != current.limit;
        let enforcement_action_changed = new.enforcement_action != current.enforcement_action;
        let unit_changed = new.unit != current.unit;
        let units_changed = new.units != current.units;

        if limit_changed || enforcement_action_changed || unit_changed || units_changed {
            Some(Self::DiffResult {
                limit_changed,
                enforcement_action_changed,
                unit_changed,
                units_changed,
            })
        } else {
            None
        }
    }
}

impl Hashable for ResourceDefinition {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceLimit {
    pub value: u64,
    pub period: Option<TimePeriod>,
}

impl From<crate::model::resource_definition::ResourceLimit> for ResourceLimit {
    fn from(value: crate::model::resource_definition::ResourceLimit) -> Self {
        use crate::model::resource_definition::ResourceLimit as DomainResourceLimit;

        match value {
            DomainResourceLimit::Rate(inner) => ResourceLimit {
                value: inner.value,
                period: Some(inner.period),
            },
            DomainResourceLimit::Capacity(inner) => ResourceLimit {
                value: inner.value,
                period: None,
            },
            DomainResourceLimit::Concurrency(inner) => ResourceLimit {
                value: inner.value,
                period: None,
            },
        }
    }
}

impl Diffable for ResourceLimit {
    type DiffResult = ResourceLimitDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let value_changed = new.value != current.value;
        let period_changed = new.period != current.period;

        if value_changed || period_changed {
            Some(Self::DiffResult {
                value_changed,
                period_changed,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDefinitionDiff {
    pub limit_changed: bool,
    pub enforcement_action_changed: bool,
    pub unit_changed: bool,
    pub units_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimitDiff {
    pub value_changed: bool,
    pub period_changed: bool,
}
