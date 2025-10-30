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

use crate::app::build::build_app;
use crate::app::build::clean::clean_app;
use crate::app::build::command::execute_custom_command;
use crate::app::error::{format_warns, AppValidationError, CustomCommandError};
use crate::app::remote_components::RemoteComponents;
use crate::config::ProfileName;
use crate::fs::{compile_and_collect_globs, PathExtra};
use crate::log::{log_action, logln, LogColorize, LogIndent, LogOutput, Output};
use crate::model::app::{
    includes_from_yaml_file, AppComponentName, Application, ApplicationComponentSelectMode,
    ApplicationConfig, ApplicationSourceMode, BinaryComponentSource, BuildProfileName,
    ComponentStubInterfaces, DependentComponent, DynamicHelpSections, DEFAULT_CONFIG_FILE_NAME,
};
use crate::model::app_raw;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::stub::{StubConfig, StubDefinition};
use crate::wasm_rpc_stubgen::wit_resolve::{ResolvedWitApplication, WitDepsResolver};
use anyhow::{anyhow, bail, Context};
use colored::control::SHOULD_COLORIZE;
use colored::Colorize;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct ApplicationContext {
    pub loaded_with_warnings: bool,
    pub config: ApplicationConfig,
    pub application: Application,
    pub wit: ResolvedWitApplication,
    pub calling_working_dir: PathBuf,
    component_stub_defs: HashMap<AppComponentName, StubDefinition>,
    common_wit_deps: OnceLock<anyhow::Result<WitDepsResolver>>,
    component_generated_base_wit_deps: HashMap<AppComponentName, WitDepsResolver>,
    selected_component_names: BTreeSet<AppComponentName>,
    remote_components: RemoteComponents,
    pub tools_with_ensured_common_deps: ToolsWithEnsuredCommonDeps,
}

pub struct ToolsWithEnsuredCommonDeps {
    tools_with_ensured_common_deps: tokio::sync::RwLock<HashSet<String>>,
}

impl Default for ToolsWithEnsuredCommonDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolsWithEnsuredCommonDeps {
    pub fn new() -> ToolsWithEnsuredCommonDeps {
        ToolsWithEnsuredCommonDeps {
            tools_with_ensured_common_deps: tokio::sync::RwLock::new(HashSet::new()),
        }
    }

    pub async fn ensure_common_deps_for_tool_once(
        &self,
        tool: &str,
        ensure: impl AsyncFnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        if self
            .tools_with_ensured_common_deps
            .read()
            .await
            .contains(tool)
        {
            return Ok(());
        }

        let mut lock = self.tools_with_ensured_common_deps.write().await;

        let result = ensure().await;

        if result.is_ok() {
            lock.insert(tool.to_string());
        }

        result
    }
}

pub struct ApplicationPreloadResult {
    pub source_mode: ApplicationSourceMode,
    pub loaded_with_warnings: bool,
    pub profiles: Option<BTreeMap<ProfileName, app_raw::Profile>>,
}

impl ApplicationContext {
    pub fn preload_sources_and_get_profiles(
        source_mode: ApplicationSourceMode,
    ) -> anyhow::Result<ApplicationPreloadResult> {
        let _output = LogOutput::new(Output::None);

        match load_profiles(source_mode) {
            Some(profiles) => to_anyhow(
                "Failed to load application manifest profiles, see problems above",
                profiles,
                Some(|mut profiles| {
                    profiles.loaded_with_warnings = true;
                    profiles
                }),
            ),
            None => Ok(ApplicationPreloadResult {
                source_mode: ApplicationSourceMode::None,
                loaded_with_warnings: false,
                profiles: None,
            }),
        }
    }

    pub async fn new(
        available_profiles: &BTreeSet<ProfileName>,
        source_mode: ApplicationSourceMode,
        config: ApplicationConfig,
        file_download_client: reqwest::Client,
    ) -> anyhow::Result<Option<ApplicationContext>> {
        let Some(app_and_calling_working_dir) = load_app(available_profiles, source_mode) else {
            return Ok(None);
        };

        let ctx = to_anyhow(
            "Failed to load application manifest, see problems above",
            Self::create_context(app_and_calling_working_dir, config, file_download_client).await,
            Some(|mut app_ctx| {
                app_ctx.loaded_with_warnings = true;
                app_ctx
            }),
        )?;

        ctx.validate_build_profile()?;

        if ctx.config.offline {
            log_action("Using", "offline mode");
        }

        Ok(Some(ctx))
    }

    async fn create_context(
        app_and_calling_working_dir: ValidatedResult<(Application, PathBuf)>,
        config: ApplicationConfig,
        file_download_client: reqwest::Client,
    ) -> ValidatedResult<ApplicationContext> {
        let mut result = ValidatedResult::Ok(());

        let (r, v) = result.merge_and_get(app_and_calling_working_dir);
        result = r;
        if let Some((application, calling_working_dir)) = v {
            let tools_with_ensured_common_deps = ToolsWithEnsuredCommonDeps::new();
            let wit_result = ResolvedWitApplication::new(
                &application,
                config.build_profile.as_ref(),
                &tools_with_ensured_common_deps,
                config.enable_wasmtime_fs_cache,
            )
            .await;

            let (r, v) = result.merge_and_get(wit_result);
            result = r;
            if let Some(wit) = v {
                let temp_dir = application.temp_dir();
                let offline = config.offline;
                let ctx = ApplicationContext {
                    loaded_with_warnings: false,
                    config,
                    application,
                    wit,
                    calling_working_dir,
                    component_stub_defs: HashMap::new(),
                    common_wit_deps: OnceLock::new(),
                    component_generated_base_wit_deps: HashMap::new(),
                    selected_component_names: BTreeSet::new(),
                    remote_components: RemoteComponents::new(
                        file_download_client,
                        temp_dir,
                        offline,
                    ),
                    tools_with_ensured_common_deps,
                };
                ValidatedResult::Ok(ctx)
            } else {
                result.expect_error()
            }
        } else {
            result.expect_error()
        }
    }

    fn validate_build_profile(&self) -> anyhow::Result<()> {
        let Some(profile) = &self.config.build_profile else {
            return Ok(());
        };

        let all_profiles = self.application.all_build_profiles();
        if all_profiles.is_empty() {
            bail!(
                "Build profile {} not found, no available build profiles",
                profile.as_str().log_color_error_highlight(),
            );
        } else if !all_profiles.contains(profile) {
            bail!(
                "Build profile {} not found, available build profiles: {}",
                profile.as_str().log_color_error_highlight(),
                all_profiles
                    .into_iter()
                    .map(|s| s.as_str().log_color_highlight())
                    .join(", ")
            );
        }

        Ok(())
    }

    pub fn build_profile(&self) -> Option<&BuildProfileName> {
        self.config.build_profile.as_ref()
    }

    pub async fn update_wit_context(&mut self) -> anyhow::Result<()> {
        to_anyhow(
            "Failed to update application wit context, see problems above",
            ResolvedWitApplication::new(
                &self.application,
                self.build_profile(),
                &self.tools_with_ensured_common_deps,
                self.config.enable_wasmtime_fs_cache,
            )
            .await
            .map(|wit| {
                self.wit = wit;
            }),
            None,
        )
    }

    pub fn component_stub_def(
        &mut self,
        component_name: &AppComponentName,
        is_ephemeral: bool,
    ) -> anyhow::Result<&StubDefinition> {
        if !self.component_stub_defs.contains_key(component_name) {
            self.component_stub_defs.insert(
                component_name.clone(),
                StubDefinition::new(StubConfig {
                    source_wit_root: self
                        .application
                        .component_generated_base_wit(component_name),
                    client_root: self.application.client_temp_build_dir(component_name),
                    selected_world: None,
                    stub_crate_version: golem_common::golem_version().to_string(),
                    golem_rust_override: self.config.golem_rust_override.clone(),
                    extract_source_exports_package: false,
                    seal_cargo_workspace: true,
                    component_name: component_name.clone(),
                    is_ephemeral,
                })
                .context("Failed to gather information for the stub generator")?,
            );
        }
        Ok(self.component_stub_defs.get(component_name).unwrap())
    }

    pub fn component_stub_interfaces(
        &mut self,
        component_name: &AppComponentName,
    ) -> anyhow::Result<ComponentStubInterfaces> {
        let is_ephemeral = self
            .application
            .component_properties(component_name, self.build_profile())
            .is_ephemeral();
        let stub_def = self.component_stub_def(component_name, is_ephemeral)?;
        let client_package_name = stub_def.client_parser_package_name();
        let result = ComponentStubInterfaces {
            component_name: component_name.clone(),
            is_ephemeral,
            stub_interface_name: client_package_name
                .interface_id(&stub_def.client_interface_name()),
            exported_interfaces_per_stub_resource: BTreeMap::from_iter(
                stub_def.stubbed_entities().iter().filter_map(|entity| {
                    entity
                        .owner_interface()
                        .map(|owner| (entity.name().to_string(), owner.to_string()))
                }),
            ),
        };
        Ok(result)
    }

    pub fn common_wit_deps(&self) -> anyhow::Result<&WitDepsResolver> {
        match self
            .common_wit_deps
            .get_or_init(|| {
                let sources = self.application.wit_deps();
                if sources.is_empty() {
                    bail!("No common witDeps were defined in the application manifest")
                }
                WitDepsResolver::new(sources)
            })
            .as_ref()
        {
            Ok(wit_deps) => Ok(wit_deps),
            Err(err) => Err(anyhow!("Failed to init wit dependency resolver: {:#}", err)),
        }
    }

    pub fn component_base_output_wit_deps(
        &mut self,
        component_name: &AppComponentName,
    ) -> anyhow::Result<&WitDepsResolver> {
        // Not using the entry API, so we can skip copying the component name
        if !self
            .component_generated_base_wit_deps
            .contains_key(component_name)
        {
            self.component_generated_base_wit_deps.insert(
                component_name.clone(),
                WitDepsResolver::new(vec![self
                    .application
                    .component_generated_base_wit(component_name)
                    .join(naming::wit::DEPS_DIR)])?,
            );
        }
        Ok(self
            .component_generated_base_wit_deps
            .get(component_name)
            .unwrap())
    }

    pub fn select_components(
        &mut self,
        component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        log_action("Selecting", "components");
        let _indent = LogIndent::new();

        let current_dir = std::env::current_dir()?.canonicalize()?;

        let selected_component_names: ValidatedResult<BTreeSet<AppComponentName>> =
            match component_select_mode {
                ApplicationComponentSelectMode::CurrentDir => {
                    let called_from_project_root = self.calling_working_dir == current_dir;
                    if called_from_project_root {
                        ValidatedResult::Ok(
                            self.application
                                .component_names()
                                .map(|cn| cn.to_owned())
                                .collect(),
                        )
                    } else {
                        ValidatedResult::Ok(
                            self.application
                                .component_names()
                                .filter(|component_name| {
                                    self.application
                                        .component_source_dir(component_name)
                                        .starts_with(self.calling_working_dir.as_path())
                                })
                                .cloned()
                                .collect(),
                        )
                    }
                }
                ApplicationComponentSelectMode::All => ValidatedResult::Ok(
                    self.application
                        .component_names()
                        .map(|cn| cn.to_owned())
                        .collect(),
                ),
                ApplicationComponentSelectMode::Explicit(component_names) => {
                    let mut validation = ValidationBuilder::new();
                    for component_name in component_names {
                        if !self.application.contains_component(component_name) {
                            validation.add_error(format!(
                                "Requested component {} not found, available components: {}",
                                component_name.as_str().log_color_error_highlight(),
                                self.application
                                    .component_names()
                                    .map(|s| s.as_str().log_color_highlight())
                                    .join(", ")
                            ));
                        }
                    }
                    validation.build(BTreeSet::from_iter(component_names.iter().cloned()))
                }
            };

        let selected_component_names = to_anyhow(
            "Failed to select requested components",
            selected_component_names,
            None,
        )?;

        if self.application.component_names().next().is_none() {
            log_action("Found", "no components")
        } else {
            log_action(
                "Found",
                format!(
                    "components: {}",
                    self.application
                        .component_names()
                        .map(|s| s.as_str().log_color_highlight())
                        .join(", ")
                ),
            )
        }

        {
            log_action("Selected", "components:");
            let _indent = LogIndent::new();

            let padding = selected_component_names
                .iter()
                .map(|name| name.as_str().len())
                .max()
                .unwrap_or(0)
                + 1;

            for component_name in &selected_component_names {
                let property_source = self
                    .application
                    .component_effective_property_source(component_name, self.build_profile());

                let property_configuration = {
                    if property_source.template_name.is_none() && property_source.profile.is_none()
                    {
                        None
                    } else {
                        let mut configuration = String::with_capacity(10);
                        if let Some(template_name) = &property_source.template_name {
                            configuration.push_str(
                                &template_name.as_str().log_color_highlight().to_string(),
                            );
                        }
                        if property_source.template_name.is_some()
                            && property_source.profile.is_some()
                        {
                            configuration.push(':');
                        }
                        if let Some(profile) = &property_source.profile {
                            configuration
                                .push_str(&profile.as_str().log_color_highlight().to_string());
                        }

                        Some(configuration)
                    }
                };

                match property_configuration {
                    Some(config) => logln(format!(
                        "{} {}",
                        format!("{:<padding$}", format!("{}:", component_name.as_str())).blue(),
                        config
                    )),
                    None => {
                        logln(component_name.as_str().blue().to_string());
                    }
                }
            }
        }

        self.selected_component_names = selected_component_names;

        Ok(())
    }

    pub fn selected_component_names(&self) -> &BTreeSet<AppComponentName> {
        &self.selected_component_names
    }

    pub async fn build(&mut self) -> anyhow::Result<()> {
        build_app(self).await
    }

    pub async fn custom_command(&self, command_name: &str) -> Result<(), CustomCommandError> {
        execute_custom_command(self, command_name).await
    }

    pub fn clean(&self) -> anyhow::Result<()> {
        clean_app(self)
    }

    pub async fn resolve_binary_component_source(
        &self,
        dep: &DependentComponent,
    ) -> anyhow::Result<PathBuf> {
        match &dep.source {
            BinaryComponentSource::AppComponent { name } => {
                if dep.dep_type.is_wasm_rpc() {
                    Ok(self.application.client_wasm(name))
                } else {
                    Ok(self.application.component_wasm(name, self.build_profile()))
                }
            }
            BinaryComponentSource::LocalFile { path } => Ok(path.clone()),
            BinaryComponentSource::Url { url } => self.remote_components.get_from_url(url).await,
        }
    }

    pub fn log_dynamic_help(&self, config: &DynamicHelpSections) -> anyhow::Result<()> {
        static LABEL_SOURCE: &str = "Source";
        static LABEL_SELECTED: &str = "Selected";
        static LABEL_TEMPLATE: &str = "Template";
        static LABEL_BUILD_PROFILES: &str = "Build Profiles";
        static LABEL_COMPONENT_TYPE: &str = "Component Type";
        static LABEL_DEPENDENCIES: &str = "Dependencies";

        let label_padding = {
            [
                &LABEL_SOURCE,
                &LABEL_SELECTED,
                &LABEL_TEMPLATE,
                &LABEL_BUILD_PROFILES,
                &LABEL_COMPONENT_TYPE,
                &LABEL_DEPENDENCIES,
            ]
            .map(|label| label.len())
            .into_iter()
            .max()
            .unwrap_or(0)
                + 1
        };

        let print_field = |label: &'static str, value: String| {
            logln(format!(
                "    {:<label_padding$} {}",
                format!("{}:", label),
                value
            ))
        };

        let should_colorize = SHOULD_COLORIZE.should_colorize();

        if config.components() {
            if self.application.has_any_component() {
                logln(format!(
                    "{}",
                    "Application components:".log_color_help_group()
                ));
                for component_name in self.application.component_names() {
                    let selected = self.selected_component_names.contains(component_name);
                    let effective_property_source = self
                        .application
                        .component_effective_property_source(component_name, self.build_profile());
                    logln(format!("  {}", component_name.as_str().bold()));
                    print_field(
                        LABEL_SELECTED,
                        if selected {
                            "yes".green().bold().to_string()
                        } else {
                            "no".red().bold().to_string()
                        },
                    );
                    print_field(
                        LABEL_SOURCE,
                        self.application
                            .component_source(component_name)
                            .to_string_lossy()
                            .underline()
                            .to_string(),
                    );
                    if let Some(template_name) = effective_property_source.template_name {
                        print_field(LABEL_TEMPLATE, template_name.as_str().bold().to_string());
                    }
                    if let Some(selected_profile) = effective_property_source.profile {
                        print_field(
                            LABEL_BUILD_PROFILES,
                            self.application
                                .component_build_profiles(component_name)
                                .iter()
                                .map(|profile| {
                                    if selected_profile == profile {
                                        if should_colorize {
                                            profile.as_str().bold().underline().to_string()
                                        } else {
                                            format!("*{}", profile.as_str())
                                        }
                                    } else {
                                        profile.to_string()
                                    }
                                })
                                .join(", "),
                        );
                    }

                    print_field(
                        LABEL_COMPONENT_TYPE,
                        self.application
                            .component_properties(component_name, None)
                            .component_type()
                            .to_string()
                            .bold()
                            .to_string(),
                    );

                    let dependencies = self.application.component_dependencies(component_name);
                    if !dependencies.is_empty() {
                        logln(format!("    {LABEL_DEPENDENCIES}:"));
                        for dependency in dependencies {
                            logln(format!(
                                "      - {} ({})",
                                dependency.source.to_string().bold(),
                                dependency.dep_type.as_str(),
                            ))
                        }
                    }
                }
                logln("")
            } else {
                logln("No components found in the application.\n");
            }
        }

        if config.api_definitions() {
            if !self.application.http_api_definitions().is_empty() {
                logln(format!(
                    "{}",
                    "Application API definitions:".log_color_help_group()
                ));
                for (name, def) in self.application.http_api_definitions() {
                    logln(format!(
                        "  {}@{}",
                        name.as_str().log_color_highlight(),
                        def.value.version.log_color_highlight(),
                    ));
                }
                logln("");
            } else {
                logln("No API definitions found in the application.\n");
            }
        }

        if let Some(profile) = config.api_deployments_profile() {
            let http_api_deployments = self.application.http_api_deployments(profile);
            match http_api_deployments {
                Some(http_api_deployments) => {
                    logln(format!(
                        "{}",
                        format!("Application API deployments for profile {}:", profile.0)
                            .log_color_help_group()
                    ));
                    for (site, definitions) in http_api_deployments {
                        logln(format!("  {}", site.to_string().log_color_highlight(),));
                        for definition in definitions.iter().flat_map(|defs| defs.value.iter()) {
                            logln(format!("    {}", definition.as_str().log_color_highlight(),));
                        }
                    }
                    logln("");
                }
                None => {
                    logln(format!(
                        "No API deployments found in the application for profile {}.\n",
                        profile.0.log_color_highlight()
                    ));
                }
            }
        }

        if config.custom_commands() {
            for (profile, commands) in self
                .application
                .all_custom_commands_for_all_build_profiles()
            {
                if commands.is_empty() {
                    continue;
                }

                match profile {
                    None => logln(format!(
                        "{}",
                        "Application custom commands:".log_color_help_group()
                    )),
                    Some(profile) => logln(format!(
                        "{}{}{}",
                        "Custom commands for ".log_color_help_group(),
                        profile.as_str().log_color_help_group(),
                        " profile:".log_color_help_group(),
                    )),
                }
                for command in commands {
                    logln(format!(
                        "  {}",
                        format!(
                            "{}{}",
                            if config.builtin_commands().contains(&command)
                                || command.starts_with(':')
                            {
                                ":"
                            } else {
                                ""
                            },
                            command
                        )
                        .bold()
                    ))
                }
                logln("")
            }
        }

        // TODO: build profiles?

        Ok(())
    }
}

fn load_app(
    available_profiles: &BTreeSet<ProfileName>,
    source_mode: ApplicationSourceMode,
) -> Option<ValidatedResult<(Application, PathBuf)>> {
    load_raw_apps(source_mode).map(|raw_apps_and_calling_working_dir| {
        raw_apps_and_calling_working_dir.and_then(|(raw_apps, calling_working_dir)| {
            Application::from_raw_apps(available_profiles, raw_apps)
                .map(|app| (app, calling_working_dir))
        })
    })
}

fn load_profiles(
    source_mode: ApplicationSourceMode,
) -> Option<ValidatedResult<ApplicationPreloadResult>> {
    load_raw_apps(source_mode).map(|raw_apps_and_calling_working_dir| {
        raw_apps_and_calling_working_dir.and_then(|(raw_apps, calling_working_dir)| {
            Application::profiles_from_raw_apps(raw_apps.as_slice()).map(|profiles| {
                ApplicationPreloadResult {
                    source_mode: ApplicationSourceMode::Preloaded {
                        raw_apps,
                        calling_working_dir,
                    },
                    loaded_with_warnings: false,
                    profiles: Some(profiles),
                }
            })
        })
    })
}

fn load_raw_apps(
    source_mode: ApplicationSourceMode,
) -> Option<ValidatedResult<(Vec<app_raw::ApplicationWithSource>, PathBuf)>> {
    fn load(
        root_source: Option<&Path>,
    ) -> Option<ValidatedResult<(Vec<app_raw::ApplicationWithSource>, PathBuf)>> {
        collect_sources_and_switch_to_app_root(root_source).map(|sources_and_calling_working_dir| {
            sources_and_calling_working_dir.and_then(|(sources, calling_working_dir)| {
                sources
                    .into_iter()
                    .map(|source| {
                        ValidatedResult::from_result(
                            app_raw::ApplicationWithSource::from_yaml_file(source),
                        )
                    })
                    .collect::<ValidatedResult<Vec<_>>>()
                    .map(|raw_apps| (raw_apps, calling_working_dir))
            })
        })
    }

    match source_mode {
        ApplicationSourceMode::Automatic => load(None),
        ApplicationSourceMode::ByRootManifest(root_manifest) => load(Some(&root_manifest)),
        ApplicationSourceMode::Preloaded {
            raw_apps,
            calling_working_dir,
        } => Some(ValidatedResult::Ok((raw_apps, calling_working_dir))),
        ApplicationSourceMode::None => None,
    }
}

fn collect_sources_and_switch_to_app_root(
    root_manifest: Option<&Path>,
) -> Option<ValidatedResult<(BTreeSet<PathBuf>, PathBuf)>> {
    let calling_working_dir = std::env::current_dir()
        .expect("Failed to get current working directory")
        .canonicalize()
        .expect("Failed to canonicalize current working directory");

    log_action("Collecting", "application manifests");
    let _indent = LogIndent::new();

    fn collect_by_main_source(source: &Path) -> Option<ValidatedResult<BTreeSet<PathBuf>>> {
        let source_ext = PathExtra::new(&source);
        let source_dir = source_ext.parent().unwrap();
        std::env::set_current_dir(source_dir).expect("Failed to set current dir for config parent");

        let includes = includes_from_yaml_file(source);
        if includes.is_empty() {
            Some(ValidatedResult::Ok(BTreeSet::from([source.to_path_buf()])))
        } else {
            Some(
                ValidatedResult::from_result(compile_and_collect_globs(source_dir, &includes)).map(
                    |mut sources| {
                        sources.insert(0, source.to_path_buf());
                        sources.into_iter().collect()
                    },
                ),
            )
        }
    }

    let sources = match root_manifest {
        None => match find_main_source() {
            Some(source) => collect_by_main_source(&source),
            None => None,
        },
        Some(source) => match source.canonicalize() {
            Ok(source) => collect_by_main_source(&source),
            Err(err) => Some(ValidatedResult::from_error(format!(
                "Cannot resolve requested application manifest source {}: {}",
                source.log_color_highlight(),
                err
            ))),
        },
    };

    sources.map(|sources| {
        sources
            .inspect(|sources| {
                if sources.is_empty() {
                    log_action("Found", "no sources");
                } else {
                    log_action(
                        "Found",
                        format!(
                            "sources: {}",
                            sources
                                .iter()
                                .map(|source| source.log_color_highlight())
                                .join(", ")
                        ),
                    );
                }
            })
            .map(|sources| (sources, calling_working_dir))
    })
}

fn find_main_source() -> Option<PathBuf> {
    let mut current_dir = std::env::current_dir().expect("Failed to get current dir");
    let mut last_source: Option<PathBuf> = None;

    loop {
        let file = current_dir.join(DEFAULT_CONFIG_FILE_NAME);
        if current_dir.join(DEFAULT_CONFIG_FILE_NAME).exists() {
            last_source = Some(file);
        }
        match current_dir.parent() {
            Some(parent_dir) => current_dir = parent_dir.to_path_buf(),
            None => {
                break;
            }
        }
    }

    last_source
}

pub fn to_anyhow<T>(
    message: &str,
    result: ValidatedResult<T>,
    mark_had_warns: Option<fn(T) -> T>,
) -> anyhow::Result<T> {
    match result {
        ValidatedResult::Ok(value) => Ok(value),
        ValidatedResult::OkWithWarns(value, warns) => {
            logln("");
            for line in format_warns(&warns).lines() {
                logln(line);
            }
            Ok(match mark_had_warns {
                Some(mark_had_warns) => mark_had_warns(value),
                None => value,
            })
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => Err(anyhow!(AppValidationError {
            message: message.to_string(),
            warns,
            errors,
        })),
    }
}
