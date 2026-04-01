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

use super::environment::EnvironmentId;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, declare_unions,
    newtype_uuid,
};
use derive_more::Display;

newtype_uuid!(
    ResourceDefinitionId,
    golem_api_grpc::proto::golem::common::ResourceDefinitionId
);

declare_revision!(ResourceDefinitionRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct ResourceName(pub String);
}

declare_structs! {
    // name and limit type are immutable after creation.
    // environment_id+name form the logical primary key for non deleted resources.
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct ResourceDefinition {
        pub id: ResourceDefinitionId,
        pub revision: ResourceDefinitionRevision,
        pub environment_id: EnvironmentId,
        pub name: ResourceName,

        pub limit: ResourceLimit,
        pub enforcement_action: EnforcementAction,

        /// single unit of measurement (e.g., token, request)
        pub unit: String,
        /// multiple units of measurement (e.g., tokens, requests)
        pub units: String,
    }

    pub struct ResourceDefinitionCreation {
        pub name: ResourceName,

        pub limit: ResourceLimit,
        pub enforcement_action: EnforcementAction,

        /// single unit of measurement (e.g., token, request)
        pub unit: String,
        /// multiple units of measurement (e.g., tokens, requests)
        pub units: String,
    }

    pub struct ResourceDefinitionUpdate {
        pub current_revision: ResourceDefinitionRevision,
        pub limit: Option<ResourceLimit>,
        pub enforcement_action: Option<EnforcementAction>,

        /// single unit of measurement (e.g., token, request)
        pub unit: Option<String>,
        /// multiple units of measurement (e.g., tokens, requests)
        pub units: Option<String>,
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct ResourceRateLimit {
        pub value: u64,
        pub period: TimePeriod,
        /// Maximum burst capacity. Defaults to `value` if not specified.
        pub max: u64
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct ResourceCapacityLimit {
        pub value: u64
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub struct ResourceConcurrencyLimit {
        pub value: u64
    }
}

declare_unions! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum ResourceLimit {
        Rate(ResourceRateLimit),
        Capacity(ResourceCapacityLimit),
        Concurrency(ResourceConcurrencyLimit)
    }
}

declare_enums! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum EnforcementAction {
        Reject,
        Throttle,
        Terminate
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum TimePeriod {
        Second,
        Minute,
        Hour,
        Day,
        Month,
        Year
    }
}
