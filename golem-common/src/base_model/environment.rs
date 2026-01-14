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

use crate::base_model::account::{AccountId, AccountSummary};
use crate::base_model::application::{ApplicationId, ApplicationSummary};
use crate::base_model::auth::EnvironmentRole;
use crate::base_model::deployment::{
    CurrentDeploymentRevision, DeploymentRevision, DeploymentVersion,
};
use crate::base_model::diff::Hash;
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use derive_more::Display;
use std::collections::BTreeSet;
use std::str::FromStr;

newtype_uuid!(
    EnvironmentId,
    golem_api_grpc::proto::golem::common::EnvironmentId
);

declare_revision!(EnvironmentRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    pub struct EnvironmentName(pub String);
}

impl TryFrom<String> for EnvironmentName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Environment", &value)?;
        Ok(EnvironmentName(value))
    }
}

impl TryFrom<&str> for EnvironmentName {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.to_string().try_into()
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
        pub name: Option<EnvironmentName>,
        pub compatibility_check: Option<bool>,
        pub version_check: Option<bool>,
        pub security_overrides: Option<bool>,
    }

    pub struct EnvironmentCurrentDeploymentView {
        pub revision: CurrentDeploymentRevision,
        pub deployment_revision: DeploymentRevision,
        pub deployment_version: DeploymentVersion,
        pub deployment_hash: Hash
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
        pub roles_from_active_shares: BTreeSet<EnvironmentRole>,

        pub current_deployment: Option<EnvironmentCurrentDeploymentView>,
    }

    pub struct EnvironmentSummary {
        pub id: EnvironmentId,
        pub revision: EnvironmentRevision,
        pub name: EnvironmentName,
        pub compatibility_check: bool,
        pub version_check: bool,
        pub security_overrides: bool,
        pub roles_from_active_shares: BTreeSet<EnvironmentRole>,
        pub current_deployment: Option<EnvironmentCurrentDeploymentView>,
    }

    /// A cross-tenant, enriched view of an environment used specifically by the
    /// `GET /environments` endpoint.
    ///
    /// This type exists because a user may be able to see an environment (as the
    /// owner or through an active share) **without having access to its parent
    /// application or owning account**. In those cases, returning only the
    /// `Environment` model would not provide enough context for UI navigation or
    /// for users to understand *whose* environment they are interacting with.
    ///
    /// `EnvironmentWithDetails` therefore bundles:
    ///   - a summarized `Environment` (fields intrinsic to the environment),
    ///   - minimal identifying information about the parent `Application`,
    ///   - minimal identifying information about the owning `Account`.
    ///
    /// This structure is *only* used for environment discovery/listing.
    /// All other environment endpoints (`GET /environments/:id`, updates, deletes)
    /// continue to return the standard `Environment` model, since those requests
    /// operate within the application/account they belong to.
    ///
    /// In short:
    /// **EnvironmentWithDetails = EnvironmentSummary + minimal parent context**,
    /// enabling safe, cross-tenant environment visibility without exposing
    /// full application/account resources.
    pub struct EnvironmentWithDetails {
        pub environment: EnvironmentSummary,
        pub application: ApplicationSummary,
        pub account: AccountSummary
    }
}
