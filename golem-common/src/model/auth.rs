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
use base64::Engine;
use chrono::Utc;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{
    ParseError, ParseFromJSON, ParseFromParameter, ParseResult, ToJSON, Type,
};
use rand::rngs::OsRng;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Debug;
use std::str::FromStr;
use strum_macros::{EnumIter, FromRepr};

#[derive(Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String")]
#[repr(transparent)]
pub struct TokenSecret(String);
impl Default for TokenSecret {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenSecret {
    pub fn new() -> Self {
        let mut token = [0u8; 32]; // 32 bytes = 256 bits
        OsRng
            .try_fill_bytes(&mut token)
            .expect("Failed to generate random bytes");
        let token_str = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token);
        Self(token_str)
    }

    /// Construct a token secret, skipping all validation
    pub fn trusted(value: String) -> Self {
        Self(value)
    }

    pub fn secret(&self) -> &str {
        &self.0
    }

    pub fn into_secret(self) -> String {
        self.0
    }

    pub fn validate(s: &str) -> Result<(), &'static str> {
        if s.len() < 16 {
            return Err("token must be at least 16 characters");
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("token contains invalid characters");
        }
        Ok(())
    }
}

impl Debug for TokenSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "*******")
    }
}

impl FromStr for TokenSecret {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::validate(s)?;
        Ok(Self(s.to_string()))
    }
}

impl TryFrom<String> for TokenSecret {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        TokenSecret::from_str(&value)
    }
}

impl Type for TokenSecret {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut meta = MetaSchema::new("string");
        meta.min_length = Some(16);
        meta.pattern = Some("^[A-Za-z0-9_-]+$".to_string());

        MetaSchemaRef::Inline(Box::new(meta))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl ParseFromJSON for TokenSecret {
    fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
        let value = value.unwrap_or_default();
        if let serde_json::Value::String(v) = value {
            TokenSecret::from_str(&v).map_err(ParseError::custom)
        } else {
            Err(ParseError::expected_type(value))
        }
    }
}

impl ToJSON for TokenSecret {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.0.clone()))
    }
}

newtype_uuid!(TokenId);

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
    #[derive(Hash, FromRepr, EnumIter, PartialOrd, Ord)]
    #[repr(i32)]
    pub enum AccountRole {
        Admin = 0,
        MarketingAdmin = 1,
    }

    #[derive(Hash, FromRepr, EnumIter, PartialOrd, Ord)]
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
