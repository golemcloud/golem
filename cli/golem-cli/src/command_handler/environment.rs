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
use crate::model::environment::{EnvironmentReference, ResolvedEnvironmentIdentity};
use golem_client::api::EnvironmentClient;
use golem_client::model::NewEnvironmentData;
use golem_common::model::application::ApplicationId;
use golem_common::model::environment::EnvironmentName;
use std::sync::Arc;

pub struct EnvironmentCommandHandler {
    // TODO: atomic
    #[allow(unused)]
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

    pub async fn select_environment(
        &self,
        environment: Option<&EnvironmentReference>,
    ) -> anyhow::Result<ResolvedEnvironmentIdentity> {
        let clients = self.ctx.golem_clients().await?;
        let request_environment = environment;
        match request_environment.or_else(|| self.ctx.default_environment()) {
            Some(environment) => {
                match environment {
                    // NOTE: when only env is referenced, we auto create the application and the env
                    //       if needed; this is used together with app manifest
                    EnvironmentReference::Environment { environment_name } => {
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
                                        environment_name,
                                    )
                                    .await?;
                                Ok(ResolvedEnvironmentIdentity {
                                    resolved_from: request_environment.cloned(),
                                    account_id: application.account_id,
                                    application_id: application.id,
                                    application_name: application.name,
                                    environment_id: environment.id,
                                    environment_name: environment.name,
                                })
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
                                let environment = clients
                                    .environment
                                    .get_application_environment(
                                        &application.id.0,
                                        &environment_name.0,
                                    )
                                    .await?;
                                Ok(ResolvedEnvironmentIdentity {
                                    resolved_from: request_environment.cloned(),
                                    account_id: application.account_id,
                                    application_id: application.id,
                                    application_name: application.name,
                                    environment_id: environment.id,
                                    environment_name: environment.name,
                                })
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
                        //       and the usecase
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
                    &NewEnvironmentData {
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
