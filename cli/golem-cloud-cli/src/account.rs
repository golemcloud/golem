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

use async_trait::async_trait;
use clap::Subcommand;
use golem_cloud_client::model::AccountData;

use crate::clients::account::AccountClient;
use crate::clients::grant::GrantClient;
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError, GolemResult, Role};

#[derive(Subcommand, Debug)]
#[command()]
pub enum AccountSubcommand {
    /// Get information about the account
    #[command()]
    Get {},

    /// Update some information about the account
    #[command()]
    Update {
        /// Set the account's name
        // TODO: validate non-empty
        #[arg(short = 'n', long)]
        account_name: Option<String>,

        /// Set the account's email address
        #[arg(short = 'e', long)]
        account_email: Option<String>,
    },

    /// Add a new account
    #[command()]
    Add {
        /// The new account's name
        #[arg(short = 'n', long)]
        account_name: String,

        /// The new account's email address
        #[arg(short = 'e', long)]
        account_email: String,
    },

    /// Delete the account
    #[command()]
    Delete {},

    /// Manage the account's roles
    #[command()]
    Grant {
        #[command(subcommand)]
        subcommand: GrantSubcommand,
    },
}

#[derive(Subcommand, Debug)]
#[command()]
pub enum GrantSubcommand {
    /// Get the roles granted to the account
    #[command()]
    Get {},

    /// Grant a new role to the account
    #[command()]
    Add {
        #[arg(value_name = "ROLE")]
        role: Role,
    },

    /// Remove a role from the account
    #[command()]
    Delete {
        #[arg(value_name = "ROLE")]
        role: Role,
    },
}

#[async_trait]
pub trait AccountHandler {
    async fn handle(
        &self,
        token: &CloudAuthentication,
        account_id: Option<AccountId>,
        subcommand: AccountSubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct AccountHandlerLive<C: AccountClient + Sync + Send, G: GrantClient + Sync + Send> {
    pub client: C,
    pub grant: G,
}

#[async_trait]
impl<C: AccountClient + Sync + Send, G: GrantClient + Sync + Send> AccountHandler
    for AccountHandlerLive<C, G>
{
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        account_id: Option<AccountId>,
        subcommand: AccountSubcommand,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.unwrap_or(auth.account_id());

        match subcommand {
            AccountSubcommand::Get {} => {
                let account = self.client.get(&account_id).await?;
                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Update {
                account_name,
                account_email,
            } => {
                let existing = self.client.get(&account_id).await?;
                let name = account_name.unwrap_or(existing.name);
                let email = account_email.unwrap_or(existing.email);
                let updated = AccountData { name, email };
                let account = self.client.put(&account_id, updated).await?;
                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Add {
                account_name,
                account_email,
            } => {
                let data = AccountData {
                    name: account_name,
                    email: account_email,
                };

                let account = self.client.post(data).await?;

                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Delete {} => {
                self.client.delete(&account_id).await?;
                Ok(GolemResult::Str("Deleted".to_string()))
            }
            AccountSubcommand::Grant { subcommand } => match subcommand {
                GrantSubcommand::Get {} => {
                    let roles = self.grant.get_all(account_id).await?;

                    Ok(GolemResult::Ok(Box::new(roles)))
                }
                GrantSubcommand::Add { role } => {
                    self.grant.put(account_id, role).await?;

                    Ok(GolemResult::Ok(Box::new("RoleGranted".to_string())))
                }
                GrantSubcommand::Delete { role } => {
                    self.grant.put(account_id, role).await?;

                    Ok(GolemResult::Ok(Box::new("RoleRemoved".to_string())))
                }
            },
        }
    }
}
