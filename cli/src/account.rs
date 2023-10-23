use async_trait::async_trait;
use clap::Subcommand;
use golem_client::model::AccountData;

use crate::clients::account::AccountClient;
use crate::clients::grant::GrantClient;
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError, GolemResult, Role};

#[derive(Subcommand, Debug)]
#[command()]
pub enum AccountSubcommand {
    #[command()]
    Get {},

    #[command()]
    Update {
        // TODO: validate non-empty
        #[arg(short = 'n', long)]
        account_name: Option<String>,

        #[arg(short = 'e', long)]
        account_email: Option<String>,
    },

    #[command()]
    Add {
        #[arg(short = 'n', long)]
        account_name: String,

        #[arg(short = 'e', long)]
        account_email: String,
    },

    #[command()]
    Delete {},

    #[command()]
    Grant {
        #[command(subcommand)]
        subcommand: GrantSubcommand,
    },
}

#[derive(Subcommand, Debug)]
#[command()]
pub enum GrantSubcommand {
    #[command()]
    Get {},

    #[command()]
    Add {
        #[arg(value_name = "ROLE")]
        role: Role,
    },

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
                let account = self.client.get(&account_id, auth).await?;
                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Update {
                account_name,
                account_email,
            } => {
                let existing = self.client.get(&account_id, auth).await?;
                let name = account_name.unwrap_or(existing.name);
                let email = account_email.unwrap_or(existing.email);
                let updated = AccountData { name, email };
                let account = self.client.put(&account_id, updated, auth).await?;
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

                let account = self.client.post(data, auth).await?;

                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Delete {} => {
                self.client.delete(&account_id, auth).await?;
                Ok(GolemResult::Str("Deleted".to_string()))
            }
            AccountSubcommand::Grant { subcommand } => match subcommand {
                GrantSubcommand::Get {} => {
                    let roles = self.grant.get_all(account_id, auth).await?;

                    Ok(GolemResult::Ok(Box::new(roles)))
                }
                GrantSubcommand::Add { role } => {
                    self.grant.put(account_id, role, auth).await?;

                    Ok(GolemResult::Ok(Box::new("RoleGranted".to_string())))
                }
                GrantSubcommand::Delete { role } => {
                    self.grant.put(account_id, role, auth).await?;

                    Ok(GolemResult::Ok(Box::new("RoleRemoved".to_string())))
                }
            },
        }
    }
}
