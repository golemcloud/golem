use golem_common::model::AccountId;

use crate::model::Token;
use cloud_common::model::{CloudPluginOwner, Role};

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
