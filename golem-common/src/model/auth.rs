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
use crate::{declare_enums, declare_structs, newtype_uuid};
use chrono::Utc;
use strum_macros::{EnumIter, FromRepr};

newtype_uuid!(TokenId, golem_api_grpc::proto::golem::token::TokenId);
newtype_uuid!(
    TokenSecret,
    golem_api_grpc::proto::golem::token::TokenSecret
);

declare_structs! {
    pub struct Token {
        pub id: TokenId,
        pub account_id: AccountId,
        pub created_at: chrono::DateTime<Utc>,
        pub expires_at: chrono::DateTime<Utc>,
    }

    pub struct TokenWithSecret {
        pub id: TokenId,
        pub secret: TokenSecret,
        pub account_id: AccountId,
        pub created_at: chrono::DateTime<Utc>,
        pub expires_at: chrono::DateTime<Utc>,
    }

    pub struct AuthCtx {
        pub token_secret: TokenSecret,
    }

    pub struct AccountAuthorisation {
        pub token: Token,
        pub roles: Vec<AccountRole>,
    }
}

impl AuthCtx {
    pub fn new(token_secret: TokenSecret) -> Self {
        Self { token_secret }
    }
}

impl TokenWithSecret {
    pub fn without_secret(self) -> Token {
        Token {
            id: self.id,
            account_id: self.account_id,
            created_at: self.created_at,
            expires_at: self.expires_at,
        }
    }
}

declare_enums! {
    #[derive(FromRepr, EnumIter)]
    #[repr(i32)]
    pub enum AccountRole {
        Admin = 0,
        MarketingAdmin = 1,
    }

    #[derive(FromRepr, EnumIter)]
    #[repr(i32)]
    pub enum EnvironmentRole {
        Admin = 0,
        Viewer = 1,
        Deployer = 2,
    }
}

impl From<AccountRole> for i32 {
    fn from(value: AccountRole) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for AccountRole {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        AccountRole::from_repr(value).ok_or_else(|| format!("Invalid role: {value}"))
    }
}

impl From<EnvironmentRole> for i32 {
    fn from(value: EnvironmentRole) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for EnvironmentRole {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        EnvironmentRole::from_repr(value).ok_or_else(|| format!("Invalid role: {value}"))
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, EnumIter, FromRepr)]
#[repr(i32)]
pub enum EnvironmentAction {
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

impl From<EnvironmentAction> for i32 {
    fn from(value: EnvironmentAction) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for EnvironmentAction {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        EnvironmentAction::from_repr(value)
            .ok_or_else(|| format!("Invalid project action: {value}"))
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::AccountAction;

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
    use super::AccountRole;
    use super::EnvironmentAction;
    use strum::IntoEnumIterator;
    use test_r::test;

    #[test]
    fn role_to_from() {
        for role in AccountRole::iter() {
            let role_as_i32: i32 = role.clone().into();
            let deserialized_role = AccountRole::try_from(role_as_i32).unwrap();
            assert_eq!(role, deserialized_role);
        }
    }

    #[test]
    fn project_action_to_from() {
        for action in EnvironmentAction::iter() {
            let action_as_i32: i32 = action.clone().into();
            let deserialized_action = EnvironmentAction::try_from(action_as_i32).unwrap();
            assert_eq!(action, deserialized_action);
        }
    }
}
