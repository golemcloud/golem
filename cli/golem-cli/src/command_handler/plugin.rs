// Copyright 2024-2025 Golem Cloud
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

use crate::command::plugin::PluginSubcommand;
use crate::command::shared_args::PluginScopeArgs;
use crate::command_handler::Handlers;
use crate::context::{Context, GolemClients};
use crate::error::service::AnyhowMapServiceError;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::component::Component;
use crate::model::plugin_manifest::{PluginManifest, PluginTypeSpecificManifest};
use crate::model::{
    ComponentName, PathBufOrStdin, PluginDefinition, ProjectNameAndId, ProjectReference,
};
use anyhow::{anyhow, Context as AnyhowContext};
use golem_client::api::{ComponentClient as ComponentClientOss, PluginClient as PluginClientOss};
use golem_client::model::{
    ComponentTransformerDefinition, ComponentType, DefaultPluginScope, OplogProcessorDefinition,
    PluginDefinitionCreationDefaultPluginScope, PluginTypeSpecificCreation,
};
use golem_cloud_client::api::{
    ComponentClient as ComponentClientCloud, PluginClient as PluginClientCloud,
};
use golem_cloud_client::model::{ComponentQuery, PluginDefinitionCreationCloudPluginScope};
use golem_cloud_client::{CloudPluginScope, ProjectPluginScope};
use golem_common::model::plugin::ComponentPluginScope;
use golem_common::model::{ComponentId, Empty};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;

pub struct PluginCommandHandler {
    ctx: Arc<Context>,
}

impl PluginCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: PluginSubcommand) -> anyhow::Result<()> {
        match subcommand {
            PluginSubcommand::List { scope } => self.cmd_list(scope).await,
            PluginSubcommand::Get {
                plugin_name,
                version,
            } => self.cmd_get(plugin_name, version).await,
            PluginSubcommand::Register { scope, manifest } => {
                self.cmd_register(scope, manifest).await
            }
            PluginSubcommand::Unregister {
                plugin_name,
                version,
            } => self.cmd_unregister(plugin_name, version).await,
        }
    }

    async fn cmd_list(&self, scope: PluginScopeArgs) -> anyhow::Result<()> {
        let (scope_project, scope_component_id) = self.resolve_scope(&scope).await?;

        let plugin_definitions = match self.ctx.golem_clients().await? {
            GolemClients::Oss(clients) => clients
                .plugin
                .list_plugins(Some(&default_plugin_scope(scope_component_id.as_ref())))
                .await
                .map(|plugins| {
                    plugins
                        .into_iter()
                        .map(PluginDefinition::from)
                        .collect::<Vec<_>>()
                })
                .map_service_error()?,
            GolemClients::Cloud(clients) => clients
                .plugin
                .list_plugins(Some(&cloud_plugin_scope(
                    scope_project.as_ref(),
                    scope_component_id.as_ref(),
                )))
                .await
                .map(|plugins| {
                    plugins
                        .into_iter()
                        .map(PluginDefinition::from)
                        .collect::<Vec<_>>()
                })
                .map_service_error()?,
        };

        self.ctx.log_handler().log_view(&plugin_definitions);

        Ok(())
    }

    async fn cmd_get(&self, plugin_name: String, version: String) -> anyhow::Result<()> {
        let plugin_definition = self.get(&plugin_name, &version).await?;
        self.ctx.log_handler().log_view(&plugin_definition);
        Ok(())
    }

    async fn cmd_register(
        &self,
        scope: PluginScopeArgs,
        manifest: PathBufOrStdin,
    ) -> anyhow::Result<()> {
        enum Specs {
            ComponentTransformerOrOplogProcessor(PluginTypeSpecificCreation),
            App(PathBuf),
            Library(PathBuf),
        }

        let (scope_project, scope_component_id) = self.resolve_scope(&scope).await?;
        let manifest = manifest.read_to_string()?;
        let manifest: PluginManifest = serde_yaml::from_str(&manifest)
            .with_context(|| anyhow!("Failed to decode plugin manifest"))?;

        let icon = std::fs::read(&manifest.icon)
            .with_context(|| anyhow!("Failed to read plugin icon: {}", &manifest.icon.display()))?;

        let specs = match &manifest.specs {
            PluginTypeSpecificManifest::ComponentTransformer(spec) => {
                Specs::ComponentTransformerOrOplogProcessor(
                    PluginTypeSpecificCreation::ComponentTransformer(
                        ComponentTransformerDefinition {
                            provided_wit_package: spec.provided_wit_package.clone(),
                            json_schema: spec.json_schema.clone(),
                            validate_url: spec.validate_url.clone(),
                            transform_url: spec.transform_url.clone(),
                        },
                    ),
                )
            }
            PluginTypeSpecificManifest::OplogProcessor(spec) => {
                let component_name = ComponentName(format!(
                    "oplog_processor:{}:{}",
                    manifest.name, manifest.version
                ));

                let component_file = File::open(&spec.component).await.with_context(|| {
                    anyhow!(
                        "Failed to open plugin component WASM at {}",
                        &spec.component.display().to_string().log_color_highlight()
                    )
                })?;

                let component = {
                    log_action(
                        "Uploading",
                        format!("oplog processor component: {}", component_name),
                    );
                    let _indent = LogIndent::new();

                    // TODO: already existing is not handled here, let's do that when we make it part of the manifest
                    let component = match self.ctx.golem_clients().await? {
                        GolemClients::Oss(clients) => {
                            clients
                                .component
                                .create_component(
                                    &component_name.0,
                                    Some(&ComponentType::Durable), // TODO: do we want to support ephemeral oplog processors?
                                    component_file,
                                    None,
                                    None::<File>,
                                    None,
                                )
                                .await
                                .map(Component::from)
                                .map_service_error()?
                        }
                        GolemClients::Cloud(clients) => {
                            clients
                                .component
                                .create_component(
                                    &ComponentQuery {
                                        project_id: scope_project.as_ref().map(|p| p.project_id.0),
                                        component_name: component_name.0.clone(),
                                    },
                                    component_file,
                                    Some(&ComponentType::Durable), // TODO: do we want to support ephemeral oplog processors?
                                    None,
                                    None::<File>,
                                    None,
                                )
                                .await
                                .map(Component::from)
                                .map_service_error()?
                        }
                    };

                    log_action(
                        "Uploaded",
                        format!(
                            "oplog processor component {} as {}/{}",
                            component_name.0.log_color_highlight(),
                            component.versioned_component_id.component_id,
                            component.versioned_component_id.version
                        ),
                    );

                    component
                };

                Specs::ComponentTransformerOrOplogProcessor(
                    PluginTypeSpecificCreation::OplogProcessor(OplogProcessorDefinition {
                        component_id: component.versioned_component_id.component_id,
                        component_version: component.versioned_component_id.version,
                    }),
                )
            }
            PluginTypeSpecificManifest::App(specs) => Specs::App(specs.component.clone()),
            PluginTypeSpecificManifest::Library(specs) => Specs::Library(specs.component.clone()),
        };

        {
            log_action(
                "Registering",
                format!(
                    "plugin {}/{}",
                    manifest.name.log_color_highlight(),
                    manifest.version.log_color_highlight()
                ),
            );

            let _indent = LogIndent::new();

            match specs {
                Specs::ComponentTransformerOrOplogProcessor(specs) => {
                    match self.ctx.golem_clients().await? {
                        GolemClients::Oss(clients) => {
                            clients
                                .plugin
                                .create_plugin(&PluginDefinitionCreationDefaultPluginScope {
                                    name: manifest.name,
                                    version: manifest.version,
                                    description: manifest.description,
                                    icon,
                                    homepage: manifest.homepage,
                                    specs,
                                    scope: default_plugin_scope(scope_component_id.as_ref()),
                                })
                                .await
                                .map(|_| ())
                                .map_service_error()?;
                        }
                        GolemClients::Cloud(clients) => clients
                            .plugin
                            .create_plugin(&PluginDefinitionCreationCloudPluginScope {
                                name: manifest.name,
                                version: manifest.version,
                                description: manifest.description,
                                icon,
                                homepage: manifest.homepage,
                                specs,
                                scope: cloud_plugin_scope(
                                    scope_project.as_ref(),
                                    scope_component_id.as_ref(),
                                ),
                            })
                            .await
                            .map(|_| ())
                            .map_service_error()?,
                    }
                }
                Specs::App(wasm) => {
                    let wasm = File::open(&wasm).await.with_context(|| {
                        anyhow!("Failed to open app plugin component: {}", wasm.display())
                    })?;

                    match self.ctx.golem_clients().await? {
                        GolemClients::Oss(clients) => {
                            clients
                                .plugin
                                .create_app_plugin(
                                    &manifest.name,
                                    &manifest.version,
                                    &manifest.description,
                                    icon,
                                    &manifest.homepage,
                                    &default_plugin_scope(scope_component_id.as_ref()),
                                    wasm,
                                )
                                .await
                                .map(|_| ())
                                .map_service_error()?;
                        }
                        GolemClients::Cloud(clients) => clients
                            .plugin
                            .create_app_plugin(
                                &manifest.name,
                                &manifest.version,
                                &manifest.description,
                                icon,
                                &manifest.homepage,
                                &cloud_plugin_scope(
                                    scope_project.as_ref(),
                                    scope_component_id.as_ref(),
                                ),
                                wasm,
                            )
                            .await
                            .map(|_| ())
                            .map_service_error()?,
                    }
                }
                Specs::Library(wasm) => {
                    let wasm = File::open(&wasm).await.with_context(|| {
                        anyhow!(
                            "Failed to open library plugin component: {}",
                            wasm.display()
                        )
                    })?;

                    match self.ctx.golem_clients().await? {
                        GolemClients::Oss(clients) => {
                            clients
                                .plugin
                                .create_library_plugin(
                                    &manifest.name,
                                    &manifest.version,
                                    &manifest.description,
                                    icon,
                                    &manifest.homepage,
                                    &default_plugin_scope(scope_component_id.as_ref()),
                                    wasm,
                                )
                                .await
                                .map(|_| ())
                                .map_service_error()?;
                        }
                        GolemClients::Cloud(clients) => clients
                            .plugin
                            .create_library_plugin(
                                &manifest.name,
                                &manifest.version,
                                &manifest.description,
                                icon,
                                &manifest.homepage,
                                &cloud_plugin_scope(
                                    scope_project.as_ref(),
                                    scope_component_id.as_ref(),
                                ),
                                wasm,
                            )
                            .await
                            .map(|_| ())
                            .map_service_error()?,
                    }
                }
            }
        }

        Ok(())
    }

    async fn cmd_unregister(&self, plugin_name: String, version: String) -> anyhow::Result<()> {
        match self.ctx.golem_clients().await? {
            GolemClients::Oss(client) => client
                .plugin
                .delete_plugin(&plugin_name, &version)
                .await
                .map(|_| ())
                .map_service_error()?,
            GolemClients::Cloud(client) => client
                .plugin
                .delete_plugin(&plugin_name, &version)
                .await
                .map(|_| ())
                .map_service_error()?,
        };

        log_warn_action(
            "Unregistered",
            format!(
                "plugin: {}/{}",
                plugin_name.log_color_highlight(),
                version.log_color_highlight()
            ),
        );

        Ok(())
    }

    async fn get(&self, name: &str, version: &str) -> anyhow::Result<PluginDefinition> {
        match self.ctx.golem_clients().await? {
            GolemClients::Oss(client) => client
                .plugin
                .get_plugin(name, version)
                .await
                .map(PluginDefinition::from)
                .map_service_error(),
            GolemClients::Cloud(client) => client
                .plugin
                .get_plugin(name, version)
                .await
                .map(PluginDefinition::from)
                .map_service_error(),
        }
    }

    async fn resolve_scope(
        &self,
        scope: &PluginScopeArgs,
    ) -> anyhow::Result<(Option<ProjectNameAndId>, Option<ComponentId>)> {
        if scope.is_global() {
            return Ok((None, None));
        }

        let project = match (&scope.account, &scope.project) {
            (Some(account_email), Some(project_name)) => {
                let project = self
                    .ctx
                    .cloud_project_handler()
                    .select_project(&ProjectReference::WithAccount {
                        account_email: account_email.clone(),
                        project_name: project_name.clone(),
                    })
                    .await?;
                Some(project)
            }
            (None, Some(project_name)) => {
                let project = self
                    .ctx
                    .cloud_project_handler()
                    .select_project(&ProjectReference::JustName(project_name.clone()))
                    .await?;
                Some(project)
            }
            _ => None,
        };

        let component_id = match &scope.component {
            Some(component) => {
                self.ctx
                    .component_handler()
                    .component_id_by_name(project.as_ref(), component)
                    .await?
            }
            None => None,
        };

        Ok((project, component_id))
    }
}

fn default_plugin_scope(scope_component_id: Option<&ComponentId>) -> DefaultPluginScope {
    match scope_component_id {
        Some(scope_component_id) => DefaultPluginScope::component(scope_component_id.clone()),
        None => DefaultPluginScope::global(),
    }
}

fn cloud_plugin_scope(
    scope_project: Option<&ProjectNameAndId>,
    scope_component_id: Option<&ComponentId>,
) -> CloudPluginScope {
    if let Some(component_id) = scope_component_id {
        CloudPluginScope::Component(ComponentPluginScope {
            component_id: component_id.clone(),
        })
    } else if let Some(project) = scope_project {
        CloudPluginScope::Project(ProjectPluginScope {
            project_id: golem_common::model::ProjectId(project.project_id.0),
        })
    } else {
        CloudPluginScope::Global(Empty {})
    }
}
