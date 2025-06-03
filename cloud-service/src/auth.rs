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

use crate::model::Token;
use cloud_common::model::{CloudPluginOwner, Role};
use golem_common::model::AccountId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AccountAuthorisation {
    pub token: Token,
    pub roles: Vec<Role>,
}

impl AccountAuthorisation {
    pub fn new(token: Token, roles: Vec<Role>) -> Self {
        Self { token, roles }
    }

    pub fn admin() -> Self {
        AccountAuthorisation::new(Token::admin(), vec![Role::Admin])
    }

    pub fn has_account(&self, account_id: &AccountId) -> bool {
        self.token.account_id == *account_id
    }

    pub fn has_role(&self, role: &Role) -> bool {
        self.roles.contains(role)
    }

    pub fn has_admin(&self) -> bool {
        self.has_role(&Role::Admin)
    }

    pub fn has_account_and_role(&self, account_id: &AccountId, role: &Role) -> bool {
        self.token.account_id == *account_id && self.roles.contains(role)
    }

    pub fn has_account_or_role(&self, account_id: &AccountId, role: &Role) -> bool {
        self.token.account_id == *account_id || self.roles.contains(role)
    }

    pub fn as_plugin_owner(&self) -> CloudPluginOwner {
        CloudPluginOwner {
            account_id: self.token.account_id.clone(),
        }
    }

    #[cfg(test)]
    pub fn new_test(account_id: &AccountId, roles: Vec<Role>) -> AccountAuthorisation {
        use cloud_common::model::TokenId;
        AccountAuthorisation {
            token: Token {
                id: TokenId::new_v4(),
                account_id: account_id.clone(),
                created_at: chrono::Utc::now(),
                expires_at: chrono::Utc::now() + chrono::Duration::days(1),
            },
            roles,
        }
    }
}
