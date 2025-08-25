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

    pub struct AccountAuthorisation {
        pub token: Token,
        pub roles: Vec<AccountRole>,
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
    #[derive(Eq, Hash, FromRepr, EnumIter)]
    #[repr(i32)]
    pub enum AccountRole {
        Admin = 0,
        MarketingAdmin = 1,
    }

    #[derive(Eq, Hash, FromRepr, EnumIter)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum GlobalAction {
    CreateAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum AccountAction {
    UpdateAccount,
    SetRoles,
    CreateApplication,
    CreateToken,
    CreateKnownSecret,
    DeleteToken,
    ViewAccount,
    UpdateUsage
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum EnvironmentAction {
    CreateComponent,
    UpdateComponent,
    ViewComponent,
    ViewShares,
    UpdateShare,
    CreateShare,
    DeleteShare,
    ViewEnvironment
}

// #[cfg(feature = "protobuf")]
// mod protobuf {
//     use super::AccountAction;

//     impl TryFrom<golem_api_grpc::proto::golem::auth::AccountAction> for AccountAction {
//         type Error = String;

//         fn try_from(
//             value: golem_api_grpc::proto::golem::auth::AccountAction,
//         ) -> Result<Self, Self::Error> {
//             Self::try_from(value as i32)
//         }
//     }

//     impl From<AccountAction> for golem_api_grpc::proto::golem::auth::AccountAction {
//         fn from(value: AccountAction) -> Self {
//             Self::try_from(value as i32).expect("Encoding AccountAction as protobuf")
//         }
//     }
// }
