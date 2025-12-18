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

use crate::command::plugin::PluginSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::plugin_manifest::{PluginManifest, PluginTypeSpecificManifest};
use crate::model::PathBufOrStdin;
use anyhow::{anyhow, Context as AnyhowContext};
use golem_client::api::{ComponentClient, PluginClient};
use golem_client::model::PluginRegistrationCreation;
use golem_common::model::component::ComponentName;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::plugin_registration::{
    ComponentTransformerPluginSpec, OplogProcessorPluginSpec, PluginSpecDto,
};
use golem_common::model::Empty;
use heck::ToKebabCase;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use uuid::Uuid;

pub struct PluginCommandHandler {
    ctx: Arc<Context>,
}

impl PluginCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: PluginSubcommand) -> anyhow::Result<()> {
        match subcommand {
            PluginSubcommand::List => self.cmd_list().await,
            PluginSubcommand::Get { id } => self.cmd_get(id).await,
            PluginSubcommand::Register { manifest } => self.cmd_register(manifest).await,
            PluginSubcommand::Unregister { id } => self.cmd_unregister(id).await,
        }
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let plugin_definitions = clients
            .plugin
            .get_account_plugins(self.ctx.account_id())
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&plugin_definitions);

        Ok(())
    }

    async fn cmd_get(&self, id: Uuid) -> anyhow::Result<()> {
        let client = self.ctx.golem_clients().await?;

        let result = client
            .plugin
            .get_plugin_by_id(&id)
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&result);
        Ok(())
    }

    async fn cmd_register(&self, manifest: PathBufOrStdin) -> anyhow::Result<()> {
        enum Specs {
            ComponentTransformer(ComponentTransformerPluginSpec),
            OplogProcessor(OplogProcessorPluginSpec),
            App(PathBuf),
            Library(PathBuf),
        }

        let clients = self.ctx.golem_clients().await?;

        let manifest = manifest.read_to_string()?;
        let manifest: PluginManifest = serde_yaml::from_str(&manifest)
            .with_context(|| anyhow!("Failed to decode plugin manifest"))?;

        let icon = std::fs::read(&manifest.icon)
            .with_context(|| anyhow!("Failed to read plugin icon: {}", &manifest.icon.display()))?;

        let specs = match &manifest.specs {
            PluginTypeSpecificManifest::ComponentTransformer(spec) => {
                Specs::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: spec.provided_wit_package.clone(),
                    json_schema: spec.json_schema.clone(),
                    validate_url: spec.validate_url.clone(),
                    transform_url: spec.transform_url.clone(),
                })
            }
            PluginTypeSpecificManifest::OplogProcessor(spec) => {
                let component_file = File::open(&spec.component).await.with_context(|| {
                    anyhow!(
                        "Failed to open plugin component WASM at {}",
                        &spec.component.display().to_string().log_color_highlight()
                    )
                })?;

                let component_metadata = ComponentMetadata::analyse_component(
                    &std::fs::read(&spec.component).with_context(|| {
                        anyhow!(
                            "Failed to read plugin component WASM from {}",
                            &spec.component.display().to_string().log_color_highlight()
                        )
                    })?,
                    HashMap::new(),
                    vec![],
                )?;

                let component_name =
                    if let Some(package_name) = component_metadata.root_package_name() {
                        ComponentName(package_name.clone())
                    } else {
                        ComponentName(format!("oplog-processor:{}", manifest.name.to_kebab_case()))
                    };

                let component = {
                    log_action(
                        "Uploading",
                        format!("oplog processor component: {component_name}"),
                    );
                    let _indent = LogIndent::new();

                    let clients = self.ctx.golem_clients().await?;

                    // TODO: already existing is not handled here, let's do that when we make it part of the manifest
                    let component = clients
                        .component
                        .create_component(
                            &ComponentQuery {
                                project_id: scope_project.as_ref().map(|p| p.project_id.0),
                                component_name: component_name.0.clone(),
                            },
                            component_file,
                            None,
                            None::<File>,
                            None,
                            None, // TODO: component env
                            None,
                        )
                        .await
                        .map(Component::from)
                        .map_service_error()?;

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
                Specs::ComponentTransformer(spec) => clients
                    .plugin
                    .create_plugin(
                        self.ctx.account_id(),
                        &PluginRegistrationCreation {
                            name: manifest.name,
                            version: manifest.version,
                            description: manifest.description,
                            icon,
                            homepage: manifest.homepage,
                            spec: PluginSpecDto::ComponentTransformer(spec),
                        },
                        None::<&[u8]>,
                    )
                    .await
                    .map(|_| ())
                    .map_service_error()?,
                Specs::OplogProcessor(spec) => clients
                    .plugin
                    .create_plugin(
                        self.ctx.account_id(),
                        &PluginRegistrationCreation {
                            name: manifest.name,
                            version: manifest.version,
                            description: manifest.description,
                            icon,
                            homepage: manifest.homepage,
                            spec: PluginSpecDto::OplogProcessor(spec),
                        },
                        None::<&[u8]>,
                    )
                    .await
                    .map(|_| ())
                    .map_service_error()?,
                Specs::App(wasm) => {
                    let wasm = File::open(&wasm).await.with_context(|| {
                        anyhow!("Failed to open app plugin component: {}", wasm.display())
                    })?;

                    let clients = self.ctx.golem_clients().await?;

                    clients
                        .plugin
                        .create_plugin(
                            self.ctx.account_id(),
                            &PluginRegistrationCreation {
                                name: manifest.name,
                                version: manifest.version,
                                description: manifest.description,
                                icon,
                                homepage: manifest.homepage,
                                spec: PluginSpecDto::App(Empty {}),
                            },
                            wasm,
                        )
                        .await
                        .map(|_| ())
                        .map_service_error()?
                }
                Specs::Library(wasm) => {
                    let wasm = File::open(&wasm).await.with_context(|| {
                        anyhow!(
                            "Failed to open library plugin component: {}",
                            wasm.display()
                        )
                    })?;

                    let clients = self.ctx.golem_clients().await?;

                    clients
                        .plugin
                        .create_plugin(
                            self.ctx.account_id(),
                            &PluginRegistrationCreation {
                                name: manifest.name,
                                version: manifest.version,
                                description: manifest.description,
                                icon,
                                homepage: manifest.homepage,
                                spec: PluginSpecDto::Library(Empty {}),
                            },
                            wasm,
                        )
                        .await
                        .map(|_| ())
                        .map_service_error()?
                }
            }
        }

        Ok(())
    }

    async fn cmd_unregister(&self, id: Uuid) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .plugin
            .delete_plugin(&id)
            .await
            .map_service_error()?;

        log_warn_action(
            "Unregistered",
            format!(
                "plugin: {}/{}",
                result.name.log_color_highlight(),
                result.version.log_color_highlight()
            ),
        );

        Ok(())
    }
}
