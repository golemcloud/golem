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

use crate::model::account::AccountId;
use crate::model::ProjectId;
use std::collections::HashSet;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, FromRepr};
use uuid::Uuid;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct TokenSecret {
    pub value: Uuid,
}

impl TokenSecret {
    pub fn new(value: Uuid) -> Self {
        Self { value }
    }
}

impl std::str::FromStr for TokenSecret {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(s).map_err(|err| format!("Invalid token: {err}"))?;
        Ok(Self { value: uuid })
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
    EnumIter,
    FromRepr,
)]
#[repr(i32)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
pub enum Role {
    Admin = 0,
    MarketingAdmin = 1,
}

impl From<Role> for i32 {
    fn from(value: Role) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for Role {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Role::from_repr(value).ok_or_else(|| format!("Invalid role: {value}"))
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            _ => Err(format!("Unknown role id: {s}")),
        }
    }
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Role::Admin => write!(f, "Admin"),
            Role::MarketingAdmin => write!(f, "MarketingAdmin"),
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct AuthCtx {
    pub token_secret: TokenSecret,
}

impl AuthCtx {
    pub fn new(token_secret: TokenSecret) -> Self {
        Self { token_secret }
    }
}

impl IntoIterator for AuthCtx {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![(
            "authorization".to_string(),
            format!("Bearer {}", self.token_secret.value),
        )]
        .into_iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, EnumIter, FromRepr)]
#[repr(i32)]
pub enum AccountAction {
    ViewAccount = 0,
    UpdateAccount = 1,
    ViewPlan = 2,
    CreateProject = 3,
    DeleteAccount = 4,
    ViewAccountGrants = 5,
    CreateAccountGrant = 6,
    DeleteAccountGrant = 7,
    ViewDefaultProject = 8,
    ListProjectGrants = 9,
    ViewLimits = 10,
    UpdateLimits = 11,
    ViewTokens = 12,
    CreateToken = 13,
    DeleteToken = 14,
    ViewGlobalPlugins = 15,
    CreateGlobalPlugin = 16,
    UpdateGlobalPlugin = 17,
    DeleteGlobalPlugin = 18,
}

impl From<AccountAction> for i32 {
    fn from(value: AccountAction) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for AccountAction {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        AccountAction::from_repr(value).ok_or_else(|| format!("Invalid account action: {value}"))
    }
}

impl Display for AccountAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ViewAccount => write!(f, "ViewAccount"),
            Self::UpdateAccount => write!(f, "UpdateAccount"),
            Self::ViewPlan => write!(f, "ViewPlan"),
            Self::CreateProject => write!(f, "CreateProject"),
            Self::DeleteAccount => write!(f, "DeleteAccount"),
            Self::ViewAccountGrants => write!(f, "ViewAccountGrants"),
            Self::CreateAccountGrant => write!(f, "CreateAccountGrant"),
            Self::DeleteAccountGrant => write!(f, "DeleteAccountGrant"),
            Self::ViewDefaultProject => write!(f, "ViewDefaultProject"),
            Self::ListProjectGrants => write!(f, "ListProjectGrants"),
            Self::ViewLimits => write!(f, "ViewLimits"),
            Self::UpdateLimits => write!(f, "UpdateLimits"),
            Self::ViewTokens => write!(f, "ViewTokens"),
            Self::CreateToken => write!(f, "CreateToken"),
            Self::DeleteToken => write!(f, "DeleteToken"),
            Self::ViewGlobalPlugins => write!(f, "ViewGlobalPlugin"),
            Self::CreateGlobalPlugin => write!(f, "CreateGlobalPlugin"),
            Self::UpdateGlobalPlugin => write!(f, "UpdateGlobalPlugin"),
            Self::DeleteGlobalPlugin => write!(f, "DeleteGlobalPlugin"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, EnumIter, FromRepr)]
#[repr(i32)]
pub enum ProjectAction {
    ViewComponent = 0,
    CreateComponent = 1,
    UpdateComponent = 2,
    DeleteComponent = 3,
    ViewWorker = 4,
    CreateWorker = 5,
    UpdateWorker = 6,
    DeleteWorker = 7,
    ViewProjectGrants = 8,
    CreateProjectGrants = 9,
    DeleteProjectGrants = 10,
    ViewApiDefinition = 11,
    CreateApiDefinition = 12,
    UpdateApiDefinition = 13,
    DeleteApiDefinition = 14,
    DeleteProject = 15,
    ViewProject = 16,
    ViewPluginInstallations = 17,
    CreatePluginInstallation = 18,
    UpdatePluginInstallation = 19,
    DeletePluginInstallation = 20,
    UpsertApiDeployment = 21,
    ViewApiDeployment = 22,
    DeleteApiDeployment = 23,
    UpsertApiDomain = 24,
    ViewApiDomain = 25,
    DeleteApiDomain = 26,
    BatchUpdatePluginInstallations = 27,
    ViewPluginDefinition = 28,
    CreatePluginDefinition = 29,
    UpdatePluginDefinition = 30,
    DeletePluginDefinition = 31,
    ExportApiDefinition = 32,
}

impl From<ProjectAction> for i32 {
    fn from(value: ProjectAction) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for ProjectAction {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        ProjectAction::from_repr(value).ok_or_else(|| format!("Invalid project action: {value}"))
    }
}

impl Display for ProjectAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ViewComponent => write!(f, "ViewComponent"),
            Self::CreateComponent => write!(f, "CreateComponent"),
            Self::UpdateComponent => write!(f, "UpdateComponent"),
            Self::DeleteComponent => write!(f, "DeleteComponent"),
            Self::ViewWorker => write!(f, "ViewWorker"),
            Self::CreateWorker => write!(f, "CreateWorker"),
            Self::UpdateWorker => write!(f, "UpdateWorker"),
            Self::DeleteWorker => write!(f, "DeleteWorker"),
            Self::ViewProjectGrants => write!(f, "ViewProjectGrants"),
            Self::CreateProjectGrants => write!(f, "CreateProjectGrants"),
            Self::DeleteProjectGrants => write!(f, "DeleteProjectGrants"),
            Self::ViewApiDefinition => write!(f, "ViewApiDefinition"),
            Self::CreateApiDefinition => write!(f, "CreateApiDefinition"),
            Self::UpdateApiDefinition => write!(f, "UpdateApiDefinition"),
            Self::DeleteApiDefinition => write!(f, "DeleteApiDefinition"),
            Self::DeleteProject => write!(f, "DeleteProject"),
            Self::ViewPluginInstallations => write!(f, "ViewPluginInstallations"),
            Self::CreatePluginInstallation => write!(f, "CreatePluginInstallation"),
            Self::UpdatePluginInstallation => write!(f, "UpdatePluginInstallation"),
            Self::DeletePluginInstallation => write!(f, "DeletePluginInstallation"),
            Self::UpsertApiDeployment => write!(f, "UpsertApiDeployment"),
            Self::ViewApiDeployment => write!(f, "ViewApiDeployment"),
            Self::DeleteApiDeployment => write!(f, "DeleteApiDeployment"),
            Self::UpsertApiDomain => write!(f, "UpsertApiDomain"),
            Self::ViewApiDomain => write!(f, "ViewApiDomain"),
            Self::DeleteApiDomain => write!(f, "DeleteApiDomain"),
            Self::ViewProject => write!(f, "ViewProject"),
            Self::BatchUpdatePluginInstallations => write!(f, "BatchUpdatePluginInstallations"),
            Self::ViewPluginDefinition => write!(f, "ViewPluginDefinition"),
            Self::CreatePluginDefinition => write!(f, "CreatePluginDefinition"),
            Self::UpdatePluginDefinition => write!(f, "UpdatePluginDefinition"),
            Self::DeletePluginDefinition => write!(f, "DeletePluginDefinition"),
            Self::ExportApiDefinition => write!(f, "ExportApiDefinition"),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
    EnumIter,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
pub enum ProjectPermission {
    ViewComponent,
    CreateComponent,
    UpdateComponent,
    DeleteComponent,
    ViewWorker,
    CreateWorker,
    UpdateWorker,
    DeleteWorker,
    ViewProjectGrants,
    CreateProjectGrants,
    DeleteProjectGrants,
    ViewApiDefinition,
    CreateApiDefinition,
    UpdateApiDefinition,
    DeleteApiDefinition,
    DeleteProject,
    ViewPluginInstallations,
    CreatePluginInstallation,
    UpdatePluginInstallation,
    DeletePluginInstallation,
    UpsertApiDeployment,
    ViewApiDeployment,
    DeleteApiDeployment,
    UpsertApiDomain,
    ViewApiDomain,
    DeleteApiDomain,
    ViewPluginDefinition,
    CreatePluginDefinition,
    UpdatePluginDefinition,
    DeletePluginDefinition,
    ExportApiDefinition,
}

impl TryFrom<ProjectAction> for ProjectPermission {
    type Error = String;

    fn try_from(value: ProjectAction) -> Result<Self, Self::Error> {
        match value {
            ProjectAction::ViewComponent => Ok(Self::ViewComponent),
            ProjectAction::CreateComponent => Ok(Self::CreateComponent),
            ProjectAction::UpdateComponent => Ok(Self::UpdateComponent),
            ProjectAction::DeleteComponent => Ok(Self::DeleteComponent),
            ProjectAction::ViewWorker => Ok(Self::ViewWorker),
            ProjectAction::CreateWorker => Ok(Self::CreateWorker),
            ProjectAction::UpdateWorker => Ok(Self::UpdateWorker),
            ProjectAction::DeleteWorker => Ok(Self::DeleteWorker),
            ProjectAction::ViewProjectGrants => Ok(Self::ViewProjectGrants),
            ProjectAction::CreateProjectGrants => Ok(Self::CreateProjectGrants),
            ProjectAction::DeleteProjectGrants => Ok(Self::DeleteProjectGrants),
            ProjectAction::ViewApiDefinition => Ok(Self::ViewApiDefinition),
            ProjectAction::CreateApiDefinition => Ok(Self::CreateApiDefinition),
            ProjectAction::UpdateApiDefinition => Ok(Self::UpdateApiDefinition),
            ProjectAction::DeleteApiDefinition => Ok(Self::DeleteApiDefinition),
            ProjectAction::DeleteProject => Ok(Self::DeleteProject),
            ProjectAction::ViewPluginInstallations => Ok(Self::ViewPluginInstallations),
            ProjectAction::CreatePluginInstallation => Ok(Self::CreatePluginInstallation),
            ProjectAction::UpdatePluginInstallation => Ok(Self::UpdatePluginInstallation),
            ProjectAction::DeletePluginInstallation => Ok(Self::DeletePluginInstallation),
            ProjectAction::UpsertApiDeployment => Ok(Self::UpsertApiDeployment),
            ProjectAction::ViewApiDeployment => Ok(Self::ViewApiDeployment),
            ProjectAction::DeleteApiDeployment => Ok(Self::DeleteApiDeployment),
            ProjectAction::UpsertApiDomain => Ok(Self::UpsertApiDomain),
            ProjectAction::ViewApiDomain => Ok(Self::ViewApiDomain),
            ProjectAction::DeleteApiDomain => Ok(Self::DeleteApiDomain),
            ProjectAction::ViewPluginDefinition => Ok(Self::ViewPluginDefinition),
            ProjectAction::CreatePluginDefinition => Ok(Self::CreatePluginDefinition),
            ProjectAction::UpdatePluginDefinition => Ok(Self::UpdatePluginDefinition),
            ProjectAction::DeletePluginDefinition => Ok(Self::DeletePluginDefinition),
            ProjectAction::ViewProject | ProjectAction::BatchUpdatePluginInstallations => {
                Err(format!("Unknown project permission: {value:?}"))
            }
            ProjectAction::ExportApiDefinition => Ok(Self::ExportApiDefinition),
        }
    }
}

impl From<ProjectPermission> for i32 {
    fn from(value: ProjectPermission) -> Self {
        value as i32
    }
}

impl Display for ProjectPermission {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ViewComponent => write!(f, "ViewComponent"),
            Self::CreateComponent => write!(f, "CreateComponent"),
            Self::UpdateComponent => write!(f, "UpdateComponent"),
            Self::DeleteComponent => write!(f, "DeleteComponent"),
            Self::ViewWorker => write!(f, "ViewWorker"),
            Self::CreateWorker => write!(f, "CreateWorker"),
            Self::UpdateWorker => write!(f, "UpdateWorker"),
            Self::DeleteWorker => write!(f, "DeleteWorker"),
            Self::ViewProjectGrants => write!(f, "ViewProjectGrants"),
            Self::CreateProjectGrants => write!(f, "CreateProjectGrants"),
            Self::DeleteProjectGrants => write!(f, "DeleteProjectGrants"),
            Self::ViewApiDefinition => write!(f, "ViewApiDefinition"),
            Self::CreateApiDefinition => write!(f, "CreateApiDefinition"),
            Self::UpdateApiDefinition => write!(f, "UpdateApiDefinition"),
            Self::DeleteApiDefinition => write!(f, "DeleteApiDefinition"),
            Self::DeleteProject => write!(f, "DeleteProject"),
            Self::ViewPluginInstallations => write!(f, "ViewPluginInstallations"),
            Self::CreatePluginInstallation => write!(f, "CreatePluginInstallation"),
            Self::UpdatePluginInstallation => write!(f, "UpdatePluginInstallation"),
            Self::DeletePluginInstallation => write!(f, "DeletePluginInstallation"),
            Self::UpsertApiDeployment => write!(f, "UpsertApiDeployment"),
            Self::ViewApiDeployment => write!(f, "ViewApiDeployment"),
            Self::DeleteApiDeployment => write!(f, "DeleteApiDeployment"),
            Self::UpsertApiDomain => write!(f, "UpsertApiDomain"),
            Self::ViewApiDomain => write!(f, "ViewApiDomain"),
            Self::DeleteApiDomain => write!(f, "DeleteApiDomain"),
            Self::ViewPluginDefinition => write!(f, "ViewPluginDefinition"),
            Self::CreatePluginDefinition => write!(f, "CreatePluginDefinition"),
            Self::UpdatePluginDefinition => write!(f, "UpdatePluginDefinition"),
            Self::DeletePluginDefinition => write!(f, "DeletePluginDefinition"),
            Self::ExportApiDefinition => write!(f, "ExportApiDefinition"),
        }
    }
}

impl FromStr for ProjectPermission {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ViewComponent" => Ok(Self::ViewComponent),
            "CreateComponent" => Ok(Self::CreateComponent),
            "UpdateComponent" => Ok(Self::UpdateComponent),
            "DeleteComponent" => Ok(Self::DeleteComponent),
            "ViewWorker" => Ok(Self::ViewWorker),
            "CreateWorker" => Ok(Self::CreateWorker),
            "UpdateWorker" => Ok(Self::UpdateWorker),
            "DeleteWorker" => Ok(Self::DeleteWorker),
            "ViewProjectGrants" => Ok(Self::ViewProjectGrants),
            "CreateProjectGrants" => Ok(Self::CreateProjectGrants),
            "DeleteProjectGrants" => Ok(Self::DeleteProjectGrants),
            "ViewApiDefinition" => Ok(Self::ViewApiDefinition),
            "CreateApiDefinition" => Ok(Self::CreateApiDefinition),
            "UpdateApiDefinition" => Ok(Self::UpdateApiDefinition),
            "DeleteApiDefinition" => Ok(Self::DeleteApiDefinition),
            "DeleteProject" => Ok(Self::DeleteProject),
            "ViewPluginInstallations" => Ok(Self::ViewPluginInstallations),
            "CreatePluginInstallation" => Ok(Self::CreatePluginInstallation),
            "UpdatePluginInstallation" => Ok(Self::UpdatePluginInstallation),
            "DeletePluginInstallation" => Ok(Self::DeletePluginInstallation),
            "UpsertApiDeployment" => Ok(Self::UpsertApiDeployment),
            "ViewApiDeployment" => Ok(Self::ViewApiDeployment),
            "DeleteApiDeployment" => Ok(Self::DeleteApiDeployment),
            "UpsertApiDomain" => Ok(Self::UpsertApiDomain),
            "ViewApiDomain" => Ok(Self::ViewApiDomain),
            "DeleteApiDomain" => Ok(Self::DeleteApiDomain),
            "ViewPluginDefinition" => Ok(Self::ViewPluginDefinition),
            "CreatePluginDefinition" => Ok(Self::CreatePluginDefinition),
            "UpdatePluginDefinition" => Ok(Self::UpdatePluginDefinition),
            "DeletePluginDefinition" => Ok(Self::DeletePluginDefinition),
            "ExportApiDefinition" => Ok(Self::ExportApiDefinition),
            _ => Err(format!("Unknown project permission: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
pub struct ProjectActions {
    pub actions: HashSet<ProjectPermission>,
}

impl ProjectActions {
    pub fn empty() -> ProjectActions {
        ProjectActions {
            actions: HashSet::new(),
        }
    }

    pub fn all() -> ProjectActions {
        let actions: HashSet<ProjectPermission> =
            ProjectPermission::iter().collect::<HashSet<ProjectPermission>>();
        ProjectActions { actions }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ProjectAuthorisedActions {
    pub project_id: ProjectId,
    pub owner_account_id: AccountId,
    pub actions: ProjectActions,
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::AccountAction;

    use super::TokenSecret;

    impl TryFrom<golem_api_grpc::proto::golem::token::TokenSecret> for TokenSecret {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::token::TokenSecret,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                value: value.value.ok_or("Missing field: value")?.into(),
            })
        }
    }

    impl From<TokenSecret> for golem_api_grpc::proto::golem::token::TokenSecret {
        fn from(value: TokenSecret) -> Self {
            Self {
                value: Some(value.value.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::AccountAction> for AccountAction {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AccountAction,
        ) -> Result<Self, Self::Error> {
            Self::try_from(value as i32)
        }
    }

    impl From<AccountAction> for golem_api_grpc::proto::golem::auth::AccountAction {
        fn from(value: AccountAction) -> Self {
            Self::try_from(value as i32).expect("Encoding AccountAction as protobuf")
        }
    }
}

#[cfg(test)]
mod test {
    use super::Role;
    use super::{ProjectAction, ProjectPermission};
    use std::str::FromStr;
    use strum::IntoEnumIterator;
    use test_r::test;

    #[test]
    fn role_to_from() {
        for role in Role::iter() {
            let role_as_i32: i32 = role.clone().into();
            let deserialized_role = Role::try_from(role_as_i32).unwrap();
            assert_eq!(role, deserialized_role);

            let role_as_str = role.to_string();
            let deserialized_role = Role::from_str(&role_as_str).unwrap();
            assert_eq!(role, deserialized_role);
            assert_eq!(role, deserialized_role);
        }
    }

    #[test]
    fn project_action_to_from() {
        for action in ProjectAction::iter() {
            let action_as_i32: i32 = action.clone().into();
            let deserialized_action = ProjectAction::try_from(action_as_i32).unwrap();
            assert_eq!(action, deserialized_action);
        }
    }

    #[test]
    fn project_permission_to_from() {
        for permission in ProjectPermission::iter() {
            let permission_as_str = permission.to_string();
            let deserialized_permission = ProjectPermission::from_str(&permission_as_str).unwrap();
            assert_eq!(permission, deserialized_permission);
        }
    }
}
