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

newtype_uuid!(TokenId);
newtype_uuid!(TokenSecret);

declare_structs! {
    pub struct Token {
        pub id: TokenId,
        pub account_id: AccountId,
        pub created_at: chrono::DateTime<Utc>,
        pub expires_at: chrono::DateTime<Utc>,
    }

    pub struct TokenCreation {
        pub expires_at: chrono::DateTime<Utc>,
    }

    pub struct TokenWithSecret {
        pub id: TokenId,
        pub secret: TokenSecret,
        pub account_id: AccountId,
        pub created_at: chrono::DateTime<Utc>,
        pub expires_at: chrono::DateTime<Utc>,
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
    #[derive(Hash, FromRepr, EnumIter)]
    #[repr(i32)]
    pub enum AccountRole {
        Admin = 0,
        MarketingAdmin = 1,
    }

    #[derive(Hash, FromRepr, EnumIter)]
    #[repr(i32)]
    pub enum EnvironmentRole {
        Admin = 0,
        Viewer = 1,
        Deployer = 2,
    }
}

mod protobuf {
    use super::{AccountRole, EnvironmentRole};

    impl TryFrom<golem_api_grpc::proto::golem::auth::AccountRole> for AccountRole {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AccountRole,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::auth::AccountRole as GrpcAccountRole;
            match value {
                GrpcAccountRole::Admin => Ok(Self::Admin),
                GrpcAccountRole::MarketingAdmin => Ok(Self::MarketingAdmin),
                GrpcAccountRole::Unspecified => Err("unknown account role".to_string()),
            }
        }
    }

    impl From<AccountRole> for golem_api_grpc::proto::golem::auth::AccountRole {
        fn from(value: AccountRole) -> Self {
            match value {
                AccountRole::Admin => Self::Admin,
                AccountRole::MarketingAdmin => Self::MarketingAdmin,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::EnvironmentRole> for EnvironmentRole {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::EnvironmentRole,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::auth::EnvironmentRole as GrpcEnvironmentRole;

            match value {
                GrpcEnvironmentRole::Admin => Ok(Self::Admin),
                GrpcEnvironmentRole::Viewer => Ok(Self::Viewer),
                GrpcEnvironmentRole::Deployer => Ok(Self::Deployer),
                GrpcEnvironmentRole::Unspecified => Err("unknown environment role".to_string()),
            }
        }
    }

    impl From<EnvironmentRole> for golem_api_grpc::proto::golem::auth::EnvironmentRole {
        fn from(value: EnvironmentRole) -> Self {
            match value {
                EnvironmentRole::Admin => Self::Admin,
                EnvironmentRole::Viewer => Self::Viewer,
                EnvironmentRole::Deployer => Self::Deployer,
            }
        }
    }
}
