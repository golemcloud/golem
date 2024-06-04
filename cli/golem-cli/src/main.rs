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

use clap::Parser;
use clap_verbosity_flag::Level;
use colored::Colorize;
use reqwest::Url;
use tracing_subscriber::FmtSubscriber;

use golem_cli::examples;
use golem_cli::factory::ServiceFactory;
use golem_cli::oss::command::{GolemOssCommand, OssCommand};
use golem_cli::oss::factory::OssServiceFactory;
use golem_cli::oss::model::OssContext;
use golem_cli::stubgen::handle_stubgen;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = GolemOssCommand::parse();

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

async fn async_main(cmd: GolemOssCommand) -> Result<(), Box<dyn std::error::Error>> {
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

    let factory = OssServiceFactory {
        component_url,
        worker_url,
        allow_insecure,
    };

    let ctx = &OssContext::EMPTY;

    let version_check = factory.version_service(ctx)?.check().await;

    if let Err(err) = version_check {
        eprintln!("{}", err.0.yellow())
    }

    let res = match cmd.command {
        OssCommand::Component { subcommand } => {
            subcommand
                .handle(factory.component_service(ctx)?.as_ref())
                .await
        }
        OssCommand::Worker { subcommand } => {
            subcommand
                .handle(cmd.format, factory.worker_service(ctx)?.as_ref())
                .await
        }
        OssCommand::New {
            example,
            package_name,
            component_name,
        } => examples::process_new(example, component_name, package_name),
        OssCommand::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        #[cfg(feature = "stubgen")]
        OssCommand::Stubgen { subcommand } => handle_stubgen(subcommand).await,
        OssCommand::ApiDefinition { subcommand } => {
            subcommand
                .handle(factory.api_definition_service(ctx)?.as_ref())
                .await
        }
        OssCommand::ApiDeployment { subcommand } => {
            subcommand
                .handle(factory.api_deployment_service(ctx)?.as_ref())
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
