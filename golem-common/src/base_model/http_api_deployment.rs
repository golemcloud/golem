// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::base_model::agent::AgentTypeName;
use crate::base_model::diff;
use crate::base_model::domain_registration::Domain;
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, declare_structs, newtype_uuid};
use chrono::DateTime;
use std::collections::BTreeSet;

newtype_uuid!(HttpApiDeploymentId);

declare_revision!(HttpApiDeploymentRevision);

declare_structs! {
    pub struct HttpApiDeploymentCreation {
        pub domain: Domain,
        pub agent_types: BTreeSet<AgentTypeName>
    }

    pub struct HttpApiDeploymentUpdate {
        pub current_revision: HttpApiDeploymentRevision,
        pub agent_types: Option<BTreeSet<AgentTypeName>>
    }

    pub struct HttpApiDeployment {
        pub id: HttpApiDeploymentId,
        pub revision: HttpApiDeploymentRevision,
        pub environment_id: EnvironmentId,
        pub domain: Domain,
        pub hash: diff::Hash,
        pub agent_types: BTreeSet<AgentTypeName>,
        pub created_at: DateTime<chrono::Utc>,
    }
}
