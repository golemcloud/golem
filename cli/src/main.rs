extern crate derive_more;

use std::fmt::Debug;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{Level, Verbosity};
use golem_client::{Context, Security};
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use model::*;
use reqwest::Url;
use tracing::debug;
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

use crate::account::{AccountHandler, AccountHandlerLive, AccountSubcommand};
use crate::auth::{Auth, AuthLive};
use crate::clients::account::AccountClientLive;
use crate::clients::grant::GrantClientLive;
use crate::clients::login::LoginClientLive;
use crate::clients::policy::ProjectPolicyClientLive;
use crate::clients::project::ProjectClientLive;
use crate::clients::project_grant::ProjectGrantClientLive;
use crate::clients::template::TemplateClientLive;
use crate::clients::token::TokenClientLive;
use crate::clients::worker::WorkerClientLive;
use crate::gateway::{GatewayHandler, GatewayHandlerLive, GatewaySubcommand};
use crate::policy::{ProjectPolicyHandler, ProjectPolicyHandlerLive, ProjectPolicySubcommand};
use crate::project::{ProjectHandler, ProjectHandlerLive, ProjectSubcommand};
use crate::project_grant::{ProjectGrantHandler, ProjectGrantHandlerLive};
use crate::template::{TemplateHandler, TemplateHandlerLive, TemplateSubcommand};
use crate::token::{TokenHandler, TokenHandlerLive, TokenSubcommand};
use crate::worker::{WorkerHandler, WorkerHandlerLive, WorkerSubcommand};

mod account;
mod auth;
pub mod clients;
mod examples;
mod gateway;
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
    #[command()]
    Gateway {
        #[command(subcommand)]
        subcommand: GatewaySubcommand,
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

    if let Some(level) = command.verbosity.log_level() {
        let tracing_level = match level {
            Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        };

        let subscriber = FmtSubscriber::builder()
            .with_max_level(tracing_level)
            .with_writer(std::io::stderr)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(command))
}

async fn async_main(cmd: GolemCommand) -> Result<(), Box<dyn std::error::Error>> {
    let url_str =
        std::env::var("GOLEM_BASE_URL").unwrap_or("https://release.api.golem.cloud/".to_string());
    let gateway_url_str = std::env::var("GOLEM_GATEWAY_BASE_URL").unwrap_or(url_str.clone());
    let url = Url::parse(&url_str).unwrap();
    let gateway_url = Url::parse(&gateway_url_str).unwrap();
    let home = dirs::home_dir().unwrap();
    let allow_insecure_str = std::env::var("GOLEM_ALLOW_INSECURE").unwrap_or("false".to_string());
    let allow_insecure = allow_insecure_str != "false";
    let default_conf_dir = home.join(".golem");

    debug!(
        "Golem configuration directory: {}",
        default_conf_dir.display()
    );

    let mut builder = reqwest::Client::builder();
    if allow_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    let client = builder.connection_verbose(true).build()?;

    let login_context = Context {
        base_url: url.clone(),
        client: client.clone(),
        security_token: Security::Empty,
    };

    let login = LoginClientLive {
        client: golem_client::api::LoginClientLive {
            context: login_context.clone(),
        },
        context: login_context,
    };
    let auth_srv = AuthLive { login };

    let auth = auth_srv
        .authenticate(
            cmd.auth_token,
            cmd.config_directory.clone().unwrap_or(default_conf_dir),
        )
        .await?;

    let context = Context {
        base_url: url.clone(),
        client: client.clone(),
        security_token: Security::Bearer(auth.0.secret.value.to_string()),
    };

    let account_client = AccountClientLive {
        client: golem_client::api::AccountClientLive {
            context: context.clone(),
        },
    };
    let grant_client = GrantClientLive {
        client: golem_client::api::GrantClientLive {
            context: context.clone(),
        },
    };
    let acc_srv = AccountHandlerLive {
        client: account_client,
        grant: grant_client,
    };
    let token_client = TokenClientLive {
        client: golem_client::api::TokenClientLive {
            context: context.clone(),
        },
    };
    let token_srv = TokenHandlerLive {
        client: token_client,
    };
    let project_client = ProjectClientLive {
        client: golem_client::api::ProjectClientLive {
            context: context.clone(),
        },
    };
    let project_srv = ProjectHandlerLive {
        client: &project_client,
    };
    let template_client = TemplateClientLive {
        client: golem_client::api::TemplateClientLive {
            context: context.clone(),
        },
    };
    let template_srv = TemplateHandlerLive {
        client: template_client,
        projects: &project_client,
    };
    let project_policy_client = ProjectPolicyClientLive {
        client: golem_client::api::ProjectPolicyClientLive {
            context: context.clone(),
        },
    };
    let project_policy_srv = ProjectPolicyHandlerLive {
        client: project_policy_client,
    };
    let project_grant_client = ProjectGrantClientLive {
        client: golem_client::api::ProjectGrantClientLive {
            context: context.clone(),
        },
    };
    let project_grant_srv = ProjectGrantHandlerLive {
        client: project_grant_client,
        project: &project_client,
    };
    let worker_client = WorkerClientLive {
        client: golem_client::api::WorkerClientLive {
            context: context.clone(),
        },
        context: context.clone(),
        allow_insecure,
    };
    let worker_srv = WorkerHandlerLive {
        client: worker_client,
        templates: &template_srv,
    };
    let gateway_srv = GatewayHandlerLive {
        base_url: gateway_url.clone(),
        client,
        projects: &project_client,
    };

    let res = match cmd.command {
        Command::Template { subcommand } => template_srv.handle(subcommand).await,
        Command::Worker { subcommand } => worker_srv.handle(subcommand).await,
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
                    project_ref,
                    recipient_account_id,
                    project_policy_id,
                    project_actions,
                )
                .await
        }
        Command::ProjectPolicy { subcommand } => project_policy_srv.handle(subcommand).await,
        Command::New {
            example,
            package_name,
            template_name,
        } => examples::process_new(example, template_name, package_name),
        Command::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        Command::Gateway { subcommand } => gateway_srv.handle(cmd.format, &auth, subcommand).await,
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
