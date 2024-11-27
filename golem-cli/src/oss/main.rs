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
use crate::completion::PrintCompletion;
use crate::config::{OssProfile, ProfileName};
use crate::diagnose::diagnose;
use crate::factory::ServiceFactory;
use crate::init::{init_profile, DummyProfileAuth, ProfileAuth};
use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use crate::oss::command::{GolemOssCommand, OssCommand};
use crate::oss::factory::OssServiceFactory;
use crate::oss::model::OssContext;
use crate::stubgen::handle_stubgen;
use crate::{check_for_newer_server_version, examples, ConfiguredMainArgs, MainArgs, VERSION};
use golem_client::model::{
    PluginDefinitionDefaultPluginOwnerDefaultPluginScope,
    PluginDefinitionWithoutOwnerDefaultPluginScope,
};
use golem_client::DefaultComponentOwner;
use golem_common::model::plugin::DefaultPluginScope;
use golem_common::uri::oss::uri::{ComponentUri, ResourceUri, WorkerUri};
use golem_common::uri::oss::url::{ComponentUrl, ResourceUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ComponentUrn, ResourceUrn, WorkerUrn};

pub async fn async_main<ProfileAdd: Into<UniversalProfileAdd> + clap::Args>(
    args: ConfiguredMainArgs<OssProfile, GolemOssCommand<ProfileAdd>>,
) -> Result<GolemResult, GolemError> {
    let format = args.format();
    let ConfiguredMainArgs {
        profile,
        profile_name: _,
        command,
        cli_kind,
        config_dir,
    } = args;

    let profile_auth = &DummyProfileAuth;

    let factory = || async {
        let factory = OssServiceFactory::from_profile(&profile)?;
        check_for_newer_server_version(factory.version_service().as_ref(), VERSION).await;
        Ok::<OssServiceFactory, GolemError>(factory)
    };

    match command.command {
        OssCommand::Component { subcommand } => {
            let factory = factory().await?;

            subcommand
                .handle(
                    format,
                    factory.component_service(),
                    factory.deploy_service(),
                    factory.project_resolver().as_ref(),
                )
                .await
        }
        OssCommand::Worker { subcommand } => {
            let factory = factory().await?;

            subcommand
                .handle(format, factory.worker_service(), factory.project_resolver())
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
            let factory = factory().await?;

            subcommand
                .handle(
                    factory.api_definition_service().as_ref(),
                    factory.project_resolver().as_ref(),
                )
                .await
        }
        OssCommand::ApiDeployment { subcommand } => {
            let factory = factory().await?;

            subcommand
                .handle(
                    factory.api_deployment_service().as_ref(),
                    factory.project_resolver().as_ref(),
                )
                .await
        }

        OssCommand::ApiSecurityScheme { subcommand } => {
            let factory = factory().await?;

            subcommand
                .handle(
                    factory.api_security_scheme_service().as_ref(),
                    factory.project_resolver().as_ref(),
                )
                .await
        }
        OssCommand::Profile { subcommand } => {
            subcommand.handle(cli_kind, &config_dir, profile_auth).await
        }
        OssCommand::Init {} => {
            let profile_name = ProfileName::default(cli_kind);

            let res = init_profile(cli_kind, profile_name, &config_dir, profile_auth).await?;

            if res.auth_required {
                profile_auth.auth(&res.profile_name, &config_dir).await?
            }

            Ok(GolemResult::Str("Profile created".to_string()))
        }
        OssCommand::Get { uri } => {
            let factory = factory().await?;

            get_resource_by_uri(uri, &factory).await
        }
        OssCommand::Completion { generator } => {
            GolemOssCommand::<ProfileAdd>::print_completion(generator);
            Ok(GolemResult::Str("".to_string()))
        }
        OssCommand::Diagnose { command } => {
            diagnose(command);
            Ok(GolemResult::Str("".to_string()))
        }
        OssCommand::Plugin { subcommand } => {
            let factory = factory().await?;

            subcommand
                .handle::<PluginDefinitionDefaultPluginOwnerDefaultPluginScope, PluginDefinitionWithoutOwnerDefaultPluginScope, OssContext, DefaultPluginScope, DefaultComponentOwner, OssContext>(
                    format,
                    factory.plugin_client(),
                    factory.project_resolver(),
                    factory.component_service(),
                )
                .await
        }
    }
}

async fn get_resource_by_urn(
    urn: ResourceUrn,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    let ctx = &OssContext::EMPTY;

    match urn {
        ResourceUrn::Component(c) => {
            factory
                .component_service()
                .get(ComponentUri::URN(c), None, None)
                .await
        }
        ResourceUrn::ComponentVersion(c) => {
            factory
                .component_service()
                .get(
                    ComponentUri::URN(ComponentUrn { id: c.id }),
                    Some(c.version),
                    None,
                )
                .await
        }
        ResourceUrn::Worker(w) => factory.worker_service().get(WorkerUri::URN(w), None).await,
        ResourceUrn::WorkerFunction(f) => {
            factory
                .worker_service()
                .get_function(
                    WorkerUri::URN(WorkerUrn {
                        id: f.id.into_target_worker_id(),
                    }),
                    &f.function,
                    None,
                )
                .await
        }
        ResourceUrn::ApiDefinition(ad) => {
            factory
                .api_definition_service()
                .get(
                    ApiDefinitionId(ad.id),
                    ApiDefinitionVersion(ad.version),
                    ctx,
                )
                .await
        }
        ResourceUrn::ApiDeployment(ad) => factory.api_deployment_service().get(ad.site).await,
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
                .component_service()
                .get(ComponentUri::URL(c), None, None)
                .await
        }
        ResourceUrl::ComponentVersion(c) => {
            factory
                .component_service()
                .get(
                    ComponentUri::URL(ComponentUrl { name: c.name }),
                    Some(c.version),
                    None,
                )
                .await
        }
        ResourceUrl::Worker(w) => factory.worker_service().get(WorkerUri::URL(w), None).await,
        ResourceUrl::WorkerFunction(f) => {
            factory
                .worker_service()
                .get_function(
                    WorkerUri::URL(WorkerUrl {
                        component_name: f.component_name,
                        worker_name: Some(f.worker_name),
                    }),
                    &f.function,
                    None,
                )
                .await
        }
        ResourceUrl::ApiDefinition(ad) => {
            factory
                .api_definition_service()
                .get(
                    ApiDefinitionId(ad.name),
                    ApiDefinitionVersion(ad.version),
                    ctx,
                )
                .await
        }
        ResourceUrl::ApiDeployment(ad) => factory.api_deployment_service().get(ad.site).await,
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
