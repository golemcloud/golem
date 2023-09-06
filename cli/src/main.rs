extern crate derive_more;

use clap::builder::ValueParser;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use golem_client::component::ComponentLive;
use golem_client::grant::GrantLive;
use golem_client::instance::InstanceLive;
use golem_client::model::{ComponentInstance, InvokeParameters};
use golem_client::project::ProjectLive;
use golem_client::project_grant::ProjectGrantLive;
use golem_client::project_policy::ProjectPolicyLive;
use std::fmt::Debug;
use std::path::PathBuf;
use uuid::Uuid;

use crate::account::{AccountHandler, AccountHandlerLive, AccountSubcommand};
use crate::auth::{Auth, AuthLive};
use crate::clients::account::AccountClientLive;
use crate::clients::grant::GrantClientLive;
use crate::clients::login::LoginClientLive;
use crate::clients::policy::ProjectPolicyClientLive;
use crate::clients::project::{ProjectClient, ProjectClientLive};
use crate::clients::project_grant::ProjectGrantClientLive;
use crate::clients::template::{TemplateClient, TemplateClientLive, TemplateView};
use crate::clients::token::TokenClientLive;
use crate::clients::worker::{WorkerClient, WorkerClientLive};
use crate::clients::CloudAuthentication;
use crate::model::{JsonValueParser, TemplateName};
use crate::policy::{ProjectPolicyHandler, ProjectPolicyHandlerLive, ProjectPolicySubcommand};
use crate::project::{ProjectHandler, ProjectHandlerLive, ProjectSubcommand};
use crate::project_grant::{ProjectGrantHandler, ProjectGrantHandlerLive};
use crate::template::{TemplateHandler, TemplateHandlerLive, TemplateSubcommand};
use crate::token::{TokenHandler, TokenHandlerLive, TokenSubcommand};
use crate::worker::{WorkerHandler, WorkerHandlerLive, WorkerSubcommand};
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use model::*;
use reqwest::Url;
use serde::Serialize;

mod account;
mod auth;
pub mod clients;
mod examples;
pub mod model;
mod policy;
mod project;
mod project_grant;
mod template;
mod token;
mod worker;

pub fn parse_key_val(
    s: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync + 'static>> {
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
        template_name: TemplateName,

        #[arg(short, long)]
        worker_name: WorkerName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long, value_name = "json", value_parser = ValueParser::new(JsonValueParser))]
        parameters: serde_json::value::Value,

        #[arg(value_name = "template-file", value_hint = clap::ValueHint::FilePath)]
        template_file: PathBuf, // TODO: validate exists,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    Template {
        #[command(subcommand)]
        subcommand: TemplateSubcommand,
    },
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand,
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

        #[arg(
            short = 'A',
            long,
            required = true,
            conflicts_with = "project_policy_id"
        )]
        project_actions: Option<Vec<ProjectAction>>,
    },
    #[command()]
    ProjectPolicy {
        #[command(subcommand)]
        subcommand: ProjectPolicySubcommand,
    },
    #[command()]
    New {
        #[arg(short, long)]
        example: ExampleName,

        #[arg(short, long)]
        template_name: golem_examples::model::TemplateName,

        #[arg(short, long)]
        package_name: Option<PackageName>,
    },
    #[command()]
    ListExamples {
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        #[arg(short, long)]
        language: Option<GuestLanguage>,
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

#[derive(Debug, Serialize)]
struct DeployResult {
    template: TemplateView,
    worker: ComponentInstance,
}

async fn handle_deploy(
    project_client: &impl ProjectClient,
    template_client: &impl TemplateClient,
    worker_client: &impl WorkerClient,
    auth: &CloudAuthentication,
    project_ref: ProjectRef,
    template_name: TemplateName,
    worker_name: WorkerName,
    env: Vec<(String, String)>,
    function: String,
    parameters: serde_json::value::Value,
    template_file: PathBuf,
    args: Vec<String>,
) -> Result<GolemResult, GolemError> {
    let project_id = project_client.resolve_id(project_ref, auth).await?;
    let template = template_client
        .add(project_id, template_name, template_file, auth)
        .await?;
    let template_id = RawTemplateId(
        Uuid::parse_str(&template.template_id)
            .map_err(|e| GolemError(format!("Unexpected error on parsing template id: {e}")))?,
    );
    let worker = worker_client
        .new_worker(worker_name.clone(), template_id.clone(), args, env, auth)
        .await?;
    worker_client
        .invoke(
            worker_name,
            template_id,
            function,
            InvokeParameters { params: parameters },
            auth,
        )
        .await?;

    let res = DeployResult { template, worker };

    Ok(GolemResult::Ok(Box::new(res)))
}

async fn async_main(cmd: GolemCommand) -> Result<(), Box<dyn std::error::Error>> {
    let url_str =
        std::env::var("GOLEM_BASE_URL").unwrap_or("https://release.api.golem.cloud/".to_string());
    let url = Url::parse(&url_str).unwrap();
    let home = std::env::var("HOME").unwrap();
    let allow_insecure_str = std::env::var("GOLEM_ALLOW_INSECURE").unwrap_or("false".to_string());
    let allow_insecure = allow_insecure_str != "false";
    let default_conf_dir = PathBuf::from(format!("{home}/.golem"));

    let login = LoginClientLive {
        login: golem_client::login::LoginLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let auth_srv = AuthLive { login };
    let account_client = AccountClientLive {
        account: golem_client::account::AccountLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let grant_client = GrantClientLive {
        client: GrantLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let acc_srv = AccountHandlerLive {
        client: account_client,
        grant: grant_client,
    };
    let token_client = TokenClientLive {
        client: golem_client::token::TokenLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let token_srv = TokenHandlerLive {
        client: token_client,
    };
    let project_client = ProjectClientLive {
        client: ProjectLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let project_srv = ProjectHandlerLive {
        client: &project_client,
    };
    let template_client = TemplateClientLive {
        client: ComponentLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let template_srv = TemplateHandlerLive {
        client: template_client.clone(),
        projects: &project_client,
    };
    let project_policy_client = ProjectPolicyClientLive {
        client: ProjectPolicyLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let project_policy_srv = ProjectPolicyHandlerLive {
        client: project_policy_client,
    };
    let project_grant_client = ProjectGrantClientLive {
        client: ProjectGrantLive {
            base_url: url.clone(),
            allow_insecure,
        },
    };
    let project_grant_srv = ProjectGrantHandlerLive {
        client: project_grant_client,
        project: &project_client,
    };
    let worker_client = WorkerClientLive {
        client: InstanceLive {
            base_url: url.clone(),
            allow_insecure,
        },
        base_url: url.clone(),
        allow_insecure,
    };
    let worker_srv = WorkerHandlerLive {
        client: worker_client.clone(),
        templates: &template_srv,
    };

    let auth = auth_srv
        .authenticate(
            cmd.auth_token,
            cmd.config_directory.clone().unwrap_or(default_conf_dir),
        )
        .await?;

    let res = match cmd.command {
        Command::Deploy {
            project_ref,
            template_name,
            worker_name,
            env,
            function,
            parameters,
            template_file,
            args,
        } => {
            handle_deploy(
                &project_client,
                &template_client,
                &worker_client,
                &auth,
                project_ref,
                template_name,
                worker_name,
                env,
                function,
                parameters,
                template_file,
                args,
            )
            .await
        }
        Command::Template { subcommand } => template_srv.handle(&auth, subcommand).await,
        Command::Worker { subcommand } => worker_srv.handle(&auth, subcommand).await,
        Command::Account {
            account_id,
            subcommand,
        } => acc_srv.handle(&auth, account_id, subcommand).await,
        Command::Token {
            account_id,
            subcommand,
        } => token_srv.handle(&auth, account_id, subcommand).await,
        Command::Project { subcommand } => project_srv.handle(&auth, subcommand).await,
        Command::Share {
            project_ref,
            recipient_account_id,
            project_policy_id,
            project_actions,
        } => {
            project_grant_srv
                .handle(
                    &auth,
                    project_ref,
                    recipient_account_id,
                    project_policy_id,
                    project_actions,
                )
                .await
        }
        Command::ProjectPolicy { subcommand } => project_policy_srv.handle(&auth, subcommand).await,
        Command::New {
            example,
            package_name,
            template_name,
        } => examples::process_new(example, template_name, package_name),
        Command::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
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
            GolemResult::Json(json) => match &cmd.format {
                Format::Json => Ok(println!("{}", serde_json::to_string_pretty(&json).unwrap())),
                Format::Yaml => Ok(println!("{}", serde_yaml::to_string(&json).unwrap())),
            },
        },
        Err(err) => Err(Box::new(err)),
    }
}
