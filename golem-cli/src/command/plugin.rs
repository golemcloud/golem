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

use crate::clients::plugin::PluginClient;
use crate::command::ComponentRefSplit;
use crate::model::plugin_manifest::{
    FromPluginManifest, PluginManifest, PluginTypeSpecificManifest,
};
use crate::model::text::fmt::MessageWithFields;
use crate::model::{
    ComponentIdResolver, ComponentName, Format, GolemError, GolemResult, PathBufOrStdin,
    PluginScopeArgs, PrintRes,
};
use crate::service::component::ComponentService;
use crate::service::project::ProjectResolver;
use async_trait::async_trait;
use clap::Subcommand;
use golem_client::model::{
    ComponentTransformerDefinition, OplogProcessorDefinition, PluginTypeSpecificDefinition,
};
use golem_common::model::{ComponentId, ComponentType};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Subcommand, Debug)]
#[command()]
pub enum PluginSubcommand<PluginScopeRef: clap::Args> {
    /// Creates a new component with a given name by uploading the component WASM
    #[command()]
    List {
        /// The project to list components from
        #[command(flatten)]
        scope: PluginScopeRef,
    },
    /// Get information about a registered plugin
    #[command()]
    Get {
        /// Plugin name
        #[arg(long)]
        plugin_name: String,

        /// Plugin version
        #[arg(long)]
        version: String,
    },
    /// Register a new plugin
    #[command()]
    Register {
        /// The project to list components from
        #[command(flatten)]
        scope: PluginScopeRef,

        /// Path to the plugin manifest JSON
        manifest: PathBuf,

        /// Do not ask for confirmation for performing an update in case the component already exists
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },
    /// Unregister a plugin
    #[command()]
    Unregister {
        /// Plugin name
        #[arg(long)]
        plugin_name: String,

        /// Plugin version
        #[arg(long)]
        version: String,
    },
}

impl<PluginScopeRef: clap::Args> PluginSubcommand<PluginScopeRef> {
    pub async fn handle<
        PluginDefinition: Serialize + MessageWithFields + 'static,
        PluginDefinitionWithoutOwner: FromPluginManifest<PluginScope = PluginScope> + 'static,
        ProjectRef: Send + Sync + 'static,
        PluginScope: Default + Send,
        PluginOwner: Send,
        ProjectContext: Send + Sync,
    >(
        self,
        format: Format,
        client: Arc<
            dyn PluginClient<
                    PluginDefinition = PluginDefinition,
                    PluginDefinitionWithoutOwner = PluginDefinitionWithoutOwner,
                    PluginScope = PluginScope,
                    ProjectContext = ProjectContext,
                > + Send
                + Sync,
        >,
        projects: Arc<dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync>,
        components: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    ) -> Result<GolemResult, GolemError>
    where
        PluginScopeRef: PluginScopeArgs<PluginScope = PluginScope>,
        <PluginScopeRef as PluginScopeArgs>::ComponentRef:
            ComponentRefSplit<ProjectRef> + Send + Sync,
        Vec<PluginDefinition>: PrintRes,
    {
        match self {
            PluginSubcommand::List { scope } => {
                let resolver = Resolver {
                    projects: projects.clone(),
                    components: components.clone(),
                    _phantom: std::marker::PhantomData,
                };
                let scope = scope.into(resolver).await?;
                let plugins = client.list_plugins(scope).await?;
                Ok(GolemResult::Ok(Box::new(plugins)))
            }
            PluginSubcommand::Get {
                plugin_name,
                version,
            } => {
                let plugin = client.get_plugin(&plugin_name, &version).await?;
                Ok(GolemResult::Ok(Box::new(plugin)))
            }
            PluginSubcommand::Register {
                scope,
                manifest,
                non_interactive,
            } => {
                let manifest = std::fs::read_to_string(manifest)
                    .map_err(|err| GolemError(format!("Failed to read plugin manifest: {err}")))?;
                let manifest: PluginManifest = serde_yaml::from_str(&manifest).map_err(|err| {
                    GolemError(format!("Failed to decode plugin manifest: {err}"))
                })?;

                let spec = match &manifest.specs {
                    PluginTypeSpecificManifest::ComponentTransformer(spec) => {
                        PluginTypeSpecificDefinition::ComponentTransformer(
                            ComponentTransformerDefinition {
                                provided_wit_package: spec.provided_wit_package.clone(),
                                json_schema: spec.json_schema.clone(),
                                validate_url: spec.validate_url.clone(),
                                transform_url: spec.transform_url.clone(),
                            },
                        )
                    }
                    PluginTypeSpecificManifest::OplogProcessor(spec) => {
                        let component_name = ComponentName(format!(
                            "oplog_processor:{}:{}",
                            manifest.name, manifest.version
                        ));
                        let component_file = PathBufOrStdin::Path(spec.component.clone());

                        info!("Uploading oplog processor component: {}", component_name);
                        let component = components
                            .add(
                                component_name.clone(),
                                component_file,
                                ComponentType::Durable, // TODO: do we want to support ephemeral oplog processors?
                                None,
                                non_interactive,
                                format,
                                vec![],
                                None,
                            )
                            .await?
                            .into_component();

                        let Some(component) = component else {
                            return Ok(GolemResult::Empty);
                        };

                        debug!(
                            "Uploaded oplog processor component {} as {}/{}",
                            component_name,
                            component.versioned_component_id.component_id,
                            component.versioned_component_id.version
                        );

                        PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
                            component_id: component.versioned_component_id.component_id,
                            component_version: component.versioned_component_id.version,
                        })
                    }
                };

                let icon = std::fs::read(&manifest.icon)
                    .map_err(|err| GolemError(format!("Failed to read plugin icon: {err}")))?;

                let resolver = Resolver {
                    projects: projects.clone(),
                    components: components.clone(),
                    _phantom: std::marker::PhantomData,
                };
                let scope = scope.into(resolver).await?.unwrap_or_default();

                let def: PluginDefinitionWithoutOwner = manifest.into_definition(scope, spec, icon);
                let result = client.register_plugin(def).await?;
                Ok(GolemResult::Ok(Box::new(result)))
            }
            PluginSubcommand::Unregister {
                plugin_name,
                version,
            } => {
                client.unregister_plugin(&plugin_name, &version).await?;
                Ok(GolemResult::Str("Plugin unregistered".to_string()))
            }
        }
    }
}

struct Resolver<ProjectRef, ProjectContext, ComponentRef: ComponentRefSplit<ProjectRef>> {
    projects: Arc<dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync>,
    components: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    _phantom: std::marker::PhantomData<ComponentRef>,
}

#[async_trait]
impl<
        ProjectRef: Send + Sync + 'static,
        ProjectContext: Send + Sync,
        ComponentRef: ComponentRefSplit<ProjectRef> + Send + Sync,
    > ComponentIdResolver<ComponentRef> for Resolver<ProjectRef, ProjectContext, ComponentRef>
{
    async fn resolve(&self, component: ComponentRef) -> Result<ComponentId, GolemError> {
        let (component_name_or_uri, project_ref) = component.split();
        let project_id = self.projects.resolve_id_or_default_opt(project_ref).await?;
        let component_urn = self
            .components
            .resolve_uri(component_name_or_uri, &project_id)
            .await?;
        Ok(component_urn.id)
    }
}
