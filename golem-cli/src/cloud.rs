// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use chrono::{DateTime, Utc};
use derive_more::{Display, FromStr, Into};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAuthenticationConfig {
    data: CloudAuthenticationConfigData,
    secret: AuthSecret,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthSecret(pub Uuid);

impl Debug for AuthSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AuthSecret").field(&"*******").finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAuthenticationConfigData {
    id: Uuid,
    account_id: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Serialize, Deserialize)]
pub struct AccountId {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Into, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

impl Display for ProjectId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
