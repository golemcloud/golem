// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::type_builder::TypeNodeBuilder;
use crate::bindings::golem::api::host::{
    AgentAllFilter, AgentAnyFilter, AgentConfigVarsFilter, AgentCreatedAtFilter, AgentEnvFilter,
    AgentMetadata, AgentNameFilter, AgentPropertyFilter, AgentStatus, AgentStatusFilter,
    AgentVersionFilter, FilterComparator, StringFilterComparator, UpdateMode,
};
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::{AgentId, NodeBuilder, WitValueExtractor};

// UpdateMode

impl IntoValue for UpdateMode {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let variant_idx = match self {
            UpdateMode::Automatic => 0,
            UpdateMode::SnapshotBased => 1,
        };
        builder.variant_unit(variant_idx)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder =
            builder.variant(Some("UpdateMode".to_string()), Some("golem".to_string()));
        builder = builder.unit_case("automatic");
        builder = builder.unit_case("snapshot-based");
        builder.finish()
    }
}

impl FromValueAndType for UpdateMode {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected UpdateMode to be a variant".to_string())?;
        if inner.is_some() {
            return Err("UpdateMode variants should not have values".to_string());
        }
        match idx {
            0 => Ok(UpdateMode::Automatic),
            1 => Ok(UpdateMode::SnapshotBased),
            _ => Err(format!("Invalid UpdateMode variant index: {}", idx)),
        }
    }
}

// FilterComparator

impl IntoValue for FilterComparator {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let variant_idx = match self {
            FilterComparator::Equal => 0,
            FilterComparator::NotEqual => 1,
            FilterComparator::GreaterEqual => 2,
            FilterComparator::Greater => 3,
            FilterComparator::LessEqual => 4,
            FilterComparator::Less => 5,
        };
        builder.variant_unit(variant_idx)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.variant(
            Some("FilterComparator".to_string()),
            Some("golem".to_string()),
        );
        builder = builder.unit_case("equal");
        builder = builder.unit_case("not-equal");
        builder = builder.unit_case("greater-equal");
        builder = builder.unit_case("greater");
        builder = builder.unit_case("less-equal");
        builder = builder.unit_case("less");
        builder.finish()
    }
}

impl FromValueAndType for FilterComparator {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected FilterComparator to be a variant".to_string())?;
        if inner.is_some() {
            return Err("FilterComparator variants should not have values".to_string());
        }
        match idx {
            0 => Ok(FilterComparator::Equal),
            1 => Ok(FilterComparator::NotEqual),
            2 => Ok(FilterComparator::GreaterEqual),
            3 => Ok(FilterComparator::Greater),
            4 => Ok(FilterComparator::LessEqual),
            5 => Ok(FilterComparator::Less),
            _ => Err(format!("Invalid FilterComparator variant index: {}", idx)),
        }
    }
}

// StringFilterComparator

impl IntoValue for StringFilterComparator {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let variant_idx = match self {
            StringFilterComparator::Equal => 0,
            StringFilterComparator::NotEqual => 1,
            StringFilterComparator::Like => 2,
            StringFilterComparator::NotLike => 3,
            StringFilterComparator::StartsWith => 4,
        };
        builder.variant_unit(variant_idx)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.variant(
            Some("StringFilterComparator".to_string()),
            Some("golem".to_string()),
        );
        builder = builder.unit_case("equal");
        builder = builder.unit_case("not-equal");
        builder = builder.unit_case("like");
        builder = builder.unit_case("not-like");
        builder = builder.unit_case("starts-with");
        builder.finish()
    }
}

impl FromValueAndType for StringFilterComparator {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected StringFilterComparator to be a variant".to_string())?;
        if inner.is_some() {
            return Err("StringFilterComparator variants should not have values".to_string());
        }
        match idx {
            0 => Ok(StringFilterComparator::Equal),
            1 => Ok(StringFilterComparator::NotEqual),
            2 => Ok(StringFilterComparator::Like),
            3 => Ok(StringFilterComparator::NotLike),
            4 => Ok(StringFilterComparator::StartsWith),
            _ => Err(format!(
                "Invalid StringFilterComparator variant index: {}",
                idx
            )),
        }
    }
}

// AgentStatus

impl IntoValue for AgentStatus {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let variant_idx = match self {
            AgentStatus::Running => 0,
            AgentStatus::Idle => 1,
            AgentStatus::Suspended => 2,
            AgentStatus::Interrupted => 3,
            AgentStatus::Retrying => 4,
            AgentStatus::Failed => 5,
            AgentStatus::Exited => 6,
        };
        builder.variant_unit(variant_idx)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder =
            builder.variant(Some("AgentStatus".to_string()), Some("golem".to_string()));
        builder = builder.unit_case("running");
        builder = builder.unit_case("idle");
        builder = builder.unit_case("suspended");
        builder = builder.unit_case("interrupted");
        builder = builder.unit_case("retrying");
        builder = builder.unit_case("failed");
        builder = builder.unit_case("exited");
        builder.finish()
    }
}

impl FromValueAndType for AgentStatus {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected AgentStatus to be a variant".to_string())?;
        if inner.is_some() {
            return Err("AgentStatus variants should not have values".to_string());
        }
        match idx {
            0 => Ok(AgentStatus::Running),
            1 => Ok(AgentStatus::Idle),
            2 => Ok(AgentStatus::Suspended),
            3 => Ok(AgentStatus::Interrupted),
            4 => Ok(AgentStatus::Retrying),
            5 => Ok(AgentStatus::Failed),
            6 => Ok(AgentStatus::Exited),
            _ => Err(format!("Invalid AgentStatus variant index: {}", idx)),
        }
    }
}

// AgentNameFilter

impl IntoValue for AgentNameFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.comparator.add_to_builder(builder.item());
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentNameFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <StringFilterComparator>::add_to_type_builder(builder.field("comparator"));
        let builder = <String>::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}

impl FromValueAndType for AgentNameFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let comparator = <StringFilterComparator>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing comparator field".to_string())?,
        )?;
        let value = <String>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing value field".to_string())?,
        )?;
        Ok(AgentNameFilter { comparator, value })
    }
}

// AgentStatusFilter

impl IntoValue for AgentStatusFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.comparator.add_to_builder(builder.item());
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentStatusFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <FilterComparator>::add_to_type_builder(builder.field("comparator"));
        let builder = <AgentStatus>::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}

impl FromValueAndType for AgentStatusFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let comparator = <FilterComparator>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing comparator field".to_string())?,
        )?;
        let value = <AgentStatus>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing value field".to_string())?,
        )?;
        Ok(AgentStatusFilter { comparator, value })
    }
}

// AgentVersionFilter

impl IntoValue for AgentVersionFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.comparator.add_to_builder(builder.item());
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentVersionFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <FilterComparator>::add_to_type_builder(builder.field("comparator"));
        let builder = <u64>::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}

impl FromValueAndType for AgentVersionFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let comparator = <FilterComparator>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing comparator field".to_string())?,
        )?;
        let value = <u64>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing value field".to_string())?,
        )?;
        Ok(AgentVersionFilter { comparator, value })
    }
}

// AgentCreatedAtFilter

impl IntoValue for AgentCreatedAtFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.comparator.add_to_builder(builder.item());
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentCreatedAtFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <FilterComparator>::add_to_type_builder(builder.field("comparator"));
        let builder = <u64>::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}

impl FromValueAndType for AgentCreatedAtFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let comparator = <FilterComparator>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing comparator field".to_string())?,
        )?;
        let value = <u64>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing value field".to_string())?,
        )?;
        Ok(AgentCreatedAtFilter { comparator, value })
    }
}

// AgentEnvFilter

impl IntoValue for AgentEnvFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.name.add_to_builder(builder.item());
        let builder = self.comparator.add_to_builder(builder.item());
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentEnvFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <String>::add_to_type_builder(builder.field("name"));
        let builder = <StringFilterComparator>::add_to_type_builder(builder.field("comparator"));
        let builder = <String>::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}

impl FromValueAndType for AgentEnvFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let name = <String>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing name field".to_string())?,
        )?;
        let comparator = <StringFilterComparator>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing comparator field".to_string())?,
        )?;
        let value = <String>::from_extractor(
            &extractor
                .field(2usize)
                .ok_or_else(|| "Missing value field".to_string())?,
        )?;
        Ok(AgentEnvFilter {
            name,
            comparator,
            value,
        })
    }
}

// AgentConfigVarsFilter

impl IntoValue for AgentConfigVarsFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.name.add_to_builder(builder.item());
        let builder = self.comparator.add_to_builder(builder.item());
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentConfigVarsFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <String>::add_to_type_builder(builder.field("name"));
        let builder = <StringFilterComparator>::add_to_type_builder(builder.field("comparator"));
        let builder = <String>::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}

impl FromValueAndType for AgentConfigVarsFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let name = <String>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing name field".to_string())?,
        )?;
        let comparator = <StringFilterComparator>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing comparator field".to_string())?,
        )?;
        let value = <String>::from_extractor(
            &extractor
                .field(2usize)
                .ok_or_else(|| "Missing value field".to_string())?,
        )?;
        Ok(AgentConfigVarsFilter {
            name,
            comparator,
            value,
        })
    }
}

// AgentPropertyFilter

impl IntoValue for AgentPropertyFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        match self {
            AgentPropertyFilter::Name(inner) => {
                let builder = builder.variant(0u32);
                inner.add_to_builder(builder).finish()
            }
            AgentPropertyFilter::Status(inner) => {
                let builder = builder.variant(1u32);
                inner.add_to_builder(builder).finish()
            }
            AgentPropertyFilter::Version(inner) => {
                let builder = builder.variant(2u32);
                inner.add_to_builder(builder).finish()
            }
            AgentPropertyFilter::CreatedAt(inner) => {
                let builder = builder.variant(3u32);
                inner.add_to_builder(builder).finish()
            }
            AgentPropertyFilter::Env(inner) => {
                let builder = builder.variant(4u32);
                inner.add_to_builder(builder).finish()
            }
            AgentPropertyFilter::WasiConfigVars(inner) => {
                let builder = builder.variant(5u32);
                inner.add_to_builder(builder).finish()
            }
        }
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.variant(
            Some("AgentPropertyFilter".to_string()),
            Some("golem".to_string()),
        );
        builder = <AgentNameFilter>::add_to_type_builder(builder.case("name"));
        builder = <AgentStatusFilter>::add_to_type_builder(builder.case("status"));
        builder = <AgentVersionFilter>::add_to_type_builder(builder.case("version"));
        builder = <AgentCreatedAtFilter>::add_to_type_builder(builder.case("created-at"));
        builder = <AgentEnvFilter>::add_to_type_builder(builder.case("env"));
        builder = <AgentConfigVarsFilter>::add_to_type_builder(builder.case("wasi-config-vars"));
        builder.finish()
    }
}

impl FromValueAndType for AgentPropertyFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected AgentPropertyFilter to be a variant".to_string())?;
        match idx {
            0 => {
                let value = <AgentNameFilter>::from_extractor(
                    &inner.ok_or_else(|| "Missing AgentPropertyFilter::Name body".to_string())?,
                )?;
                Ok(AgentPropertyFilter::Name(value))
            }
            1 => {
                let value = <AgentStatusFilter>::from_extractor(
                    &inner.ok_or_else(|| "Missing AgentPropertyFilter::Status body".to_string())?,
                )?;
                Ok(AgentPropertyFilter::Status(value))
            }
            2 => {
                let value = <AgentVersionFilter>::from_extractor(
                    &inner
                        .ok_or_else(|| "Missing AgentPropertyFilter::Version body".to_string())?,
                )?;
                Ok(AgentPropertyFilter::Version(value))
            }
            3 => {
                let value =
                    <AgentCreatedAtFilter>::from_extractor(&inner.ok_or_else(|| {
                        "Missing AgentPropertyFilter::CreatedAt body".to_string()
                    })?)?;
                Ok(AgentPropertyFilter::CreatedAt(value))
            }
            4 => {
                let value = <AgentEnvFilter>::from_extractor(
                    &inner.ok_or_else(|| "Missing AgentPropertyFilter::Env body".to_string())?,
                )?;
                Ok(AgentPropertyFilter::Env(value))
            }
            5 => {
                let value = <AgentConfigVarsFilter>::from_extractor(&inner.ok_or_else(|| {
                    "Missing AgentPropertyFilter::WasiConfigVars body".to_string()
                })?)?;
                Ok(AgentPropertyFilter::WasiConfigVars(value))
            }
            _ => Err(format!(
                "Invalid AgentPropertyFilter variant index: {}",
                idx
            )),
        }
    }
}

// AgentAllFilter

impl IntoValue for AgentAllFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.filters.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentAllFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <Vec<AgentPropertyFilter>>::add_to_type_builder(builder.field("filters"));
        builder.finish()
    }
}

impl FromValueAndType for AgentAllFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let filters = <Vec<AgentPropertyFilter>>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing filters field".to_string())?,
        )?;
        Ok(AgentAllFilter { filters })
    }
}

// AgentAnyFilter

impl IntoValue for AgentAnyFilter {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.filters.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("AgentAnyFilter".to_string()),
            Some("golem".to_string()),
        );
        let builder = <Vec<AgentAllFilter>>::add_to_type_builder(builder.field("filters"));
        builder.finish()
    }
}

impl FromValueAndType for AgentAnyFilter {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let filters = <Vec<AgentAllFilter>>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing filters field".to_string())?,
        )?;
        Ok(AgentAnyFilter { filters })
    }
}

// AgentMetadata

impl IntoValue for AgentMetadata {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.agent_id.add_to_builder(builder.item());
        let builder = self.args.add_to_builder(builder.item());
        let builder = self.env.add_to_builder(builder.item());
        let builder = self.config_vars.add_to_builder(builder.item());
        let builder = self.status.add_to_builder(builder.item());
        let builder = self.component_revision.add_to_builder(builder.item());
        let builder = self.retry_count.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("AgentMetadata".to_string()), Some("golem".to_string()));
        let builder = <AgentId>::add_to_type_builder(builder.field("agent-id"));
        let builder = <Vec<String>>::add_to_type_builder(builder.field("args"));
        let builder = <Vec<(String, String)>>::add_to_type_builder(builder.field("env"));
        let builder = <Vec<(String, String)>>::add_to_type_builder(builder.field("config-vars"));
        let builder = <AgentStatus>::add_to_type_builder(builder.field("status"));
        let builder = <u64>::add_to_type_builder(builder.field("component-revision"));
        let builder = <u64>::add_to_type_builder(builder.field("retry-count"));
        builder.finish()
    }
}

impl FromValueAndType for AgentMetadata {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let agent_id = <AgentId>::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing agent-id field".to_string())?,
        )?;
        let args = <Vec<String>>::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing args field".to_string())?,
        )?;
        let env = <Vec<(String, String)>>::from_extractor(
            &extractor
                .field(2usize)
                .ok_or_else(|| "Missing env field".to_string())?,
        )?;
        let config_vars = <Vec<(String, String)>>::from_extractor(
            &extractor
                .field(3usize)
                .ok_or_else(|| "Missing config-vars field".to_string())?,
        )?;
        let status = <AgentStatus>::from_extractor(
            &extractor
                .field(4usize)
                .ok_or_else(|| "Missing status field".to_string())?,
        )?;
        let component_revision = <u64>::from_extractor(
            &extractor
                .field(5usize)
                .ok_or_else(|| "Missing component-revision field".to_string())?,
        )?;
        let retry_count = <u64>::from_extractor(
            &extractor
                .field(6usize)
                .ok_or_else(|| "Missing retry-count field".to_string())?,
        )?;
        Ok(AgentMetadata {
            agent_id,
            args,
            env,
            config_vars,
            status,
            component_revision,
            retry_count,
        })
    }
}
