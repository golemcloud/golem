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

use clap::Subcommand;

use crate::cloud::model::{AccountId, Role};
use crate::cloud::service::account::AccountService;
use crate::cloud::service::grant::GrantService;
use crate::model::{GolemError, GolemResult};

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

impl AccountSubcommand {
    pub async fn handle(
        self,
        account_id: Option<AccountId>,
        service: &(dyn AccountService + Send + Sync),
        grant: &(dyn GrantService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            AccountSubcommand::Get {} => service.get(account_id).await,
            AccountSubcommand::Update {
                account_name,
                account_email,
            } => {
                service
                    .update(account_name, account_email, account_id)
                    .await
            }
            AccountSubcommand::Add {
                account_name,
                account_email,
            } => service.add(account_name, account_email).await,
            AccountSubcommand::Delete {} => service.delete(account_id).await,
            AccountSubcommand::Grant { subcommand } => match subcommand {
                GrantSubcommand::Get {} => grant.get(account_id).await,
                GrantSubcommand::Add { role } => grant.add(role, account_id).await,
                GrantSubcommand::Delete { role } => grant.delete(role, account_id).await,
            },
        }
    }
}
