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

use super::auth::Namespace;
use super::{AccountId, ProjectId};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectView {
    pub id: ProjectId,
    pub owner_account_id: AccountId,
    pub name: String,
    pub description: String,
}

impl From<ProjectView> for Namespace {
    fn from(value: ProjectView) -> Self {
        Namespace::new(value.id, value.owner_account_id)
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::ProjectView;

    impl TryFrom<golem_api_grpc::proto::golem::project::Project> for ProjectView {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::project::Project,
        ) -> Result<Self, Self::Error> {
            let golem_api_grpc::proto::golem::project::ProjectData {
                name,
                description,
                owner_account_id,
                ..
            } = value.data.ok_or("Missing data")?;
            Ok(Self {
                id: value.id.ok_or("Missing id")?.try_into()?,
                owner_account_id: owner_account_id.ok_or("Missing owner_account_id")?.into(),
                name,
                description,
            })
        }
    }
}
