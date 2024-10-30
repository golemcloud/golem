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

use crate::clients::plugin::PluginClient;
use crate::command::ComponentRefSplit;
use crate::model::{
    ComponentIdResolver, Format, GolemError, GolemResult, PluginScopeArgs, PrintRes,
};
use crate::service::component::ComponentService;
use crate::service::project::ProjectResolver;
use async_trait::async_trait;
use clap::Subcommand;
use golem_common::model::ComponentId;
use serde::Serialize;
use std::sync::Arc;

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
}

impl<PluginScopeRef: clap::Args> PluginSubcommand<PluginScopeRef> {
    pub async fn handle<
        PluginDefinition: Serialize + 'static,
        ProjectRef: Send + Sync + 'static,
        PluginScope: Send,
        ProjectContext: Send + Sync,
    >(
        self,
        _format: Format,
        client: Arc<
            dyn PluginClient<
                    PluginDefinition = PluginDefinition,
                    PluginScope = PluginScope,
                    ProjectContext = ProjectContext,
                > + Send
                + Sync,
        >,
        projects: Arc<dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync>,
        components: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    ) -> Result<GolemResult, GolemError>
    where
        Vec<PluginDefinition>: PrintRes,
        PluginScopeRef: PluginScopeArgs<PluginScope = PluginScope>,
        <PluginScopeRef as PluginScopeArgs>::ComponentRef:
            ComponentRefSplit<ProjectRef> + Send + Sync,
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
