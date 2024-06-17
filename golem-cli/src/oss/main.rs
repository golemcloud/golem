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

use crate::config::{OssProfile, ProfileName};
use crate::examples;
use crate::factory::ServiceFactory;
use crate::init::{CliKind, DummyProfileAuth};
use crate::model::GolemResult;
use crate::oss::command::{GolemOssCommand, OssCommand};
use crate::oss::factory::OssServiceFactory;
use crate::oss::model::OssContext;
use crate::stubgen::handle_stubgen;
use colored::Colorize;
use std::path::PathBuf;

pub async fn async_main(
    cmd: GolemOssCommand,
    profile: OssProfile,
    config_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let OssProfile {
        url,
        worker_url,
        allow_insecure,
        config,
    } = profile;

    let format = cmd.format.unwrap_or(config.default_format);

    let component_url = url;
    let worker_url = worker_url.unwrap_or_else(|| component_url.clone());

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
                .handle(
                    factory.component_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::Worker { subcommand } => {
            subcommand
                .handle(
                    format,
                    factory.worker_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
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
                .handle(
                    factory.api_definition_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::ApiDeployment { subcommand } => {
            subcommand
                .handle(
                    factory.api_deployment_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::Profile { subcommand } => {
            subcommand
                .handle(CliKind::Universal, &config_dir, &DummyProfileAuth {})
                .await
        }
        OssCommand::Init {} => crate::init::init_profile(
            CliKind::Universal,
            ProfileName::default(CliKind::Universal),
            &config_dir,
        )
        .await
        .map(|_| GolemResult::Str("Profile created".to_string())),
    };

    match res {
        Ok(res) => {
            res.print(format);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}
