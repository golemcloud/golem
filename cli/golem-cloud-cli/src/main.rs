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

extern crate derive_more;

use std::fmt::Debug;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{Level, Verbosity};
use golem_cloud_client::{Context, Security};
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use model::*;
use reqwest::Url;
use tracing::debug;
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

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
use crate::clients::worker::WorkerClientLive;
use crate::component::{ComponentHandler, ComponentHandlerLive, ComponentSubcommand};
use crate::gateway::{GatewayHandler, GatewayHandlerLive, GatewaySubcommand};
use crate::policy::{ProjectPolicyHandler, ProjectPolicyHandlerLive, ProjectPolicySubcommand};
use crate::project::{ProjectHandler, ProjectHandlerLive, ProjectSubcommand};
use crate::project_grant::{ProjectGrantHandler, ProjectGrantHandlerLive};
use crate::token::{TokenHandler, TokenHandlerLive, TokenSubcommand};
use crate::worker::{WorkerHandler, WorkerHandlerLive, WorkerSubcommand};

mod account;
mod auth;
pub mod clients;
mod component;
mod examples;
mod gateway;
pub mod model;
mod policy;
mod project;
mod project_grant;
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
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubcommand,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand,
    },

    /// Manage accounts
    #[command()]
    Account {
        /// The account ID to operate on
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: AccountSubcommand,
    },

    /// Manage access tokens
    #[command()]
    Token {
        /// The account ID to operate on
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: TokenSubcommand,
    },

    /// Manage projects
    #[command()]
    Project {
        #[command(subcommand)]
        subcommand: ProjectSubcommand,
    },

    /// Share a project with another account
    #[command()]
    Share {
        /// Project to be shared
        #[command(flatten)]
        project_ref: ProjectRef,

        /// User account the project will be shared with
        #[arg(long)]
        recipient_account_id: AccountId,

        /// The sharing policy's identifier. If not provided, use `--project-actions` instead
        #[arg(long, required = true, conflicts_with = "project_actions")]
        project_policy_id: Option<ProjectPolicyId>,

        /// A list of actions to be granted to the recipient account. If not provided, use `--project-policy-id` instead
        #[arg(
            short = 'A',
            long,
            required = true,
            conflicts_with = "project_policy_id"
        )]
        project_actions: Option<Vec<ProjectAction>>,
    },

    /// Manage project sharing policies
    #[command()]
    ProjectPolicy {
        #[command(subcommand)]
        subcommand: ProjectPolicySubcommand,
    },
    /// Create a new Golem component from built-in examples
    #[command()]
    New {
        /// Name of the example to use
        #[arg(short, long)]
        example: ExampleName,

        /// The new component's name
        #[arg(short, long)]
        component_name: golem_examples::model::TemplateName,

        /// The package name of the generated component (in namespace:name format)
        #[arg(short, long)]
        package_name: Option<PackageName>,
    },
    /// Lists the built-in examples available for creating new components
    #[command()]
    ListExamples {
        /// The minimum language tier to include in the list
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        /// Filter examples by a given guest language
        #[arg(short, long)]
        language: Option<GuestLanguage>,
    },
    #[command()]
    Gateway {
        #[command(subcommand)]
        subcommand: GatewaySubcommand,
    },

    /// WASM RPC stub generator
    #[cfg(feature = "stubgen")]
    Stubgen {
        #[command(subcommand)]
        subcommand: golem_wasm_rpc_stubgen::Command,
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
    let url_str = std::env::var("GOLEM_CLOUD_BASE_URL")
        .ok()
        .or_else(|| std::env::var("GOLEM_BASE_URL").ok())
        .unwrap_or("https://release.api.golem.cloud/".to_string());
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
        client: golem_cloud_client::api::LoginClientLive {
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
        client: golem_cloud_client::api::AccountClientLive {
            context: context.clone(),
        },
    };
    let grant_client = GrantClientLive {
        client: golem_cloud_client::api::GrantClientLive {
            context: context.clone(),
        },
    };
    let acc_srv = AccountHandlerLive {
        client: account_client,
        grant: grant_client,
    };
    let token_client = TokenClientLive {
        client: golem_cloud_client::api::TokenClientLive {
            context: context.clone(),
        },
    };
    let token_srv = TokenHandlerLive {
        client: token_client,
    };
    let project_client = ProjectClientLive {
        client: golem_cloud_client::api::ProjectClientLive {
            context: context.clone(),
        },
    };
    let project_srv = ProjectHandlerLive {
        client: &project_client,
    };
    let component_client = ComponentClientLive {
        client: golem_cloud_client::api::ComponentClientLive {
            context: context.clone(),
        },
    };
    let component_srv = ComponentHandlerLive {
        client: component_client,
        projects: &project_client,
    };
    let project_policy_client = ProjectPolicyClientLive {
        client: golem_cloud_client::api::ProjectPolicyClientLive {
            context: context.clone(),
        },
    };
    let project_policy_srv = ProjectPolicyHandlerLive {
        client: project_policy_client,
    };
    let project_grant_client = ProjectGrantClientLive {
        client: golem_cloud_client::api::ProjectGrantClientLive {
            context: context.clone(),
        },
    };
    let project_grant_srv = ProjectGrantHandlerLive {
        client: project_grant_client,
        project: &project_client,
    };
    let worker_client = WorkerClientLive {
        client: golem_cloud_client::api::WorkerClientLive {
            context: context.clone(),
        },
        context: context.clone(),
        allow_insecure,
    };
    let worker_srv = WorkerHandlerLive {
        client: worker_client,
        components: &component_srv,
    };
    let gateway_srv = GatewayHandlerLive {
        base_url: gateway_url.clone(),
        client,
        projects: &project_client,
    };

    let res = match cmd.command {
        Command::Component { subcommand } => component_srv.handle(subcommand).await,
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
            component_name,
        } => examples::process_new(example, component_name, package_name),
        Command::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        Command::Gateway { subcommand } => gateway_srv.handle(cmd.format, &auth, subcommand).await,
        #[cfg(feature = "stubgen")]
        Command::Stubgen { subcommand } => match subcommand {
            golem_wasm_rpc_stubgen::Command::Generate(args) => {
                golem_wasm_rpc_stubgen::generate(args)
                    .map_err(|err| GolemError(format!("{err}")))
                    .map(|_| GolemResult::Ok(Box::new("Done")))
            }
            golem_wasm_rpc_stubgen::Command::Build(args) => golem_wasm_rpc_stubgen::build(args)
                .await
                .map_err(|err| GolemError(format!("{err}")))
                .map(|_| GolemResult::Ok(Box::new("Done"))),
            golem_wasm_rpc_stubgen::Command::AddStubDependency(args) => {
                golem_wasm_rpc_stubgen::add_stub_dependency(args)
                    .map_err(|err| GolemError(format!("{err}")))
                    .map(|_| GolemResult::Ok(Box::new("Done")))
            }
            golem_wasm_rpc_stubgen::Command::Compose(args) => golem_wasm_rpc_stubgen::compose(args)
                .map_err(|err| GolemError(format!("{err}")))
                .map(|_| GolemResult::Ok(Box::new("Done"))),
            golem_wasm_rpc_stubgen::Command::InitializeWorkspace(args) => {
                golem_wasm_rpc_stubgen::initialize_workspace(args, "golem-cloud-cli", &["stubgen"])
                    .map_err(|err| GolemError(format!("{err}")))
                    .map(|_| GolemResult::Ok(Box::new("Done")))
            }
        },
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
