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

use crate::{declare_enums, declare_structs, declare_transparent_newtypes, newtype_uuid};
use anyhow::anyhow;
use chrono::Utc;
use std::fmt::Display;
use std::str::FromStr;

newtype_uuid!(OAuth2WebflowStateId);

declare_transparent_newtypes! {
    pub struct EncodedOAuth2DeviceflowSession(pub String);
}

declare_structs! {
    pub struct OAuth2DeviceflowStart {
        pub provider: OAuth2Provider,
    }

    pub struct OAuth2DeviceflowData {
        pub url: String,
        pub user_code: String,
        pub expires: chrono::DateTime<Utc>,
        pub encoded_session: EncodedOAuth2DeviceflowSession,
    }

    pub struct OAuth2WebflowData {
        pub url: String,
        pub state: OAuth2WebflowStateId,
    }
}

declare_enums! {
    pub enum OAuth2Provider {
        Github,
    }
}

impl Display for OAuth2Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuth2Provider::Github => write!(f, "github"),
        }
    }
}

impl FromStr for OAuth2Provider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(OAuth2Provider::Github),
            _ => Err(anyhow!("Invalid OAuth2Provider: {s}")),
        }
    }
}
