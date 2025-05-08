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

use crate::app::build::build_app;
use crate::app::build::clean::clean_app;
use crate::app::build::external_command::execute_custom_command;
use crate::app::error::{format_warns, AppValidationError, CustomCommandError};
use crate::fs::{compile_and_collect_globs, PathExtra};
use crate::log::{log_action, log_warn_action, logln, LogColorize, LogIndent};
use crate::model::app::{
    includes_from_yaml_file, AppComponentName, Application, ApplicationComponentSelectMode,
    ApplicationConfig, ApplicationSourceMode, BuildProfileName, ComponentStubInterfaces,
    DynamicHelpSections, DEFAULT_CONFIG_FILE_NAME,
};
use crate::model::app_raw;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::stub::{StubConfig, StubDefinition};
use crate::wasm_rpc_stubgen::wit_resolve::{ResolvedWitApplication, WitDepsResolver};
use anyhow::{anyhow, bail, Context};
use colored::control::SHOULD_COLORIZE;
use colored::Colorize;
use golem_wasm_rpc::WASM_RPC_VERSION;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct ApplicationContext {
    pub config: ApplicationConfig,
    pub application: Application,
    pub wit: ResolvedWitApplication,
    pub calling_working_dir: PathBuf,
    component_stub_defs: HashMap<AppComponentName, StubDefinition>,
    common_wit_deps: OnceLock<anyhow::Result<WitDepsResolver>>,
    component_generated_base_wit_deps: HashMap<AppComponentName, WitDepsResolver>,
    selected_component_names: BTreeSet<AppComponentName>,
}

impl ApplicationContext {
    pub fn new(config: ApplicationConfig) -> anyhow::Result<Option<ApplicationContext>> {
        let Some(app_and_calling_working_dir) = load_app(&config) else {
            return Ok(None);
        };

        let ctx = to_anyhow(
            "Failed to create application context, see problems above",
            app_and_calling_working_dir.and_then(|(application, calling_working_dir)| {
                ResolvedWitApplication::new(&application, config.profile.as_ref()).map(|wit| {
                    ApplicationContext {
                        config,
                        application,
                        wit,
                        calling_working_dir,
                        component_stub_defs: HashMap::new(),
                        common_wit_deps: OnceLock::new(),
                        component_generated_base_wit_deps: HashMap::new(),
                        selected_component_names: BTreeSet::new(),
                    }
                })
            }),
        )?;

        ctx.select_and_validate_profiles()?;

        if ctx.config.offline {
            log_action("Selected", "offline mode");
        }

        Ok(Some(ctx))
    }

    fn select_and_validate_profiles(&self) -> anyhow::Result<()> {
        match &self.config.profile {
            Some(profile) => {
                let all_profiles = self.application.all_profiles();
                if all_profiles.is_empty() {
                    bail!(
                        "Profile {} not found, no available profiles",
                        profile.as_str().log_color_error_highlight(),
                    );
                } else if !all_profiles.contains(profile) {
                    bail!(
                        "Profile {} not found, available profiles: {}",
                        profile.as_str().log_color_error_highlight(),
                        all_profiles
                            .into_iter()
                            .map(|s| s.as_str().log_color_highlight())
                            .join(", ")
                    );
                }
                log_action(
                    "Selecting",
                    format!(
                        "profiles, requested profile: {}",
                        profile.as_str().log_color_highlight()
                    ),
                );
            }
            None => {
                log_action("Selecting", "profiles, no profile was requested");
            }
        }

        let _indent = LogIndent::new();
        for component_name in self.application.component_names() {
            let selection = self
                .application
                .component_effective_property_source(component_name, self.profile());

            // TODO: simplify this
            let message = match (
                selection.profile,
                selection.template_name,
                self.profile().is_some(),
                selection.is_requested_profile,
            ) {
                (None, None, false, _) => {
                    format!(
                        "default build profile for {}",
                        component_name.as_str().log_color_highlight()
                    )
                }
                (None, None, true, _) => {
                    format!(
                        "default build profile for {}, component has no profiles",
                        component_name.as_str().log_color_highlight()
                    )
                }
                (None, Some(template), false, _) => {
                    format!(
                        "default build profile for {} using template {}{}",
                        component_name.as_str().log_color_highlight(),
                        template.as_str().log_color_highlight(),
                        if selection.any_template_overrides {
                            " with overrides"
                        } else {
                            ""
                        }
                    )
                }
                (None, Some(template), true, _) => {
                    format!(
                        "default build profile for {} using template {}{}, component has no profiles",
                        component_name.as_str().log_color_highlight(),
                        template.as_str().log_color_highlight(),
                        if selection.any_template_overrides {
                            " with overrides"
                        } else {
                            ""
                        }
                    )
                }
                (Some(profile), None, false, false) => {
                    format!(
                        "default build profile {} for {}",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight()
                    )
                }
                (Some(profile), None, true, false) => {
                    format!(
                        "default build profile {} for {}, component has no matching requested profile",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight()
                    )
                }
                (Some(profile), Some(template), false, false) => {
                    format!(
                        "default build profile {} for {} using template {}{}",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight(),
                        template.as_str().log_color_highlight(),
                        if selection.any_template_overrides {
                            " with overrides"
                        } else {
                            ""
                        }
                    )
                }
                (Some(profile), Some(template), true, false) => {
                    format!(
                        "default build profile {} for {} using template {}{}, component has no matching requested profile",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight(),
                        template.as_str().log_color_highlight(),
                        if selection.any_template_overrides {
                            " with overrides"
                        } else {
                            ""
                        }
                    )
                }
                (Some(profile), None, false, true) => {
                    format!(
                        "build profile {} for {}",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight()
                    )
                }
                (Some(profile), None, true, true) => {
                    format!(
                        "requested build profile {} for {}",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight()
                    )
                }
                (Some(profile), Some(template), false, true) => {
                    format!(
                        "build profile {} for {} using template {}{}",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight(),
                        template.as_str().log_color_highlight(),
                        if selection.any_template_overrides {
                            " with overrides"
                        } else {
                            ""
                        }
                    )
                }
                (Some(profile), Some(template), true, true) => {
                    format!(
                        "build requested profile {} for {} using template {}{}",
                        profile.as_str().log_color_highlight(),
                        component_name.as_str().log_color_highlight(),
                        template.as_str().log_color_highlight(),
                        if selection.any_template_overrides {
                            " with overrides"
                        } else {
                            ""
                        }
                    )
                }
            };

            log_action("Selected", message);
        }

        Ok(())
    }

    pub fn profile(&self) -> Option<&BuildProfileName> {
        self.config.profile.as_ref()
    }

    pub fn update_wit_context(&mut self) -> anyhow::Result<()> {
        to_anyhow(
            "Failed to update application wit context, see problems above",
            ResolvedWitApplication::new(&self.application, self.profile()).map(|wit| {
                self.wit = wit;
            }),
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
                    stub_crate_version: WASM_RPC_VERSION.to_string(),
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
            .component_properties(component_name, self.profile())
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
                ApplicationComponentSelectMode::CurrentDir => match &self.config.app_source_mode {
                    ApplicationSourceMode::Automatic => {
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
                    // TODO: review this after changing explicit mode
                    ApplicationSourceMode::Explicit(_) => ValidatedResult::Ok(
                        self.application
                            .component_names()
                            .map(|cn| cn.to_owned())
                            .collect(),
                    ),
                    ApplicationSourceMode::None => {
                        panic!("Cannot select components without source");
                    }
                },
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

        let components_formatted = selected_component_names
            .iter()
            .map(|s| s.as_str().log_color_highlight())
            .join(", ");
        match component_select_mode {
            ApplicationComponentSelectMode::CurrentDir => log_action(
                "Selected",
                format!("components based on current dir: {} ", components_formatted),
            ),
            ApplicationComponentSelectMode::All => log_action("Selected", "all components"),
            ApplicationComponentSelectMode::Explicit(_) => log_action(
                "Selected",
                format!("components based on request: {} ", components_formatted),
            ),
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

    pub fn custom_command(&self, command_name: &str) -> Result<(), CustomCommandError> {
        execute_custom_command(self, command_name)
    }

    pub fn clean(&self) -> anyhow::Result<()> {
        clean_app(self)
    }

    pub fn log_dynamic_help(&self, config: &DynamicHelpSections) -> anyhow::Result<()> {
        static LABEL_SOURCE: &str = "Source";
        static LABEL_SELECTED: &str = "Selected";
        static LABEL_TEMPLATE: &str = "Template";
        static LABEL_PROFILES: &str = "Profiles";
        static LABEL_COMPONENT_TYPE: &str = "Component Type";
        static LABEL_DEPENDENCIES: &str = "Dependencies";

        let label_padding = {
            [
                &LABEL_SOURCE,
                &LABEL_SELECTED,
                &LABEL_TEMPLATE,
                &LABEL_PROFILES,
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
                        .component_effective_property_source(component_name, self.profile());
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
                            LABEL_PROFILES,
                            self.application
                                .component_profiles(component_name)
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
                            .component_type
                            .to_string()
                            .bold()
                            .to_string(),
                    );

                    let dependencies = self.application.component_dependencies(component_name);
                    if !dependencies.is_empty() {
                        logln(format!("    {}:", LABEL_DEPENDENCIES));
                        for dependency in dependencies {
                            logln(format!(
                                "      - {} ({})",
                                dependency.name.as_str().bold(),
                                dependency.dep_type.as_str(),
                            ))
                        }
                    }
                }
                logln("\n")
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

        if config.api_deployments() {
            if !self.application.http_api_deployments().is_empty() {
                logln(format!(
                    "{}",
                    "Application API deployments:".log_color_help_group()
                ));
                for dep in self.application.http_api_deployments().values() {
                    logln(format!(
                        "  {}{}",
                        match &dep.value.subdomain {
                            Some(subdomain) => {
                                format!("{}.", subdomain.log_color_highlight())
                            }
                            None => {
                                "".to_string()
                            }
                        },
                        dep.value.host.log_color_highlight(),
                    ));
                }
                logln("");
            } else {
                logln("No API deployments found in the application.\n");
            }
        }

        if config.custom_commands() {
            for (profile, commands) in self.application.all_custom_commands_for_all_profiles() {
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

        // TODO: profiles?

        Ok(())
    }
}

fn load_app(config: &ApplicationConfig) -> Option<ValidatedResult<(Application, PathBuf)>> {
    let result =
        collect_sources(&config.app_source_mode)?.and_then(|(sources, calling_working_dir)| {
            sources
                .into_iter()
                .map(|source| {
                    ValidatedResult::from_result(app_raw::ApplicationWithSource::from_yaml_file(
                        source,
                    ))
                })
                .collect::<ValidatedResult<Vec<_>>>()
                .and_then(Application::from_raw_apps)
                .map(|app| (app, calling_working_dir))
        });

    Some(result)
}

fn collect_sources(
    mode: &ApplicationSourceMode,
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

    let sources = match mode {
        ApplicationSourceMode::Automatic => match find_main_source() {
            Some(source) => collect_by_main_source(&source),
            None => None,
        },
        ApplicationSourceMode::Explicit(source) => match source.canonicalize() {
            Ok(source) => collect_by_main_source(&source),
            Err(err) => Some(ValidatedResult::from_error(format!(
                "Cannot resolve requested application manifest source {}: {}",
                source.log_color_highlight(),
                err
            ))),
        },
        ApplicationSourceMode::None => None,
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

fn to_anyhow<T>(message: &str, result: ValidatedResult<T>) -> anyhow::Result<T> {
    match result {
        ValidatedResult::Ok(value) => Ok(value),
        ValidatedResult::OkWithWarns(components, warns) => {
            log_warn_action("App validation warnings:\n", format_warns(&warns));
            Ok(components)
        }
        ValidatedResult::WarnsAndErrors(warns, errors) => Err(anyhow!(AppValidationError {
            message: message.to_string(),
            warns,
            errors,
        })),
    }
}
