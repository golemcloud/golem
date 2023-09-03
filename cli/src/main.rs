extern crate derive_more;

use std::fmt::Debug;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use uuid::Uuid;
use clap_verbosity_flag::Verbosity;
use derive_more::{Display, FromStr};
use golem_client::component::ComponentLive;
use golem_client::grant::GrantLive;
use golem_client::project::ProjectLive;
use golem_client::project_grant::ProjectGrantLive;
use golem_client::project_policy::ProjectPolicyLive;
use tokio;
use serde::{Serialize, Deserialize};
use model::*;
use golem_examples::model::{
    ExampleName, GuestLanguage, GuestLanguageTier, PackageName, TemplateName,
};
use reqwest::Url;
use crate::account::{AccountHandler, AccountHandlerLive, AccountSubcommand};
use crate::auth::{Auth, AuthLive};
use crate::clients::account::AccountClientLive;
use crate::clients::component::ComponentClientLive;
use crate::clients::grant::GrantClientLive;
use crate::clients::login::LoginClientLive;
use crate::clients::policy::ProjectPolicyClientLive;
use crate::clients::project::ProjectClientLive;
use crate::clients::project_grant::ProjectGrantClientLive;
use crate::clients::token::TokenClientLive;
use crate::component::{ComponentHandler, ComponentHandlerLive, ComponentSubcommand};
use crate::policy::{ProjectPolicyHandler, ProjectPolicyHandlerLive, ProjectPolicySubcommand};
use crate::project::{ProjectHandler, ProjectHandlerLive, ProjectSubcommand};
use crate::project_grant::{ProjectGrantHandler, ProjectGrantHandlerLive};
use crate::token::{TokenHandler, TokenHandlerLive, TokenSubcommand};

pub mod model;
mod examples;
mod auth;
pub mod clients;
mod account;
mod token;
mod component;
mod project;
mod policy;
mod project_grant;

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct InstanceName(String); // TODO: Validate

fn parse_key_val(s: &str) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

#[derive(Subcommand, Debug)]
#[command()]
enum Command {
    #[command()]
    Deploy {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        component_name: ComponentName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long)]
        parameters: String, // TODO: validate json

        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBuf, // TODO: validate exists,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubcommand
    },
    #[command()]
    Instance {
        #[command(subcommand)]
        subcommand: InstanceSubcommand
    },
    #[command()]
    Account {
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: AccountSubcommand,
    },
    #[command()]
    Token {
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: TokenSubcommand,
    },
    #[command()]
    Project {
        #[command(subcommand)]
        subcommand: ProjectSubcommand,
    },
    #[command()]
    Share {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(long)]
        recipient_account_id: AccountId,

        #[arg(long, required = true, conflicts_with = "project_actions")]
        project_policy_id: Option<ProjectPolicyId>,

        #[arg(short = 'A', long, required = true, conflicts_with = "project_policy_id")]
        project_actions: Option<Vec<ProjectAction>>,
    },
    #[command()]
    ProjectPolicy {
        #[command(subcommand)]
        subcommand: ProjectPolicySubcommand
    },
    #[command()]
    New {
        #[arg(short, long)]
        example: ExampleName,

        #[arg(short, long)]
        template_name: TemplateName,

        #[arg(short, long)]
        package_name: Option<PackageName>,
    },
    #[command()]
    ListTemplates {
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        #[arg(short, long)]
        language: Option<GuestLanguage>,
    },
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct InvocationKey(String); // TODO: Validate

#[derive(Subcommand, Debug)]
#[command()]
enum InstanceSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    InvocationKey {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    InvokeAndAwait {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short = 'k', long)]
        invocation_key: Option<InvocationKey>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long)]
        parameters: String, // TODO: validate json

        #[arg(short = 's', long, default_value_t = false)]
        use_stdio: bool,
    },
    #[command()]
    Invoke {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long)]
        parameters: String, // TODO: validate json
    },
    #[command()]
    Connect {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Interrupt {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    SimulatedCrash {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Get {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, rename_all = "kebab-case")]
struct GolemCommand {
    #[arg(short = 'D', long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    config_directory: Option<PathBuf>,

    #[arg(short = 'T', long)]
    auth_token: Option<Uuid>, // TODO: uuid

    #[command(flatten)]
    verbosity: Verbosity,

    #[arg(short = 'F', long, default_value = "yaml")]
    format: Format,

    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = GolemCommand::parse();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(command))
}

#[derive(Debug, Serialize, Deserialize)]
struct DeployResult {
    msg: String,
}

async fn process_deploy(cmd: &Command) -> Result<GolemResult, GolemError> {
    Ok(GolemResult::Ok(Box::new(DeployResult { msg: format!("{:?}", cmd) })))
}

async fn async_main(cmd: GolemCommand) -> Result<(), Box<dyn std::error::Error>> {
    let url = Url::parse("https://release.dev-api.golem.cloud/").unwrap();
    let home = std::env::var("HOME").unwrap();
    let default_conf_dir = PathBuf::from(format!("{home}/.golem"));

    let login = LoginClientLive { login: golem_client::login::LoginLive { base_url: url.clone() } };
    let auth_srv = AuthLive { login };
    let account_client = AccountClientLive { account: golem_client::account::AccountLive { base_url: url.clone() } };
    let grant_client = GrantClientLive { client: GrantLive { base_url: url.clone() } };
    let acc_srv = AccountHandlerLive { client: account_client, grant: grant_client };
    let token_client = TokenClientLive { client: golem_client::token::TokenLive { base_url: url.clone() } };
    let token_srv = TokenHandlerLive { client: token_client };
    let project_client = ProjectClientLive { client: ProjectLive { base_url: url.clone() } };
    let project_srv = ProjectHandlerLive { client: &project_client };
    let component_client = ComponentClientLive { client: ComponentLive { base_url: url.clone() } };
    let component_srv = ComponentHandlerLive { client: component_client, projects: &project_client };
    let project_policy_client = ProjectPolicyClientLive { client: ProjectPolicyLive { base_url: url.clone() } };
    let project_policy_srv = ProjectPolicyHandlerLive { client: project_policy_client };
    let project_grant_client = ProjectGrantClientLive { client: ProjectGrantLive { base_url: url.clone() } };
    let project_grant_srv = ProjectGrantHandlerLive { client: project_grant_client, project: &project_client };

    let auth = auth_srv.authenticate(cmd.auth_token.clone(), cmd.config_directory.clone().unwrap_or(default_conf_dir)).await?;

    let res = match cmd.command {
        c @ Command::Deploy { .. } => process_deploy(&c).await,
        Command::Component { subcommand } => component_srv.handle(&auth, subcommand).await,
        Command::Instance { .. } => todo!(),
        Command::Account { account_id, subcommand } => acc_srv.handle(&auth, account_id, subcommand).await,
        Command::Token { account_id, subcommand } => token_srv.handle(&auth, account_id, subcommand).await,
        Command::Project { subcommand } => project_srv.handle(&auth, subcommand).await,
        Command::Share { project_ref, recipient_account_id, project_policy_id, project_actions } =>
            project_grant_srv.handle(&auth, project_ref, recipient_account_id, project_policy_id, project_actions).await,
        Command::ProjectPolicy { subcommand } => project_policy_srv.handle(&auth, subcommand).await,
        Command::New { example, package_name, template_name } =>
            examples::process_new(example, template_name, package_name),
        Command::ListTemplates { min_tier, language } =>
            examples::process_list_templates(min_tier, language),
    };

    match res {
        Ok(res) => match res {
            GolemResult::Ok(r) => {
                r.println(&cmd.format);

                Ok(())
            }
            GolemResult::Str(s) => {
                println!("{s}");

                Ok(())
            }
        }
        Err(err) => Err(Box::new(err)),
    }
}