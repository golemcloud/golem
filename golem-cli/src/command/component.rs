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

use crate::command::{ComponentRefSplit, ComponentRefsSplit};
use crate::model::app_ext::GolemComponentExtensions;
use crate::model::component::ComponentUpsertResult;
use crate::model::text::component::{ComponentAddView, ComponentUpdateView};
use crate::model::{
    ComponentName, Format, GolemError, GolemResult, PathBufOrStdin, PrintRes, WorkerUpdateMode,
};
use crate::parse_key_val;
use crate::service::component::ComponentService;
use crate::service::deploy::DeployService;
use crate::service::project::ProjectResolver;
use clap::Subcommand;
use futures_util::future::join_all;
use golem_client::model::{
    ComponentType, DynamicLinkedInstance, DynamicLinkedWasmRpc, DynamicLinking,
};
use golem_common::model::PluginInstallationId;
use golem_common::uri::oss::uri::ComponentUri;
use golem_common::uri::oss::url::ComponentUrl;
use golem_wasm_rpc_stubgen::commands::app::{
    ApplicationContext, ApplicationSourceMode, ComponentSelectMode, Config,
};
use golem_wasm_rpc_stubgen::log::Output;
use golem_wasm_rpc_stubgen::model::app;
use golem_wasm_rpc_stubgen::model::app::DependencyType;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ComponentSubCommand<
    ProjectRef: clap::Args,
    ComponentRef: clap::Args,
    ComponentRefs: clap::Args,
> {
    /// Creates a new component by uploading the component WASM.
    ///
    /// If neither `component-file` nor `app` is specified, the command will look for the manifest in the current directory and all parent directories.
    /// It will then look for the component in the manifest and use the settings there.
    #[command(alias = "create", verbatim_doc_comment)]
    Add {
        /// The WASM file to be used as a Golem component
        ///
        /// Conflicts with `app` flag.
        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: Option<PathBufOrStdin>, // TODO: validate exists

        /// The newly created component's owner project
        #[command(flatten)]
        project_ref: ProjectRef,

        /// Name of the newly created component
        ///
        /// If 'component-file' is specified, this flag specifies the name for the component.
        /// If 'component-file' is not specified, or 'app' is specified,
        /// this flag is used to resolve the component from the app manifest,
        /// in this case multiple component can be defined
        #[arg(short, long, verbatim_doc_comment)]
        component_name: Vec<ComponentName>,

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

        /// Component(s) to update
        #[command(flatten)]
        component_names_or_uris: ComponentRefs,

        /// The updated component's type. If none specified, the previous version's type is used.
        ///
        /// Conflicts with `app` flag.
        #[command(flatten, verbatim_doc_comment)]
        component_type: UpdatedComponentTypeArg,

        /// Application manifest to use. Can be specified multiple times.
        /// The component-name flag can be used to select component(s) from the app manifest.
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
        ComponentRefs: ComponentRefsSplit<ProjectRef> + clap::Args,
    > ComponentSubCommand<ProjectRef, ComponentRef, ComponentRefs>
{
    pub async fn handle<ProjectContext: Clone + Send + Sync>(
        self,
        format: Format,
        component_service: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
        deploy_service: Arc<dyn DeployService<ProjectContext = ProjectContext> + Send + Sync>,
        projects: &(dyn ProjectResolver<ProjectRef, ProjectContext> + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            ComponentSubCommand::Add {
                project_ref,
                mut component_name,
                component_file: Some(component_file),
                component_type,
                app: _,
                build_profile: _,
                non_interactive,
            } => {
                if component_name.len() != 1 {
                    return errors::expected_one_component_name_with_component_file();
                }
                let component_name = component_name.swap_remove(0);

                let project_id = projects.resolve_id_or_default(project_ref).await?;
                Ok(component_service
                    .add(
                        component_name,
                        component_file,
                        component_type.component_type(),
                        Some(project_id),
                        non_interactive,
                        format,
                        vec![],
                        None,
                    )
                    .await?
                    .to_golem_result())
            }
            ComponentSubCommand::Add {
                project_ref,
                component_name: component_names,
                component_file: None,
                component_type: _,
                app,
                build_profile,
                non_interactive,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;

                let mut ctx = ApplicationComponentContext::new(
                    format,
                    app,
                    build_profile.map(|profile| profile.into()),
                    component_names.into_iter().map(|name| name.0).collect(),
                )?;

                let component_names = ctx.selected_component_names().clone();
                if component_names.is_empty() {
                    return errors::no_components_found();
                }

                for component_name in &component_names {
                    let dynamic_linking = ctx.dynamic_linking(component_name)?;
                    let extensions = ctx.component_extensions(component_name);
                    component_service
                        .add(
                            ComponentName(component_name.to_string()),
                            PathBufOrStdin::Path(ctx.component_linked_wasm_rpc(component_name)),
                            extensions.component_type,
                            Some(project_id.clone()),
                            non_interactive,
                            format,
                            extensions.files.clone(),
                            dynamic_linking,
                        )
                        .await?
                        .to_golem_result()
                        .streaming_print(format);
                }

                Ok(GolemResult::Empty)
            }
            ComponentSubCommand::Update {
                component_names_or_uris,
                component_file: Some(component_file),
                component_type,
                app: _,
                build_profile: _,
                try_update_workers,
                update_mode,
                non_interactive,
            } => {
                let Some(split) = component_names_or_uris.split() else {
                    return errors::all_component_uris_must_use_the_same_project_id();
                };
                let (mut component_names_or_uris, project_ref) = split;

                if component_names_or_uris.len() != 1 {
                    return errors::expected_one_component_name_with_component_file();
                }

                let component_name_or_uri = component_names_or_uris.swap_remove(0);

                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                component_service
                    .update(
                        component_name_or_uri.clone(),
                        component_file,
                        component_type.optional_component_type(),
                        project_id.clone(),
                        non_interactive,
                        format,
                        vec![],
                        None,
                    )
                    .await?
                    .to_golem_result()
                    .streaming_print(format);

                if try_update_workers {
                    deploy_service
                        .try_update_all_workers(component_name_or_uri, project_id, update_mode)
                        .await?
                        .streaming_print(format);
                }

                Ok(GolemResult::Empty)
            }
            ComponentSubCommand::Update {
                component_names_or_uris,
                non_interactive,
                component_file: None,
                component_type: _,
                app,
                build_profile,
                try_update_workers,
                update_mode,
            } => {
                let Some(split) = component_names_or_uris.split() else {
                    return errors::all_component_uris_must_use_the_same_project_id();
                };
                let (component_names_or_uris, project_ref) = split;

                let component_names_to_uris =
                    resolve_component_names(component_service.clone(), component_names_or_uris)
                        .await?;

                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;

                let mut ctx = ApplicationComponentContext::new(
                    format,
                    app,
                    build_profile.map(|profile| profile.into()),
                    component_names_to_uris.keys().cloned().collect(),
                )?;

                let component_names = ctx.selected_component_names().clone();
                if component_names.is_empty() {
                    return errors::no_components_found();
                }

                let component_names_to_uris = {
                    let mut component_names_to_uris = component_names_to_uris;
                    component_names_to_uris.extend(
                        resolve_component_names(
                            component_service.clone(),
                            component_names
                                .iter()
                                .filter(|component_name| {
                                    !component_names_to_uris.contains_key(component_name.as_str())
                                })
                                .map(|component_name| {
                                    ComponentUri::URL(ComponentUrl {
                                        name: component_name.to_string(),
                                    })
                                })
                                .collect(),
                        )
                        .await?,
                    );
                    component_names_to_uris
                };

                for component_name in &component_names {
                    let dynamic_linking = ctx.dynamic_linking(component_name)?;
                    let extensions = ctx.component_extensions(component_name);
                    component_service
                        .update(
                            component_names_to_uris
                                .get(component_name.as_str())
                                .expect("Failed to get component uri by name")
                                .clone(),
                            PathBufOrStdin::Path(ctx.component_linked_wasm_rpc(component_name)),
                            Some(extensions.component_type),
                            project_id.clone(),
                            non_interactive,
                            format,
                            extensions.files.clone(),
                            dynamic_linking,
                        )
                        .await?
                        .to_golem_result()
                        .streaming_print(format);

                    if try_update_workers {
                        deploy_service
                            .try_update_all_workers(
                                component_names_to_uris
                                    .get(component_name.as_str())
                                    .expect("Failed to get component uri by name")
                                    .clone(),
                                project_id.clone(),
                                update_mode.clone(),
                            )
                            .await?
                            .streaming_print(format);
                    }
                }

                Ok(GolemResult::Empty)
            }
            ComponentSubCommand::List {
                project_ref,
                component_name,
            } => {
                let project_id = projects.resolve_id_or_default(project_ref).await?;
                component_service
                    .list(component_name, Some(project_id))
                    .await
            }
            ComponentSubCommand::Get {
                component_name_or_uri,
                version,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                component_service
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
                component_service
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
                component_service
                    .get_installations(component_name_or_uri, project_id, version)
                    .await
            }
            ComponentSubCommand::UninstallPlugin {
                component_name_or_uri,
                installation_id,
            } => {
                let (component_name_or_uri, project_ref) = component_name_or_uri.split();
                let project_id = projects.resolve_id_or_default_opt(project_ref).await?;
                component_service
                    .uninstall_plugin(component_name_or_uri, project_id, &installation_id)
                    .await
            }
        }
    }
}

trait ToGolemResult {
    fn to_golem_result(self) -> GolemResult;
}

impl ToGolemResult for ComponentUpsertResult {
    fn to_golem_result(self) -> GolemResult {
        match self {
            ComponentUpsertResult::Skipped => GolemResult::Empty,
            ComponentUpsertResult::Added(component) => {
                GolemResult::Ok(Box::new(ComponentAddView(component.into())))
            }
            ComponentUpsertResult::Updated(component) => {
                GolemResult::Ok(Box::new(ComponentUpdateView(component.into())))
            }
        }
    }
}

fn app_ctx(
    format: Format,
    sources: Vec<PathBuf>,
    component_names: Vec<String>,
    build_profile: Option<app::ProfileName>,
) -> Result<ApplicationContext<GolemComponentExtensions>, GolemError> {
    Ok(ApplicationContext::new(Config {
        app_source_mode: {
            if sources.is_empty() {
                ApplicationSourceMode::Automatic
            } else {
                ApplicationSourceMode::Explicit(sources)
            }
        },
        component_select_mode: {
            if component_names.is_empty() {
                ComponentSelectMode::CurrentDir
            } else {
                ComponentSelectMode::Explicit(
                    component_names
                        .into_iter()
                        .map(|component_name| component_name.to_string().into())
                        .collect(),
                )
            }
        },
        skip_up_to_date_checks: false,
        profile: build_profile,
        offline: false,
        extensions: PhantomData::<GolemComponentExtensions>,
        log_output: match format {
            Format::Json => Output::None,
            Format::Yaml => Output::None,
            Format::Text => Output::Stdout,
        },
        steps_filter: HashSet::new(),
    })?)
}

struct ApplicationComponentContext {
    application_context: ApplicationContext<GolemComponentExtensions>,
    build_profile: Option<app::ProfileName>,
}

impl ApplicationComponentContext {
    fn new(
        format: Format,
        sources: Vec<PathBuf>,
        build_profile: Option<app::ProfileName>,
        component_names: Vec<String>,
    ) -> Result<Self, GolemError> {
        Ok(ApplicationComponentContext {
            application_context: app_ctx(format, sources, component_names, build_profile.clone())?,
            build_profile,
        })
    }

    fn component_linked_wasm_rpc(&self, component_name: &app::ComponentName) -> PathBuf {
        self.application_context
            .application
            .component_linked_wasm(component_name, self.build_profile.as_ref())
    }

    fn component_extensions(
        &self,
        component_name: &app::ComponentName,
    ) -> &GolemComponentExtensions {
        &self
            .application_context
            .application
            .component_properties(component_name, self.build_profile.as_ref())
            .extensions
    }

    fn selected_component_names(&self) -> &BTreeSet<app::ComponentName> {
        self.application_context.selected_component_names()
    }

    fn dynamic_linking(
        &mut self,
        component_name: &app::ComponentName,
    ) -> Result<Option<DynamicLinking>, GolemError> {
        let mut mapping = Vec::new();

        let wasm_rpc_deps = self
            .application_context
            .application
            .component_wasm_rpc_dependencies(component_name)
            .iter()
            .filter(|dep| dep.dep_type == DependencyType::DynamicWasmRpc)
            .cloned()
            .collect::<Vec<_>>();

        for wasm_rpc_dep in wasm_rpc_deps {
            let ifaces = self
                .application_context
                .component_stub_interfaces(&wasm_rpc_dep.name)
                .map_err(|err| GolemError(err.to_string()))?;

            mapping.push(ifaces);
        }

        if mapping.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DynamicLinking {
                dynamic_linking: HashMap::from_iter(mapping.into_iter().map(|stub_interfaces| {
                    (
                        stub_interfaces.stub_interface_name,
                        DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                            target_interface_name: HashMap::from_iter(
                                stub_interfaces.exported_interfaces_per_stub_resource,
                            ),
                        }),
                    )
                })),
            }))
        }
    }
}

async fn resolve_component_names<ProjectContext>(
    component_service: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    component_names_or_uris: Vec<ComponentUri>,
) -> Result<BTreeMap<String, ComponentUri>, GolemError>
where
    ProjectContext: Clone + Send + Sync,
{
    join_all(
        component_names_or_uris
            .into_iter()
            .map(|component_name_or_uri| async {
                (
                    component_service
                        .resolve_component_name(&component_name_or_uri)
                        .await,
                    component_name_or_uri,
                )
            }),
    )
    .await
    .into_iter()
    .map(|(component_name, component_name_or_uri)| {
        component_name.map(|component_name| (component_name, component_name_or_uri))
    })
    .collect::<Result<BTreeMap<_, _>, _>>()
}

mod errors {
    use crate::model::{GolemError, GolemResult};

    pub fn all_component_uris_must_use_the_same_project_id() -> Result<GolemResult, GolemError> {
        Err(GolemError(
            "All component URIs must use the same project id".to_string(),
        ))
    }

    pub fn expected_one_component_name_with_component_file() -> Result<GolemResult, GolemError> {
        Err(GolemError(
            "When component file is specified then exactly one component name is expected"
                .to_string(),
        ))
    }

    pub fn no_components_found() -> Result<GolemResult, GolemError> {
        Err(GolemError("No components found".to_string()))
    }
}
