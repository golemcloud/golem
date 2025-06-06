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

use super::ProjectId;
use crate::base_model::{ComponentId, ComponentVersion};
use crate::model::AccountId;
use bincode::{Decode, Encode};
use core::fmt;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ComponentOwner {
    pub project_id: ProjectId,
    pub account_id: AccountId,
}

impl Display for ComponentOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.account_id, self.project_id)
    }
}

impl FromStr for ComponentOwner {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid namespace: {s}"));
        }

        Ok(Self {
            project_id: ProjectId::try_from(parts[1])?,
            account_id: AccountId::from(parts[0]),
        })
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]

pub struct VersionedComponentId {
    pub component_id: ComponentId,
    pub version: ComponentVersion,
}

impl Display for VersionedComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.component_id, self.version)
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::component::VersionedComponentId;

    impl TryFrom<golem_api_grpc::proto::golem::component::VersionedComponentId>
        for VersionedComponentId
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::VersionedComponentId,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: value
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                version: value.version,
            })
        }
    }

    impl From<VersionedComponentId> for golem_api_grpc::proto::golem::component::VersionedComponentId {
        fn from(value: VersionedComponentId) -> Self {
            Self {
                component_id: Some(value.component_id.into()),
                version: value.version,
            }
        }
    }
}
