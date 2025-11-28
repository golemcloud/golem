// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command::environment::EnvironmentSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use golem_client::api::EnvironmentClient;
use golem_client::model::EnvironmentCreation;
use golem_common::model::application::ApplicationId;
use golem_common::model::environment::EnvironmentName;
use std::sync::Arc;

pub struct EnvironmentCommandHandler {
    ctx: Arc<Context>,
}

impl EnvironmentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, _subcommand: EnvironmentSubcommand) -> anyhow::Result<()> {
        // TODO: atomic
        todo!()
    }

    // TODO: atomic: recheck if and when we need to cache this
    pub async fn resolve_environment(
        &self,
        mode: EnvironmentResolveMode,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        match self.ctx.environment_reference() {
            Some(environment_reference) => {
                self.resolve_environment_reference(mode, environment_reference)
                    .await
            }
            None => {
                match self.ctx.manifest_environment() {
                    Some(env) => {
                        let application = self
                            .ctx
                            .app_handler()
                            .get_or_create_remote_application()
                            .await?;

                        match application {
                            Some(application) => {
                                let environment = self
                                    .get_or_create_remote_environment(
                                        &application.id,
                                        &env.environment_name,
                                    )
                                    .await?;
                                Ok(ResolvedEnvironmentIdentity::new(
                                    None,
                                    application,
                                    environment,
                                ))
                            }
                            None => {
                                // TODO: atomic: show error about
                                //       - using an app manifest
                                //       - using flags
                                //       - using ENV VARS
                                todo!()
                            }
                        }
                    }
                    None => {
                        // TODO: atomic: show error about
                        //       - using an app manifest
                        //       - using flags
                        //       - using ENV VARS
                        todo!()
                    }
                }
            }
        }
    }

    pub async fn resolve_environment_reference(
        &self,
        mode: EnvironmentResolveMode,
        environment_reference: &EnvironmentReference,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        if !mode.allowed(environment_reference) {
            // TODO: atomic: message about manifest etc.
            todo!()
        }

        match environment_reference {
            // NOTE: when only the env name is included in the reference,
            //       we on-demand create the application and the env
            EnvironmentReference::Environment { environment_name } => {
                let application = self
                    .ctx
                    .app_handler()
                    .get_or_create_remote_application()
                    .await?;

                match application {
                    Some(application) => {
                        let environment = self
                            .get_or_create_remote_environment(&application.id, environment_name)
                            .await?;
                        Ok(ResolvedEnvironmentIdentity::new(
                            Some(environment_reference),
                            application,
                            environment,
                        ))
                    }
                    None => {
                        // TODO: atomic: show error about
                        //       - using an app manifest
                        //       - using flags
                        //       - using ENV VARS
                        todo!()
                    }
                }
            }
            // NOTE: with app-env references we DO NOT create anything, these are used for
            //       querying without using the manifest
            EnvironmentReference::ApplicationEnvironment {
                application_name,
                environment_name,
            } => {
                let application = self
                    .ctx
                    .app_handler()
                    .get_remote_application(&self.ctx.account_id().await?, application_name)
                    .await?;

                match application {
                    Some(application) => {
                        let environment = self
                            .ctx
                            .golem_clients()
                            .await?
                            .environment
                            .get_application_environment(&application.id.0, &environment_name.0)
                            .await?;
                        Ok(ResolvedEnvironmentIdentity::new(
                            Some(environment_reference),
                            application,
                            environment,
                        ))
                    }
                    None => {
                        // TODO: atomic: show error about
                        //       - using an app manifest
                        //       - using flags
                        //       - using ENV VARS
                        todo!()
                    }
                }
            }
            EnvironmentReference::AccountApplicationEnvironment { .. } => {
                // TODO: atomic: use search / lookup API once available
                // TODO: this mode should be dynamic on auto-creation based on the current account id
                //       and the use case
                todo!()
            }
        }
    }

    async fn get_remote_environment(
        &self,
        application_id: &ApplicationId,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<Option<golem_client::model::Environment>> {
        self.ctx
            .golem_clients()
            .await?
            .environment
            .get_application_environment(&application_id.0, &environment_name.0)
            .await
            .map_service_error_not_found_as_opt()
    }

    async fn get_or_create_remote_environment(
        &self,
        application_id: &ApplicationId,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<golem_client::model::Environment> {
        match self
            .get_remote_environment(application_id, environment_name)
            .await?
        {
            Some(environment) => Ok(environment),
            None => self
                .ctx
                .golem_clients()
                .await?
                .environment
                .create_environment(
                    &application_id.0,
                    &EnvironmentCreation {
                        name: environment_name.clone(),
                        // TODO: atomic: get props from manifest
                        compatibility_check: false,
                        version_check: false,
                        security_overrides: false,
                    },
                )
                .await
                .map_service_error(),
        }
    }
}
