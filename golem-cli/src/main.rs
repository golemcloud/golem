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

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{Level, Verbosity};
use golem_cli::model::*;
use golem_client::Context;
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use reqwest::Url;
use tracing_subscriber::FmtSubscriber;

use golem_cli::api_definition::{
    ApiDefinitionHandler, ApiDefinitionHandlerLive, ApiDefinitionSubcommand,
};
use golem_cli::clients::api_definition::ApiDefinitionClientLive;
use golem_cli::clients::component::ComponentClientLive;
use golem_cli::clients::health_check::HealthCheckClientLive;
use golem_cli::clients::worker::WorkerClientLive;
use golem_cli::component::{ComponentHandler, ComponentHandlerLive, ComponentSubCommand};
use golem_cli::examples;
use golem_cli::version::{VersionHandler, VersionHandlerLive};
use golem_cli::worker::{WorkerHandler, WorkerHandlerLive, WorkerSubcommand};

#[derive(Subcommand, Debug)]
#[command()]
enum Command {
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand,
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

    /// WASM RPC stub generator
    #[cfg(feature = "stubgen")]
    Stubgen {
        #[command(subcommand)]
        subcommand: golem_wasm_rpc_stubgen::Command,
    },

    /// Manage Golem api definitions
    #[command()]
    ApiDefinition {
        #[command(subcommand)]
        subcommand: ApiDefinitionSubcommand,
    },
}

#[derive(Parser, Debug)]
#[command(author, version = option_env ! ("VERSION").unwrap_or(env ! ("CARGO_PKG_VERSION")), about, long_about, rename_all = "kebab-case")]
/// Command line interface for OSS version of Golem.
///
/// For Golem Cloud client see golem-cloud-cli instead: https://github.com/golemcloud/golem-cloud-cli
struct GolemCommand {
    #[command(flatten)]
    verbosity: Verbosity,

    #[arg(short = 'F', long, default_value = "text")]
    format: Format,

    #[arg(short = 'u', long)]
    /// Golem base url. Default: GOLEM_BASE_URL environment variable or http://localhost:9881.
    ///
    /// You can also specify different URLs for different services
    /// via GOLEM_COMPONENT_BASE_URL and GOLEM_WORKER_BASE_URL
    /// environment variables.
    golem_url: Option<String>,

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
    let url_str = cmd
        .golem_url
        .or_else(|| std::env::var("GOLEM_BASE_URL").ok())
        .unwrap_or("http://localhost:9881".to_string());
    let component_url_str = std::env::var("GOLEM_COMPONENT_BASE_URL")
        .ok()
        .unwrap_or(url_str.to_string());
    let worker_url_str = std::env::var("GOLEM_WORKER_BASE_URL")
        .ok()
        .unwrap_or(url_str);
    let component_url = Url::parse(&component_url_str).unwrap();
    let worker_url = Url::parse(&worker_url_str).unwrap();
    let allow_insecure_str = std::env::var("GOLEM_ALLOW_INSECURE").unwrap_or("false".to_string());
    let allow_insecure = allow_insecure_str != "false";

    let mut builder = reqwest::Client::builder();
    if allow_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    let client = builder.connection_verbose(true).build()?;

    let component_context = Context {
        base_url: component_url.clone(),
        client: client.clone(),
    };

    let worker_context = Context {
        base_url: worker_url.clone(),
        client: client.clone(),
    };

    let component_client = ComponentClientLive {
        client: golem_client::api::ComponentClientLive {
            context: component_context.clone(),
        },
    };
    let component_srv = ComponentHandlerLive {
        client: component_client,
    };
    let worker_client = WorkerClientLive {
        client: golem_client::api::WorkerClientLive {
            context: worker_context.clone(),
        },
        context: worker_context.clone(),
        allow_insecure,
    };
    let worker_srv = WorkerHandlerLive {
        client: worker_client,
        components: &component_srv,
        worker_context: worker_context.clone(),
        component_context: component_context.clone(),
        allow_insecure,
    };

    let api_definition_client = ApiDefinitionClientLive {
        client: golem_client::api::ApiDefinitionClientLive {
            context: worker_context.clone(),
        },
    };

    let api_definition_srv = ApiDefinitionHandlerLive {
        client: api_definition_client,
    };

    let health_check_client_for_component = HealthCheckClientLive {
        client: golem_client::api::HealthCheckClientLive {
            context: component_context.clone(),
        },
    };

    let health_check_client_for_worker = HealthCheckClientLive {
        client: golem_client::api::HealthCheckClientLive {
            context: worker_context.clone(),
        },
    };

    let update_srv = VersionHandlerLive {
        component_client: health_check_client_for_component,
        worker_client: health_check_client_for_worker,
    };

    let yellow = "\x1b[33m";
    let reset_color = "\x1b[0m";

    let version_check = update_srv.check().await;

    if let Err(err) = version_check {
        eprintln!("{}{}{}", yellow, err.0, reset_color)
    }

    let res = match cmd.command {
        Command::Component { subcommand } => component_srv.handle(subcommand).await,
        Command::Worker { subcommand } => worker_srv.handle(cmd.format, subcommand).await,
        Command::New {
            example,
            package_name,
            component_name,
        } => examples::process_new(example, component_name, package_name),
        Command::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        #[cfg(feature = "stubgen")]
        Command::Stubgen { subcommand } => match subcommand {
            golem_wasm_rpc_stubgen::Command::Generate(args) => {
                golem_wasm_rpc_stubgen::generate(args)
                    .map_err(|err| GolemError(format!("{err}")))
                    .map(|_| GolemResult::Str("Done".to_string()))
            }
            golem_wasm_rpc_stubgen::Command::Build(args) => golem_wasm_rpc_stubgen::build(args)
                .await
                .map_err(|err| GolemError(format!("{err}")))
                .map(|_| GolemResult::Str("Done".to_string())),
            golem_wasm_rpc_stubgen::Command::AddStubDependency(args) => {
                golem_wasm_rpc_stubgen::add_stub_dependency(args)
                    .map_err(|err| GolemError(format!("{err}")))
                    .map(|_| GolemResult::Str("Done".to_string()))
            }
            golem_wasm_rpc_stubgen::Command::Compose(args) => golem_wasm_rpc_stubgen::compose(args)
                .map_err(|err| GolemError(format!("{err}")))
                .map(|_| GolemResult::Str("Done".to_string())),
            golem_wasm_rpc_stubgen::Command::InitializeWorkspace(args) => {
                golem_wasm_rpc_stubgen::initialize_workspace(args, "golem-cli", &["stubgen"])
                    .map_err(|err| GolemError(format!("{err}")))
                    .map(|_| GolemResult::Str("Done".to_string()))
            }
        },
        Command::ApiDefinition { subcommand } => api_definition_srv.handle(subcommand).await,
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
                Format::Json | Format::Text => {
                    Ok(println!("{}", serde_json::to_string_pretty(&json).unwrap()))
                }
                Format::Yaml => Ok(println!("{}", serde_yaml::to_string(&json).unwrap())),
            },
        },
        Err(err) => Err(Box::new(err)),
    }
}
