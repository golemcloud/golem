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

use clap::Parser;
use clap_verbosity_flag::Level;
use colored::Colorize;
use golem_cli::cloud::command::{CloudCommand, GolemCloudCommand};
use golem_cli::cloud::factory::CloudServiceFactory;
use golem_cli::examples;
use golem_cli::factory::ServiceFactory;
use golem_cli::stubgen::handle_stubgen;
use tracing::debug;
use tracing_subscriber::FmtSubscriber;
use url::Url;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = GolemCloudCommand::parse();

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

async fn async_main(cmd: GolemCloudCommand) -> Result<(), Box<dyn std::error::Error>> {
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

    let factory = CloudServiceFactory {
        url,
        gateway_url,
        allow_insecure,
    };

    let auth = factory
        .auth()?
        .authenticate(
            cmd.auth_token,
            cmd.config_directory.clone().unwrap_or(default_conf_dir),
        )
        .await?;

    let version_check = factory.version_service(&auth)?.check().await;

    if let Err(err) = version_check {
        eprintln!("{}", err.0.yellow())
    }

    let res = match cmd.command {
        CloudCommand::Component { subcommand } => {
            subcommand
                .handle(
                    factory.component_service(&auth)?.as_ref(),
                    factory.project_service(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Worker { subcommand } => {
            subcommand
                .handle(
                    cmd.format,
                    factory.worker_service(&auth)?.as_ref(),
                    factory.project_service(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Account {
            account_id,
            subcommand,
        } => {
            subcommand
                .handle(
                    account_id,
                    factory.account_service(&auth)?.as_ref(),
                    factory.grant_service(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Token {
            account_id,
            subcommand,
        } => {
            subcommand
                .handle(account_id, factory.token_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Project { subcommand } => {
            subcommand
                .handle(factory.project_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Share {
            project_ref,
            recipient_account_id,
            project_policy_id,
            project_actions,
        } => {
            factory
                .project_grant_service(&auth)?
                .grant(
                    project_ref,
                    recipient_account_id,
                    project_policy_id,
                    project_actions,
                )
                .await
        }
        CloudCommand::ProjectPolicy { subcommand } => {
            subcommand
                .handle(factory.project_policy_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::New {
            example,
            package_name,
            component_name,
        } => examples::process_new(example, component_name, package_name),
        CloudCommand::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        #[cfg(feature = "stubgen")]
        CloudCommand::Stubgen { subcommand } => handle_stubgen(subcommand).await,
        CloudCommand::ApiDefinition { subcommand } => {
            subcommand
                .handle(
                    factory.api_definition_service(&auth)?.as_ref(),
                    factory.project_service(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::ApiDeployment { subcommand } => {
            subcommand
                .handle(
                    factory.api_deployment_service(&auth)?.as_ref(),
                    factory.project_service(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Certificate { subcommand } => {
            subcommand
                .handle(factory.certificate_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Domain { subcommand } => {
            subcommand
                .handle(factory.domain_service(&auth)?.as_ref())
                .await
        }
    };

    match res {
        Ok(res) => {
            res.print(cmd.format);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}
