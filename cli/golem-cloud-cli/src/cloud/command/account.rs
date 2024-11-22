use clap::Subcommand;
use golem_cli::cloud::AccountId;

use crate::cloud::service::account::AccountService;
use crate::cloud::service::grant::GrantService;
use golem_cli::model::{GolemError, GolemResult};
use golem_cloud_client::model::Role;

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
    #[command(alias = "create")]
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
