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

use crate::command::profile::UniversalProfileAdd;
use crate::config::{OssProfile, ProfileName};
use crate::examples;
use crate::factory::ServiceFactory;
use crate::init::{CliKind, DummyProfileAuth};
use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use crate::oss::command::{GolemOssCommand, OssCommand};
use crate::oss::factory::OssServiceFactory;
use crate::oss::model::OssContext;
use crate::stubgen::handle_stubgen;
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use golem_common::uri::oss::uri::{ComponentUri, ResourceUri, WorkerUri};
use golem_common::uri::oss::url::{ComponentUrl, ResourceUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, ResourceUrn, WorkerUrn};
use std::path::PathBuf;

pub async fn async_main<ProfileAdd: Into<UniversalProfileAdd> + clap::Args>(
    cmd: GolemOssCommand<ProfileAdd>,
    profile: OssProfile,
    cli_kind: CliKind,
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

    let res = match cmd.command {
        OssCommand::Component { subcommand } => {
            check_version(&factory, &cmd.verbosity).await?;

            subcommand
                .handle(
                    factory.component_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::Worker { subcommand } => {
            check_version(&factory, &cmd.verbosity).await?;

            subcommand
                .handle(
                    format,
                    factory.worker_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::Examples(golem_examples::cli::Command::New {
            name_or_language,
            package_name,
            component_name,
        }) => examples::process_new(
            name_or_language.example_name(),
            component_name,
            package_name,
        ),
        OssCommand::Examples(golem_examples::cli::Command::ListExamples { min_tier, language }) => {
            examples::process_list_examples(min_tier, language)
        }
        #[cfg(feature = "stubgen")]
        OssCommand::Stubgen { subcommand } => handle_stubgen(subcommand).await,
        OssCommand::ApiDefinition { subcommand } => {
            check_version(&factory, &cmd.verbosity).await?;

            subcommand
                .handle(
                    factory.api_definition_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::ApiDeployment { subcommand } => {
            check_version(&factory, &cmd.verbosity).await?;

            subcommand
                .handle(
                    factory.api_deployment_service(ctx)?.as_ref(),
                    factory.project_resolver(ctx)?.as_ref(),
                )
                .await
        }
        OssCommand::Profile { subcommand } => {
            subcommand
                .handle(cli_kind, &config_dir, &DummyProfileAuth {})
                .await
        }
        OssCommand::Init {} => {
            crate::init::init_profile(cli_kind, ProfileName::default(cli_kind), &config_dir)
                .await
                .map(|_| GolemResult::Str("Profile created".to_string()))
        }
        OssCommand::Get { uri } => {
            check_version(&factory, &cmd.verbosity).await?;

            get_resource_by_uri(uri, &factory).await
        }
    };

    match res {
        Ok(res) => {
            res.print(format);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}

async fn check_version(
    factory: &OssServiceFactory,
    verbosity: &Verbosity,
) -> Result<(), GolemError> {
    let ctx = &OssContext::EMPTY;

    let version_check = factory.version_service(ctx)?.check().await;

    if let Err(err) = version_check {
        if verbosity.log_level() == Some(log::Level::Warn)
            || verbosity.log_level() == Some(log::Level::Info)
            || verbosity.log_level() == Some(log::Level::Debug)
            || verbosity.log_level() == Some(log::Level::Trace)
        {
            eprintln!("{}", err.0.yellow())
        }
    }

    Ok(())
}

async fn get_resource_by_urn(
    urn: ResourceUrn,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    let ctx = &OssContext::EMPTY;

    match urn {
        ResourceUrn::Component(c) => {
            factory
                .component_service(ctx)?
                .get(ComponentUri::URN(c), None, None)
                .await
        }
        ResourceUrn::ComponentVersion(c) => {
            factory
                .component_service(ctx)?
                .get(
                    ComponentUri::URN(ComponentUrn { id: c.id }),
                    Some(c.version),
                    None,
                )
                .await
        }
        ResourceUrn::Worker(w) => {
            factory
                .worker_service(ctx)?
                .get(WorkerUri::URN(w), None)
                .await
        }
        ResourceUrn::WorkerFunction(f) => {
            factory
                .worker_service(ctx)?
                .get_function(WorkerUri::URN(WorkerUrn { id: f.id }), &f.function, None)
                .await
        }
        ResourceUrn::ApiDefinition(ad) => {
            factory
                .api_definition_service(ctx)?
                .get(
                    ApiDefinitionId(ad.id),
                    ApiDefinitionVersion(ad.version),
                    ctx,
                )
                .await
        }
        ResourceUrn::ApiDeployment(ad) => factory.api_deployment_service(ctx)?.get(ad.site).await,
    }
}

async fn get_resource_by_url(
    url: ResourceUrl,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    let ctx = &OssContext::EMPTY;

    match url {
        ResourceUrl::Component(c) => {
            factory
                .component_service(ctx)?
                .get(ComponentUri::URL(c), None, None)
                .await
        }
        ResourceUrl::ComponentVersion(c) => {
            factory
                .component_service(ctx)?
                .get(
                    ComponentUri::URL(ComponentUrl { name: c.name }),
                    Some(c.version),
                    None,
                )
                .await
        }
        ResourceUrl::Worker(w) => {
            factory
                .worker_service(ctx)?
                .get(WorkerUri::URL(w), None)
                .await
        }
        ResourceUrl::WorkerFunction(f) => {
            factory
                .worker_service(ctx)?
                .get_function(
                    WorkerUri::URL(WorkerUrl {
                        component_name: f.component_name,
                        worker_name: f.worker_name,
                    }),
                    &f.function,
                    None,
                )
                .await
        }
        ResourceUrl::ApiDefinition(ad) => {
            factory
                .api_definition_service(ctx)?
                .get(
                    ApiDefinitionId(ad.name),
                    ApiDefinitionVersion(ad.version),
                    ctx,
                )
                .await
        }
        ResourceUrl::ApiDeployment(ad) => factory.api_deployment_service(ctx)?.get(ad.site).await,
    }
}

async fn get_resource_by_uri(
    uri: ResourceUri,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    match uri {
        ResourceUri::URN(urn) => get_resource_by_urn(urn, factory).await,
        ResourceUri::URL(url) => get_resource_by_url(url, factory).await,
    }
}
