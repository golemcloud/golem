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

use crate::command::ComponentRefSplit;
use crate::model::app_ext::GolemComponentExtensions;
use crate::model::text::component::ComponentAddView;
use crate::model::{
    ComponentName, Format, GolemError, GolemResult, PathBufOrStdin, WorkerUpdateMode,
};
use crate::parse_key_val;
use crate::service::component::ComponentService;
use crate::service::deploy::DeployService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;
use golem_client::model::ComponentType;
use golem_common::model::PluginInstallationId;
use golem_wasm_rpc_stubgen::commands::app::{ApplicationContext, ApplicationSourceMode, Config};
use golem_wasm_rpc_stubgen::log::Output;
use golem_wasm_rpc_stubgen::model::app;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubCommand<ProjectRef: clap::Args, ComponentRef: clap::Args> {
    /// Creates a new component by uploading the component WASM.
    ///
    /// If neither `component-file` nor `app` is specified, the command will look for the manifest in the current directory and all parent directories.
    /// It will then look for the component in the manifest and use the settings there.
    #[command(alias = "create", verbatim_doc_comment)]
    Add {
        /// The WASM file to be used as a Golem component
        ///
        /// Conflics with `app` flag.
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: Option<PathBufOrStdin>, // TODO: validate exists

        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Name of the newly created component
        ///
        /// If 'component-file' is specified, this flag controls the name of the component.
        /// If 'component-file' is not specified, or 'app' is specified, this flag is used to resolve the component from the app manifest.
        #[arg(short, long, verbatim_doc_comment)]
        component_name: ComponentName,

        /// The component type. If none specified, the command creates a Durable component.
        ///
        /// Conflicts with `app` flag.
        #[command(flatten, verbatim_doc_comment)]
        component_type: ComponentTypeArg,

        /// Application manifest to use. Can be specified multiple times.
        /// The component-name flag is used to resolve the component from the app manifest.
        /// Other settings are then taken from the app manifest.
        ///
        /// Conflicts with `component_file` flag.
        /// Conflicts with `ephemeral` flag.
        /// Conflicts with `durable` flag.
        #[arg(
            long,
            short,
            conflicts_with_all = vec!["component_file", "component-type-flag"],
            verbatim_doc_comment
        )]
        app: Vec<PathBuf>,

        /// Select build profile, only effective when application manifest is used
        #[arg(long, short)]
        build_profile: Option<String>,

        /// Do not ask for confirmation for performing an update in case the component already exists
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },
    /// Updates an existing component by uploading a new version of its WASM
    ///
    /// If neither `component-file` nor `app` is specified, the command will look for the manifest in the current directory and all parent directories.
    /// It will then look for the component in the manifest and use the settings there.
    #[command()]
    Update {
        /// The WASM file to be used as a new version of the Golem component
        ///
        /// Conflics with `app` flag.
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath, verbatim_doc_comment
        )]
        component_file: Option<PathBufOrStdin>, // TODO: validate exists

        /// The component to update
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The updated component's type. If none specified, the previous version's type is used.
        ///
        /// Conflicts with `app` flag.
        #[command(flatten, verbatim_doc_comment)]
        component_type: UpdatedComponentTypeArg,

        /// Application manifest to use. Can be specified multiple times.
        /// The component-name flag is used to resolve the component from the app manifest.
        /// Other settings are then taken from the app manifest.
        ///
        /// Conflicts with `component-file` flag.
        /// Conflicts with `ephemeral` flag.
        /// Conflicts with `durable` flag.
        #[arg(long, short)]
        app: Vec<PathBuf>,

        /// Select build profile, only effective when application manifest is used
        #[arg(long, short)]
        build_profile: Option<String>,

        /// Try to automatically update all existing workers to the new version
        #[arg(long, default_value_t = false)]
        try_update_workers: bool,

        /// Update mode - auto or manual
        #[arg(long, default_value = "auto", requires = "try_update_workers")]
        update_mode: WorkerUpdateMode,

        /// Do not ask for confirmation for creating a new component in case it does not exist
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },
    /// Lists the existing components
    #[command()]
    List {
        /// The project to list components from
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Optionally look for only components matching a given name
        #[arg(short, long)]
        component_name: Option<ComponentName>,
    },
    /// Get component
    #[command()]
    Get {
        /// The Golem component
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The version of the component
        #[arg(short = 't', long)]
        version: Option<u64>,
    },
    /// Try to automatically update all existing workers to the latest version
    #[command()]
    TryUpdateWorkers {
        /// The component to redeploy
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Update mode - auto or manual
        #[arg(long, default_value = "auto")]
        update_mode: WorkerUpdateMode,
    },
    /// Redeploy all workers of a component using the latest version
    #[command()]
    Redeploy {
        /// The component to redeploy
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// Do not ask for confirmation
        #[arg(short = 'y', long)]
        non_interactive: bool,
    },
    /// Install a plugin for this component
    #[command()]
    InstallPlugin {
        /// The component to install the plugin for
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The plugin to install
        #[arg(long)]
        plugin_name: String,

        /// The version of the plugin to install
        #[arg(long)]
        plugin_version: String,

        /// Priority of the plugin - largest priority is applied first
        #[arg(long)]
        priority: i32,

        /// List of parameters (key-value pairs) passed to the plugin
        #[arg(short, long, value_parser = parse_key_val, value_name = "KEY=VAL")]
        parameter: Vec<(String, String)>,
    },
    /// Get the installed plugins of the component
    #[command()]
    GetInstallations {
        /// The component to list plugin installations for
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The version of the component
        #[arg(short = 't', long)]
        version: Option<u64>,
    },
    /// Uninstall a plugin for this component
    #[command()]
    UninstallPlugin {
        /// The component to install the plugin for
        #[command(flatten)]
        component_name_or_uri: ComponentRef,

        /// The plugin to install
        #[arg(long)]
        installation_id: PluginInstallationId,
    },
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct ComponentTypeArg {
    /// Create an Ephemeral component. If not specified, the command creates a Durable component.
    #[arg(long, group = "component-type-flag")]
    ephemeral: bool,

    /// Create a Durable component. This is the default.
    #[arg(long, group = "component-type-flag")]
    durable: bool,
}

impl ComponentTypeArg {
    pub fn component_type(&self) -> ComponentType {
        if self.ephemeral {
            ComponentType::Ephemeral
        } else {
            ComponentType::Durable
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct UpdatedComponentTypeArg {
    /// Create an Ephemeral component. If not specified, the previous version's type will be used.
    #[arg(long, group = "component-type-flag")]
    ephemeral: bool,

    /// Create a Durable component. If not specified, the previous version's type will be used.
    #[arg(long, group = "component-type-flag")]
    durable: bool,
}

impl UpdatedComponentTypeArg {
    pub fn optional_component_type(&self) -> Option<ComponentType> {
        if self.ephemeral {
            Some(ComponentType::Ephemeral)
        } else if self.durable {
            Some(ComponentType::Durable)
        } else {
            None
        }
    }
}

impl<
        ProjectRef: clap::Args + Send + Sync + 'static,
        ComponentRef: ComponentRefSplit<ProjectRef> + clap::Args,
    > ComponentSubCommand<ProjectRef, ComponentRef>
{
    pub async fn handle<ProjectContext: Clone + Send + Sync>(
        self,
        format: Format,
        service: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
        deploy_service: Arc<dyn DeployService<ProjectContext = ProjectContext> + Send + Sync>,
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ComponentSubCommand::Add {
                project_ref,
                component_name,
                component_file: Some(component_file),
                component_type,
                app: _,
                build_profile: _,
                non_interactive,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                let component = service
                    .add(
                        component_name,
                        component_file,
                        component_type.component_type(),
                        Some(project_id),
                        non_interactive,
                        format,
                        vec![],
                    )
                    .await?;
                Ok(GolemResult::Ok(Box::new(ComponentAddView(
                    component.into(),
                ))))
            }
            ComponentSubCommand::Add {
                project_ref,
                component_name,
                component_file: None,
                component_type: _,
                app,
                build_profile,
                non_interactive,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;

                let ctx = ApplicationComponentContext::new(
                    app,
                    build_profile.map(|profile| profile.into()),
                    &component_name.0,
                )?;

                let component = service
                    .add(
                        component_name,
                        PathBufOrStdin::Path(ctx.linked_wasm),
                        ctx.extensions.component_type,
                        Some(project_id),
                        non_interactive,
                        format,
                        ctx.extensions.files,
                    )
                    .await?;
                Ok(GolemResult::Ok(Box::new(ComponentAddView(
                    component.into(),
                ))))
            }
            ComponentSubCommand::Update {
                component_name_or_uri,
                component_file: Some(component_file),
                component_type,
                app: _,
                build_profile: _,
                try_update_workers,
                update_mode,
                non_interactive,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                let mut result = service
                    .update(
                        component_name_or_uri.clone(),
                        component_file,
                        component_type.optional_component_type(),
                        project_id.clone(),
                        non_interactive,
                        format,
                        vec![],
                    )
                    .await?;

                if try_update_workers {
                    let deploy_result = deploy_service
                        .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                        .await?;
                    result = result.merge(deploy_result);
                }
                Ok(result)
            }
            ComponentSubCommand::Update {
                component_name_or_uri,
                non_interactive,
                component_file: None,
                component_type: _,
                app,
                build_profile,
                try_update_workers,
                update_mode,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();

                let component_name = service
                    .resolve_component_name(&component_name_or_uri)
                    .await?;

                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;

                let ctx = ApplicationComponentContext::new(
                    app,
                    build_profile.map(|profile| profile.into()),
                    &component_name,
                )?;

                let mut result = service
                    .update(
                        component_name_or_uri.clone(),
                        PathBufOrStdin::Path(ctx.linked_wasm),
                        Some(ctx.extensions.component_type),
                        project_id.clone(),
                        non_interactive,
                        format,
                        ctx.extensions.files,
                    )
                    .await?;

                if try_update_workers {
                    let deploy_result = deploy_service
                        .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                        .await?;
                    result = result.merge(deploy_result);
                }
                Ok(result)
            }
            ComponentSubCommand::List {
                project_ref,
                component_name,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                service.list(component_name, Some(project_id)).await
            }
            ComponentSubCommand::Get {
                component_name_or_uri,
                version,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .get(component_name_or_uri, version, project_id)
                    .await
            }
            ComponentSubCommand::TryUpdateWorkers {
                component_name_or_uri,
                update_mode,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                deploy_service
                    .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                    .await
            }
            ComponentSubCommand::Redeploy {
                component_name_or_uri,
                non_interactive,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                deploy_service
                    .redeploy(component_name_or_uri, project_id, non_interactive, format)
                    .await
            }
            ComponentSubCommand::InstallPlugin {
                component_name_or_uri,
                plugin_name,
                plugin_version: version,
                priority,
                parameter,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .install_plugin(
                        component_name_or_uri,
                        project_id,
                        &plugin_name,
                        &version,
                        priority,
                        HashMap::from_iter(parameter),
                    )
                    .await
            }
            ComponentSubCommand::GetInstallations {
                component_name_or_uri,
                version,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .get_installations(component_name_or_uri, project_id, version)
                    .await
            }
            ComponentSubCommand::UninstallPlugin {
                component_name_or_uri,
                installation_id,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                service
                    .uninstall_plugin(component_name_or_uri, project_id, &installation_id)
                    .await
            }
        }
    }
}

fn app_ctx(
    sources: Vec<PathBuf>,
    build_profile: Option<app::ProfileName>,
) -> Result<ApplicationContext<GolemComponentExtensions>, GolemError> {
    Ok(ApplicationContext::new(Config {
        app_resolve_mode: {
            if sources.is_empty() {
                ApplicationSourceMode::Automatic
            } else {
                ApplicationSourceMode::Explicit(sources)
            }
        },
        skip_up_to_date_checks: false,
        profile: build_profile,
        offline: false,
        extensions: PhantomData::<GolemComponentExtensions>,
        log_output: Output::None,
        steps_filter: HashSet::new(),
    })?)
}

struct ApplicationComponentContext {
    #[allow(dead_code)]
    build_profile: Option<app::ProfileName>,
    #[allow(dead_code)]
    app_ctx: ApplicationContext<GolemComponentExtensions>,
    #[allow(dead_code)]
    name: app::ComponentName,
    linked_wasm: PathBuf,
    extensions: GolemComponentExtensions,
}

impl ApplicationComponentContext {
    fn new(
        sources: Vec<PathBuf>,
        build_profile: Option<app::ProfileName>,
        component_name: &str,
    ) -> Result<Self, GolemError> {
        let app_ctx = app_ctx(sources, build_profile.clone())?;
        let name = app::ComponentName::from(component_name.to_string());

        if !app_ctx.application.component_names().contains(&name) {
            return Err(GolemError(format!(
                "Component {} not found in application manifest",
                name
            )));
        }

        let linked_wasm = app_ctx
            .application
            .component_linked_wasm(&name, build_profile.as_ref());

        let component_properties = app_ctx
            .application
            .component_properties(&name, build_profile.as_ref());
        let extensions = component_properties.extensions.as_ref().unwrap().clone();

        Ok(ApplicationComponentContext {
            build_profile,
            app_ctx,
            name,
            linked_wasm,
            extensions,
        })
    }
}
