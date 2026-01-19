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

use crate::base_model::account::AccountId;
use crate::{declare_enums, declare_structs, newtype_uuid};
use chrono::Utc;

use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::str::FromStr;
use strum_macros::{EnumIter, FromRepr};

#[derive(Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String")]
#[repr(transparent)]
pub struct TokenSecret(pub(crate) String);

impl TokenSecret {
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
