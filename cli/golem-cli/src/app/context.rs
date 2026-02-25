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
use crate::app::template::AppTemplateRepo;
use crate::log::{log_action, logln, LogColorize, LogIndent, LogOutput, Output};
use crate::model::app::{
    includes_from_yaml_file, AppBuildStep, Application, ApplicationComponentSelectMode,
    ApplicationConfig, ApplicationNameAndEnvironments, ApplicationSourceMode, BuildConfig,
    CleanMode, ComponentPresetSelector, CustomBridgeSdkTarget, DynamicHelpSections, WithSource,
    DEFAULT_CONFIG_FILE_NAME,
};
use crate::model::text::fmt::format_component_applied_layers;
use crate::model::text::server::ToFormattedServerContext;
use crate::model::{app_raw, GuestLanguage};
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::{fs, SdkOverrides};
use anyhow::anyhow;
use colored::Colorize;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::environment::EnvironmentName;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

pub struct BuildContext<'a> {
    application_context: &'a ApplicationContext,
    build_config: &'a BuildConfig,
}

impl<'a> BuildContext<'a> {
    pub fn new(application_context: &'a ApplicationContext, build_config: &'a BuildConfig) -> Self {
        Self {
            application_context,
            build_config,
        }
    }

    pub fn application_context(&self) -> &ApplicationContext {
        self.application_context
    }

    pub fn application(&self) -> &Application {
        &self.application_context.application
    }

    pub fn application_config(&self) -> &ApplicationConfig {
        &self.application_context.config
    }

    pub fn agent_types(&self) -> &crate::app::agent_types::AgentTypeRegistry {
        self.application_context.agent_types()
    }

    pub fn build_config(&self) -> &BuildConfig {
        self.build_config
    }

    pub fn should_run_step(&self, step: AppBuildStep) -> bool {
        self.build_config().should_run_step(step)
    }

    pub fn custom_bridge_sdk_target(&self) -> Option<&CustomBridgeSdkTarget> {
        self.build_config.custom_bridge_sdk_target.as_ref()
    }

    pub fn repl_bridge_sdk_target(&self) -> Option<&CustomBridgeSdkTarget> {
        self.build_config.repl_bridge_sdk_target.as_ref()
    }

    pub fn skip_up_to_date_checks(&self) -> bool {
        self.build_config.skip_up_to_date_checks
    }

    pub fn tools_with_ensured_common_deps(&self) -> &ToolsWithEnsuredCommonDeps {
        &self.application_context.tools_with_ensured_common_deps
    }
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
    pub application_name_and_environments: Option<ApplicationNameAndEnvironments>,
    pub used_language_templates: HashSet<GuestLanguage>,
}

pub struct ApplicationContext {
    loaded_with_warnings: bool,
    config: ApplicationConfig,
    application: Application,
    agent_types: crate::app::agent_types::AgentTypeRegistry,
    calling_working_dir: PathBuf,
    selected_component_names: BTreeSet<ComponentName>,
    tools_with_ensured_common_deps: ToolsWithEnsuredCommonDeps,
}

impl ApplicationContext {
    pub fn preload_application(
        source_mode: ApplicationSourceMode,
        dev_mode: bool,
        sdk_overrides: &SdkOverrides,
    ) -> anyhow::Result<ApplicationPreloadResult> {
        let _output = LogOutput::new(Output::None);

        match preload_app(source_mode, dev_mode, sdk_overrides) {
            Some(environments) => to_anyhow(
                "Failed to load application manifest environments, see problems above",
                environments,
                Some(|mut preload_result| {
                    preload_result.loaded_with_warnings = true;
                    preload_result
                }),
            ),
            None => Ok(ApplicationPreloadResult {
                source_mode: ApplicationSourceMode::None,
                loaded_with_warnings: false,
                application_name_and_environments: None,
                used_language_templates: HashSet::new(),
            }),
        }
    }

    pub async fn new(
        source_mode: ApplicationSourceMode,
        config: ApplicationConfig,
        application_name: WithSource<ApplicationName>,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        component_presets: ComponentPresetSelector,
        file_download_client: reqwest::Client,
    ) -> anyhow::Result<Option<ApplicationContext>> {
        let Some(app_and_calling_working_dir) = load_app(
            application_name,
            environments,
            component_presets,
            source_mode,
        ) else {
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

        if ctx.config.offline {
            log_action("Using", "offline mode");
        }

        Ok(Some(ctx))
    }

    pub fn loaded_with_warnings(&self) -> bool {
        self.loaded_with_warnings
    }

    pub fn application(&self) -> &Application {
        &self.application
    }

    pub fn agent_types(&self) -> &crate::app::agent_types::AgentTypeRegistry {
        &self.agent_types
    }

    pub fn new_repl_bridge_sdk_target(&self, language: GuestLanguage) -> CustomBridgeSdkTarget {
        let repl_root_bridge_sdk_dir = self.application.repl_root_bridge_sdk_dir(language);
        CustomBridgeSdkTarget {
            agent_type_names: Default::default(),
            target_language: Some(language),
            output_dir: Some(repl_root_bridge_sdk_dir.clone()),
        }
    }

    async fn create_context(
        app_and_calling_working_dir: ValidatedResult<(Application, PathBuf)>,
        config: ApplicationConfig,
        _file_download_client: reqwest::Client,
    ) -> ValidatedResult<ApplicationContext> {
        let tools_with_ensured_common_deps = ToolsWithEnsuredCommonDeps::new();
        app_and_calling_working_dir.map(|(application, calling_working_dir)| {
            let agent_types =
                crate::app::agent_types::AgentTypeRegistry::new(config.enable_wasmtime_fs_cache);
            ApplicationContext {
                loaded_with_warnings: false,
                config,
                application,
                agent_types,
                calling_working_dir,
                selected_component_names: BTreeSet::new(),
                tools_with_ensured_common_deps,
            }
        })
    }

    pub fn select_components(
        &mut self,
        component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        log_action("Selecting", "components");
        let _indent = LogIndent::new();

        let current_dir = std::env::current_dir()?.canonicalize()?;

        let selected_component_names: ValidatedResult<BTreeSet<ComponentName>> =
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
                                        .component(component_name)
                                        .source_dir()
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
            log_action("Selected", "components and layers:");
            let _indent = LogIndent::new();

            let padding = selected_component_names
                .iter()
                .map(|name| name.as_str().len())
                .max()
                .unwrap_or(0)
                + 1;

            for component_name in &selected_component_names {
                let component = self.application.component(component_name);
                let applied_layers = component.applied_layers();

                if applied_layers.is_empty() {
                    logln(component_name.as_str().blue().to_string());
                } else {
                    logln(format!(
                        "{} {}",
                        format!("{:<padding$}", format!("{}:", component_name.as_str())).blue(),
                        format_component_applied_layers(applied_layers)
                    ))
                }
            }
        }

        self.selected_component_names = selected_component_names;

        Ok(())
    }

    pub fn selected_component_names(&self) -> &BTreeSet<ComponentName> {
        &self.selected_component_names
    }

    pub fn deployable_component_names(&self) -> BTreeSet<ComponentName> {
        self.application
            .component_names()
            .filter(|name| self.application.component(name).is_deployable())
            .cloned()
            .collect()
    }

    pub async fn build(&self, build_config: &BuildConfig) -> anyhow::Result<()> {
        build_app(&BuildContext::new(self, build_config)).await
    }

    pub async fn custom_command(
        &self,
        build_config: &BuildConfig,
        command_name: &str,
    ) -> Result<(), CustomCommandError> {
        execute_custom_command(&BuildContext::new(self, build_config), command_name).await
    }

    pub fn clean(&self, build_config: &BuildConfig, mode: CleanMode) -> anyhow::Result<()> {
        clean_app(&BuildContext::new(self, build_config), mode)
    }

    pub fn log_dynamic_help(&self, config: &DynamicHelpSections) -> anyhow::Result<()> {
        fn print_field(label_padding: usize, label: &'static str, value: String) {
            logln(format!(
                "    {:<label_padding$} {}",
                format!("{}:", label),
                value
            ));
        }

        fn padding(labels: &[&str]) -> usize {
            labels.iter().map(|label| label.len()).max().unwrap_or(0) + 1
        }

        if config.environments() {
            static LABEL_NAME: &str = "Name";
            static LABEL_SELECTED: &str = "Selected";
            static LABEL_SERVER: &str = "Server";
            static LABEL_PRESETS: &str = "Presets";

            let label_padding = padding(&[LABEL_NAME, LABEL_SELECTED, LABEL_SERVER, LABEL_PRESETS]);
            let selected_environment_name = self.application.environment_name();

            logln(format!(
                "{}",
                "Application environments:".log_color_help_group()
            ));
            for (environment_name, environment) in self.application.environments() {
                logln(format!("  {}", environment_name.0.bold()));
                print_field(
                    label_padding,
                    LABEL_SELECTED,
                    if environment_name == selected_environment_name {
                        "yes".green().bold().to_string()
                    } else {
                        "no".red().bold().to_string()
                    },
                );
                print_field(
                    label_padding,
                    LABEL_SERVER,
                    environment.to_formatted_server_context().bold().to_string(),
                );
                print_field(
                    label_padding,
                    LABEL_PRESETS,
                    environment
                        .component_presets
                        .clone()
                        .into_vec()
                        .join(", ")
                        .bold()
                        .to_string(),
                );
                logln("")
            }
        }

        if config.components() {
            if self.application.has_any_component() {
                static LABEL_SOURCE: &str = "Source";
                static LABEL_SELECTED: &str = "Selected";
                static LABEL_LAYERS: &str = "Layers";
                let label_padding = padding(&[LABEL_SOURCE, LABEL_LAYERS]);

                logln(format!(
                    "{}",
                    "Application components:".log_color_help_group()
                ));
                for component_name in self.application.component_names() {
                    let component = self.application.component(component_name);
                    let selected = self.selected_component_names.contains(component_name);
                    logln(format!("  {}", component_name.as_str().bold()));
                    print_field(
                        label_padding,
                        LABEL_SELECTED,
                        if selected {
                            "yes".green().bold().to_string()
                        } else {
                            "no".red().bold().to_string()
                        },
                    );
                    print_field(
                        label_padding,
                        LABEL_SOURCE,
                        component
                            .source()
                            .display()
                            .to_string()
                            .underline()
                            .to_string(),
                    );
                    print_field(
                        label_padding,
                        LABEL_LAYERS,
                        format_component_applied_layers(component.applied_layers()),
                    );

                    logln("")
                }
            } else {
                logln("No components found in the application.\n");
            }
        }

        if config.api_deployments() {
            let environment_name = self.application.environment_name();
            let http_api_deployments = self
                .application
                .http_api_deployments(self.application.environment_name());
            match http_api_deployments {
                Some(http_api_deployments) => {
                    logln(format!(
                        "{}",
                        format!(
                            "Application API deployments for environment {}:",
                            environment_name.0
                        )
                        .log_color_help_group()
                    ));

                    for (site, deployment) in http_api_deployments {
                        logln(format!("  {}", site.to_string().log_color_highlight(),));
                        for (agent_name, _) in deployment.value.agents.iter() {
                            logln(format!("    {}", agent_name.as_str().log_color_highlight(),));
                        }
                    }
                    logln("");
                }
                None => {
                    logln(format!(
                        "No API deployments found in the application for environment {}.\n",
                        environment_name.0.log_color_highlight()
                    ));
                }
            }
        }

        if config.custom_commands() {
            let commands = self.application.all_custom_commands();
            if !commands.is_empty() {
                logln(format!(
                    "{}",
                    "Application custom commands:".log_color_help_group()
                ));
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
                    ));
                }
                logln("")
            }
        }

        Ok(())
    }
}

fn load_app(
    application_name: WithSource<ApplicationName>,
    environments: BTreeMap<EnvironmentName, app_raw::Environment>,
    component_presets: ComponentPresetSelector,
    source_mode: ApplicationSourceMode,
) -> Option<ValidatedResult<(Application, PathBuf)>> {
    load_raw_apps(source_mode).map(|raw_apps_and_calling_working_dir| {
        raw_apps_and_calling_working_dir.and_then(|(raw_apps, calling_working_dir)| {
            Application::from_raw_apps(application_name, environments, component_presets, raw_apps)
                .map(|app| (app, calling_working_dir))
        })
    })
}

fn preload_app(
    source_mode: ApplicationSourceMode,
    dev_mode: bool,
    sdk_overrides: &SdkOverrides,
) -> Option<ValidatedResult<ApplicationPreloadResult>> {
    load_raw_apps(source_mode).map(|raw_apps_and_calling_working_dir| {
        raw_apps_and_calling_working_dir.and_then(|(raw_apps, calling_working_dir)| {
            let used_language_templates = Application::language_templates_from_raw_apps(&raw_apps);

            Application::environments_from_raw_apps(raw_apps.as_slice())
                .and_then(|application_name_and_environments| {
                    ValidatedResult::from_result(ensure_on_demand_commons(
                        &used_language_templates,
                        dev_mode,
                        sdk_overrides,
                    ))
                    .map(|on_demand_common_raw_apps| {
                        (on_demand_common_raw_apps, application_name_and_environments)
                    })
                })
                .map(
                    |(on_demand_common_raw_apps, application_name_and_environments)| {
                        let raw_apps = {
                            let mut raw_apps = raw_apps;
                            raw_apps.extend(on_demand_common_raw_apps);
                            raw_apps
                        };

                        ApplicationPreloadResult {
                            source_mode: ApplicationSourceMode::Preloaded {
                                raw_apps,
                                calling_working_dir,
                            },
                            loaded_with_warnings: false,
                            application_name_and_environments: Some(
                                application_name_and_environments,
                            ),
                            used_language_templates,
                        }
                    },
                )
        })
    })
}

fn ensure_on_demand_commons(
    languages: &HashSet<GuestLanguage>,
    dev_mode: bool,
    sdk_overrides: &SdkOverrides,
) -> anyhow::Result<Vec<app_raw::ApplicationWithSource>> {
    let app_template_repo = AppTemplateRepo::get(dev_mode)?;

    let common_on_demand_templates = languages
        .iter()
        .map(|language| app_template_repo.common_on_demand_templates(*language))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flat_map(|templates| templates.values())
        .collect::<Vec<_>>();

    common_on_demand_templates
        .into_iter()
        .map(|template| {
            let target_dir = Application::on_demand_common_dir_for_language(template.0.language);
            template
                .generate(&target_dir, sdk_overrides)
                .map(|_| target_dir)
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|target_dir| {
            let golem_yaml_path = target_dir.join("golem.yaml");
            golem_yaml_path
                .exists()
                .then(|| app_raw::ApplicationWithSource::from_yaml_file(golem_yaml_path))
        })
        .collect::<Result<Vec<_>, _>>()
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
        let source_dir =
            fs::parent_or_err(source).expect("Failed to get parent dir for config parent");
        std::env::set_current_dir(source_dir).expect("Failed to set current dir for config parent");

        let includes = includes_from_yaml_file(source);
        if includes.is_empty() {
            Some(ValidatedResult::Ok(BTreeSet::from([source.to_path_buf()])))
        } else {
            Some(
                ValidatedResult::from_result(fs::compile_and_collect_globs(source_dir, &includes))
                    .map(|mut sources| {
                        sources.insert(0, source.to_path_buf());
                        sources.into_iter().collect()
                    }),
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
