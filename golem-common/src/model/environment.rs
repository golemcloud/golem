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

use super::account::AccountId;
use super::application::ApplicationId;
use super::auth::EnvironmentRole;
use super::deployment::DeploymentRevision;
use super::diff::Hash;
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use std::collections::HashSet;
use std::str::FromStr;

newtype_uuid!(
    EnvironmentId,
    golem_api_grpc::proto::golem::common::EnvironmentId
);

declare_revision!(EnvironmentRevision);

declare_transparent_newtypes! {
    #[derive(Eq, Hash, PartialOrd, Ord)]
    pub struct EnvironmentName(pub String);
}

impl TryFrom<String> for EnvironmentName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        // TODO: atomic: Add validations
        Ok(EnvironmentName(value))
    }
}

impl FromStr for EnvironmentName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

declare_structs! {
    pub struct EnvironmentCreation {
        pub name: EnvironmentName,
        pub compatibility_check: bool,
        pub version_check: bool,
        pub security_overrides: bool,
    }

    pub struct EnvironmentUpdate {
        pub current_revision: EnvironmentRevision,
        pub new_name: Option<EnvironmentName>
    }

    pub struct EnvironmentCurrentDeploymentView {
        pub revision: DeploymentRevision,
        pub hash: Hash
    }

    pub struct Environment {
        pub id: EnvironmentId,
        pub revision: EnvironmentRevision,
        pub application_id: ApplicationId,
        pub name: EnvironmentName,
        pub compatibility_check: bool,
        pub version_check: bool,
        pub security_overrides: bool,

        pub owner_account_id: AccountId,
        /// Roles in the environment that were given to the current user by shares. The owner always has full access.
        /// Note that even if getting a deleted environment only non-deleted shares are considered.
        pub roles_from_active_shares: HashSet<EnvironmentRole>,

        pub current_deployment: Option<EnvironmentCurrentDeploymentView>,
    }
}
