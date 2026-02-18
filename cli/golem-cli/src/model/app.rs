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

use super::http_api::HttpApiDeploymentDeployProperties;
use crate::bridge_gen::bridge_client_directory_name;
use crate::fs;
use crate::log::LogColorize;
use crate::model::app::app_builder::{build_application, build_environments};
use crate::model::app_raw;
use crate::model::cascade::layer::Layer;
use crate::model::cascade::property::map::{MapMergeMode, MapProperty};
use crate::model::cascade::property::optional::OptionalProperty;
use crate::model::cascade::property::vec::{VecMergeMode, VecProperty};
use crate::model::cascade::property::Property;
use crate::model::component::AppComponentType;
use crate::model::repl::ReplLanguage;
use crate::model::template::Template;
use crate::validation::{ValidatedResult, ValidationBuilder};

use golem_common::model::agent::{AgentType, AgentTypeName};
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentFilePath, ComponentFilePermissions, ComponentName};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::validate_lower_kebab_case_identifier;
use golem_templates::model::GuestLanguage;
use heck::{
    ToKebabCase, ToLowerCamelCase, ToPascalCase, ToShoutyKebabCase, ToShoutySnakeCase, ToSnakeCase,
    ToTitleCase, ToTrainCase, ToUpperCamelCase,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Formatter;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";
pub const DEFAULT_TEMP_DIR: &str = "golem-temp";
pub const APP_ENV_PRESET_PREFIX: &str = "app-env:";

#[derive(Clone, Debug, Default)]
pub struct BuildConfig {
    pub skip_up_to_date_checks: bool,
    pub steps_filter: HashSet<AppBuildStep>,
    pub custom_bridge_sdk_target: Option<CustomBridgeSdkTarget>,
    pub repl_bridge_sdk_target: Option<CustomBridgeSdkTarget>,
}

impl BuildConfig {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_skip_up_to_date_checks(mut self, skip_up_to_date_checks: bool) -> Self {
        self.skip_up_to_date_checks = skip_up_to_date_checks;
        self
    }

    pub fn with_steps_filter(mut self, steps_filter: HashSet<AppBuildStep>) -> Self {
        self.steps_filter = steps_filter;
        self
    }

    pub fn with_custom_bridge_sdk_target(
        mut self,
        custom_bridge_sdk_target: CustomBridgeSdkTarget,
    ) -> Self {
        self.custom_bridge_sdk_target = Some(custom_bridge_sdk_target);
        self
    }

    pub fn with_repl_bridge_sdk_target(
        mut self,
        repl_bridge_sdk_target: CustomBridgeSdkTarget,
    ) -> Self {
        self.repl_bridge_sdk_target = Some(repl_bridge_sdk_target);
        self
    }

    pub fn should_run_step(&self, step: AppBuildStep) -> bool {
        if self.steps_filter.is_empty() {
            true
        } else {
            self.steps_filter.contains(&step)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RustDependencyOverride {
    pub path_override: Option<PathBuf>,
    pub version_override: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ApplicationConfig {
    pub offline: bool,
    pub golem_rust_override: RustDependencyOverride,
    pub dev_mode: bool,
    pub enable_wasmtime_fs_cache: bool,
}

#[derive(Debug, Clone)]
pub enum ApplicationSourceMode {
    Automatic,
    ByRootManifest(PathBuf),
    Preloaded {
        raw_apps: Vec<app_raw::ApplicationWithSource>,
        calling_working_dir: PathBuf,
    },
    None,
}

#[derive(Debug, Clone)]
pub enum ApplicationComponentSelectMode {
    CurrentDir,
    All,
    Explicit(Vec<ComponentName>),
}

impl ApplicationComponentSelectMode {
    pub fn all_or_explicit(component_names: Vec<ComponentName>) -> Self {
        if component_names.is_empty() {
            ApplicationComponentSelectMode::All
        } else {
            ApplicationComponentSelectMode::Explicit(component_names)
        }
    }

    pub fn current_dir_or_explicit(component_names: Vec<ComponentName>) -> Self {
        if component_names.is_empty() {
            ApplicationComponentSelectMode::CurrentDir
        } else {
            ApplicationComponentSelectMode::Explicit(component_names)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CleanMode {
    All,
    SelectedComponentsOnly,
}

#[derive(Debug, Clone)]
pub struct DynamicHelpSections {
    environments: bool,
    components: bool,
    custom_commands: bool,
    builtin_commands: BTreeSet<String>,
    api_definitions: bool,
    api_deployments: bool,
}

impl DynamicHelpSections {
    pub fn show_all(builtin_commands: BTreeSet<String>) -> Self {
        Self {
            environments: true,
            components: true,
            custom_commands: true,
            builtin_commands,
            api_definitions: true,
            api_deployments: true,
        }
    }

    pub fn show_components() -> Self {
        Self {
            environments: true,
            components: true,
            custom_commands: false,
            builtin_commands: Default::default(),
            api_definitions: false,
            api_deployments: false,
        }
    }

    pub fn show_custom_commands(builtin_commands: BTreeSet<String>) -> Self {
        Self {
            environments: false,
            components: false,
            custom_commands: true,
            builtin_commands,
            api_definitions: false,
            api_deployments: false,
        }
    }

    pub fn show_api_definitions() -> Self {
        Self {
            environments: false,
            components: false,
            custom_commands: false,
            builtin_commands: Default::default(),
            api_definitions: true,
            api_deployments: false,
        }
    }

    pub fn show_api_deployments() -> Self {
        Self {
            environments: false,
            components: true,
            custom_commands: false,
            builtin_commands: Default::default(),
            api_definitions: true,
            api_deployments: false,
        }
    }

    pub fn environments(&self) -> bool {
        self.environments
    }

    pub fn components(&self) -> bool {
        self.components
    }

    pub fn custom_commands(&self) -> bool {
        self.custom_commands
    }

    pub fn builtin_commands(&self) -> &BTreeSet<String> {
        &self.builtin_commands
    }

    pub fn api_definitions(&self) -> bool {
        self.api_definitions
    }

    pub fn api_deployments(&self) -> bool {
        self.api_deployments
    }
}

#[derive(Debug)]
pub struct ComponentStubInterfaces {
    pub stub_interface_name: String,
    pub component_name: ComponentName,
    pub exported_interfaces_per_stub_resource: BTreeMap<String, String>,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[clap(rename_all = "kebab_case")]
pub enum AppBuildStep {
    GenWit,
    Componentize,
    Link,
    AddMetadata,
    GenBridge,
    GenBridgeRepl,
}

#[derive(Debug, Clone)]
pub struct BridgeSdkTarget {
    pub component_name: ComponentName,
    pub agent_type: AgentType,
    pub target_language: GuestLanguage,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CustomBridgeSdkTarget {
    pub agent_type_names: HashSet<AgentTypeName>,
    pub target_language: Option<GuestLanguage>,
    pub output_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TemplateName(String);

impl TemplateName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for TemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TemplateName {
    fn from(value: String) -> Self {
        TemplateName(value)
    }
}

impl From<&str> for TemplateName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

pub fn includes_from_yaml_file(source: &Path) -> Vec<String> {
    fs::read_to_string(source)
        .ok()
        .and_then(|source| app_raw::Application::from_yaml_str(source.as_str()).ok())
        .map(|app| {
            if app.includes.is_empty() {
                vec!["**/golem.yaml".to_string()]
            } else {
                app.includes
            }
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct WithSource<T> {
    pub source: PathBuf,
    pub value: T,
}

impl<T> WithSource<T> {
    pub fn new(source: PathBuf, value: T) -> Self {
        Self { source, value }
    }
}

impl<T: Default> Default for WithSource<T> {
    fn default() -> Self {
        Self {
            source: Default::default(),
            value: T::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default, EnumIter)]
pub enum DependencyType {
    #[default]
    /// Dynamic ("stubless") wasm-rpc
    DynamicWasmRpc,
    /// Static (composed with compiled stub) wasm-rpc
    StaticWasmRpc,
    /// Composes the two WASM components together
    Wasm,
}

impl DependencyType {
    pub const STATIC_WASM_RPC: &'static str = "static-wasm-rpc";
    pub const WASM_RPC: &'static str = "wasm-rpc";
    pub const WASM: &'static str = "wasm";

    pub fn as_str(&self) -> &'static str {
        match self {
            DependencyType::DynamicWasmRpc => Self::WASM_RPC,
            DependencyType::StaticWasmRpc => Self::STATIC_WASM_RPC,
            DependencyType::Wasm => Self::WASM,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            DependencyType::DynamicWasmRpc => "WASM RPC dependency",
            DependencyType::StaticWasmRpc => "Statically composed WASM RPC dependency",
            DependencyType::Wasm => "WASM component dependency",
        }
    }

    pub fn is_wasm_rpc(&self) -> bool {
        matches!(
            self,
            DependencyType::DynamicWasmRpc | DependencyType::StaticWasmRpc
        )
    }

    pub fn interactively_selectable_types() -> Vec<Self> {
        Self::iter()
            .filter(|dep_type| dep_type != &DependencyType::StaticWasmRpc)
            .collect()
    }
}

impl FromStr for DependencyType {
    type Err = String;

    fn from_str(str: &str) -> Result<Self, Self::Err> {
        match str {
            Self::WASM_RPC => Ok(Self::DynamicWasmRpc),
            Self::STATIC_WASM_RPC => Ok(Self::StaticWasmRpc),
            Self::WASM => Ok(Self::Wasm),
            _ => {
                let all = DependencyType::iter()
                    .map(|dt| format!("\"{dt}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!(
                    "Unknown dependency type: {str}. Expected one of {all}"
                ))
            }
        }
    }
}

impl Display for DependencyType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinaryComponentSource {
    AppComponent { name: ComponentName },
    LocalFile { path: PathBuf },
    Url { url: Url },
}

impl Display for BinaryComponentSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryComponentSource::AppComponent { name } => write!(f, "{name}"),
            BinaryComponentSource::LocalFile { path } => write!(f, "{}", path.display()),
            BinaryComponentSource::Url { url } => write!(f, "{url}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DependentComponent {
    pub source: BinaryComponentSource,
    pub dep_type: DependencyType,
}

impl DependentComponent {
    pub fn from_raw(
        validation: &mut ValidationBuilder,
        source: &Path,
        dependency: app_raw::Dependency,
    ) -> Option<Self> {
        let (dep, _) = validation.with_context_returning(
            vec![("source", source.to_string_lossy().to_string())],
            |validation| {
                let dep_type = DependencyType::from_str(&dependency.type_);
                if let Ok(dep_type) = dep_type {
                    let binary_component_source = match (dependency.target, dependency.path, dependency.url)
                    {
                        (Some(target_name), None, None) => Some(BinaryComponentSource::AppComponent {
                            name: ComponentName(target_name),
                        }),
                        (None, Some(path), None) => Some(BinaryComponentSource::LocalFile {
                            path: Path::new(&path).to_path_buf(),
                        }),
                        (None, None, Some(url)) => match Url::from_str(&url) {
                            Ok(url) => Some(BinaryComponentSource::Url { url }),
                            Err(_) => {
                                validation.add_error(format!(
                                    "Invalid URL for component dependency: {}",
                                    url.log_color_highlight()
                                ));
                                None
                            }
                        },
                        (None, None, None) => {
                            validation.add_error(format!(
                                "Missing one of the {}/{}/{} fields for component dependency",
                                "target".log_color_error_highlight(),
                                "path".log_color_error_highlight(),
                                "url".log_color_error_highlight()
                            ));
                            None
                        }
                        _ => {
                            validation.add_error(format!(
                                "Only one of the {}/{}/{} fields can be specified for a component dependency",
                                "target".log_color_error_highlight(),
                                "path".log_color_error_highlight(),
                                "url".log_color_error_highlight()
                            ));
                            None
                        }
                    };

                    binary_component_source.map(|source| Self { source, dep_type })
                } else {
                    validation.add_error(format!(
                        "Unknown component dependency type: {}",
                        dependency.type_.log_color_error_highlight()
                    ));
                    None
                }
            });
        dep
    }

    pub fn from_raw_vec(
        validation: &mut ValidationBuilder,
        source: &Path,
        dependencies: Vec<app_raw::Dependency>,
    ) -> BTreeSet<Self> {
        dependencies
            .into_iter()
            .filter_map(|dependencies| Self::from_raw(validation, source, dependencies))
            .collect()
    }

    pub fn as_dependent_app_component(&self) -> Option<DependentAppComponent> {
        match &self.source {
            BinaryComponentSource::AppComponent { name } => Some(DependentAppComponent {
                name: name.clone(),
                dep_type: self.dep_type,
            }),
            _ => None,
        }
    }
}

impl PartialOrd for DependentComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DependentComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.source.cmp(&other.source)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DependentAppComponent {
    pub name: ComponentName,
    pub dep_type: DependencyType,
}

impl PartialOrd for DependentAppComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DependentAppComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Clone, Debug)]
pub struct ApplicationNameAndEnvironments {
    pub application_name: WithSource<ApplicationName>,
    pub environments: BTreeMap<EnvironmentName, app_raw::Environment>,
}

#[derive(Clone, Debug)]
pub struct Application {
    application_name: WithSource<ApplicationName>,
    environments: BTreeMap<EnvironmentName, app_raw::Environment>,
    component_preset_selector: ComponentPresetSelector,
    all_sources: BTreeSet<PathBuf>,
    // TODO: atomic
    #[allow(unused)]
    temp_dir: Option<WithSource<String>>,
    resolved_temp_dir: PathBuf,
    wit_deps: WithSource<Vec<String>>,
    components:
        BTreeMap<ComponentName, WithSource<(ComponentProperties, ComponentLayerProperties)>>,
    custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
    clean: Vec<WithSource<String>>,
    http_api_deployments:
        BTreeMap<EnvironmentName, BTreeMap<Domain, WithSource<HttpApiDeploymentDeployProperties>>>,
    bridge_sdks: WithSource<app_raw::BridgeSdks>,
}

impl Application {
    pub fn environments_from_raw_apps(
        apps: &[app_raw::ApplicationWithSource],
    ) -> ValidatedResult<ApplicationNameAndEnvironments> {
        build_environments(apps)
    }

    pub fn application_name(&self) -> &ApplicationName {
        &self.application_name.value
    }

    pub fn environment_name(&self) -> &EnvironmentName {
        &self.component_preset_selector.environment
    }

    pub fn environments(&self) -> &BTreeMap<EnvironmentName, app_raw::Environment> {
        &self.environments
    }

    pub fn from_raw_apps(
        application_name: WithSource<ApplicationName>,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        component_presets: ComponentPresetSelector,
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Self> {
        build_application(application_name, environments, component_presets, apps)
    }

    pub fn all_sources(&self) -> &BTreeSet<PathBuf> {
        &self.all_sources
    }

    pub fn component_count(&self) -> usize {
        self.components.len()
    }

    pub fn component_names(&self) -> impl Iterator<Item = &ComponentName> {
        self.components.keys()
    }

    pub fn has_any_component(&self) -> bool {
        !self.components.is_empty()
    }

    pub fn contains_component(&self, component_name: &ComponentName) -> bool {
        self.components.contains_key(component_name)
    }

    pub fn common_custom_commands(
        &self,
    ) -> &HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>> {
        &self.custom_commands
    }

    pub fn common_clean(&self) -> &Vec<WithSource<String>> {
        &self.clean
    }

    pub fn wit_deps(&self) -> Vec<PathBuf> {
        self.wit_deps
            .value
            .iter()
            .cloned()
            .map(|path| self.wit_deps.source.join(path))
            .collect()
    }

    pub fn all_custom_commands(&self) -> BTreeSet<String> {
        let mut custom_commands = BTreeSet::new();
        custom_commands.extend(self.component_names().flat_map(|component_name| {
            self.component(component_name)
                .custom_commands()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        }));
        custom_commands.extend(self.custom_commands.keys().cloned());
        custom_commands
    }

    pub fn temp_dir(&self) -> &Path {
        &self.resolved_temp_dir
    }

    pub fn task_result_marker_dir(&self) -> PathBuf {
        self.temp_dir().join("task-results")
    }

    pub fn bridge_sdks(&self) -> &app_raw::BridgeSdks {
        &self.bridge_sdks.value
    }

    pub fn bridge_sdk_dir(
        &self,
        agent_type_name: &AgentTypeName,
        language: GuestLanguage,
    ) -> PathBuf {
        match self
            .bridge_sdks
            .value
            .for_language(language)
            .and_then(|sdk| sdk.output_dir.as_ref())
        {
            Some(output_dir) => self.bridge_sdks.source.join(output_dir),
            None => self
                .temp_dir()
                .join("bridge-sdk")
                .join(language.id())
                .join(bridge_client_directory_name(agent_type_name)),
        }
    }

    pub fn repl_root_dir(&self, language: GuestLanguage) -> PathBuf {
        self.temp_dir().join("repl").join(language.id())
    }

    pub fn repl_root_bridge_sdk_dir(&self, language: GuestLanguage) -> PathBuf {
        self.repl_root_dir(language).join("bridge-sdk")
    }

    pub fn repl_metadata_json(&self, language: GuestLanguage) -> PathBuf {
        self.repl_root_dir(language).join("repl-metadata.json")
    }

    pub fn repl_cli_commands_metadata_json(&self, language: GuestLanguage) -> PathBuf {
        self.repl_root_dir(language)
            .join("repl-cli-commands-metadata.json")
    }

    pub fn repl_history_file(&self, language: ReplLanguage) -> PathBuf {
        self.temp_dir()
            .join("repl-history")
            .join(language.id())
            .join(".repl_history")
    }

    pub fn component<'a>(&'a self, component_name: &'a ComponentName) -> Component<'a> {
        Component {
            component_name,
            temp_dir: self.temp_dir(),
            properties: self
                .components
                .get(component_name)
                .unwrap_or_else(|| panic!("Component not found: {component_name}")),
        }
    }

    pub fn component_dependencies(
        &self,
        component_name: &ComponentName,
    ) -> &BTreeSet<DependentComponent> {
        &self
            .components
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {component_name}"))
            .value
            .0
            .dependencies
    }

    pub fn http_api_deployments(
        &self,
        environment: &EnvironmentName,
    ) -> Option<&BTreeMap<Domain, WithSource<HttpApiDeploymentDeployProperties>>> {
        self.http_api_deployments.get(environment)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentPresetName(pub String);

impl FromStr for ComponentPresetName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_lower_kebab_case_identifier("Component preset", s)?;
        Ok(Self(s.to_string()))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentPresetSelector {
    pub environment: EnvironmentName,
    pub presets: Vec<ComponentPresetName>,
}

#[derive(Debug, Clone)]
struct PartitionedComponentPresets {
    custom_presets: IndexMap<String, ComponentLayerProperties>,
    default_custom_preset: Option<String>,

    env_presets: IndexMap<String, ComponentLayerProperties>,
}

impl PartitionedComponentPresets {
    fn new(presets: IndexMap<String, app_raw::ComponentLayerProperties>) -> Self {
        let mut default_custom_preset = None;
        let mut custom_presets = IndexMap::new();
        let mut env_presets = IndexMap::new();

        for (preset_name, properties) in presets {
            match preset_name.strip_prefix(APP_ENV_PRESET_PREFIX) {
                Some(env_name) => {
                    env_presets.insert(env_name.to_string(), properties.into());
                }
                None => {
                    if properties.default == Some(app_raw::Marker)
                        || default_custom_preset.is_none()
                    {
                        default_custom_preset = Some(preset_name.clone());
                    }
                    custom_presets.insert(preset_name, properties.into());
                }
            }
        }

        Self {
            custom_presets,
            default_custom_preset,
            env_presets,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentLayerId {
    TemplateCommon(TemplateName),
    TemplateEnvironmentPresets(TemplateName),
    TemplateCustomPresets(TemplateName),
    ComponentCommon(ComponentName),
    ComponentEnvironmentPresets(ComponentName),
    ComponentCustomPresets(ComponentName),
}

impl Display for ComponentLayerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentLayerId::TemplateCommon(template_name) => {
                write!(f, "template:{template_name}:common")
            }
            ComponentLayerId::TemplateEnvironmentPresets(template_name) => {
                write!(f, "template:{template_name}:environment-presets")
            }
            ComponentLayerId::TemplateCustomPresets(template_name) => {
                write!(f, "template:{template_name}:custom-presets")
            }
            ComponentLayerId::ComponentCommon(component_name) => {
                write!(f, "component:{component_name}:common")
            }
            ComponentLayerId::ComponentEnvironmentPresets(component_name) => {
                write!(f, "component:{component_name}:environment-presets")
            }
            ComponentLayerId::ComponentCustomPresets(component_name) => {
                write!(f, "component:{component_name}:custom-presets")
            }
        }
    }
}

impl ComponentLayerId {
    pub fn is_template(&self) -> bool {
        match self {
            ComponentLayerId::TemplateCommon(_)
            | ComponentLayerId::TemplateEnvironmentPresets(_)
            | ComponentLayerId::TemplateCustomPresets(_) => true,
            ComponentLayerId::ComponentCommon(_)
            | ComponentLayerId::ComponentEnvironmentPresets(_)
            | ComponentLayerId::ComponentCustomPresets(_) => false,
        }
    }

    pub fn is_environment_preset(&self) -> bool {
        match self {
            ComponentLayerId::TemplateEnvironmentPresets(_)
            | ComponentLayerId::ComponentEnvironmentPresets(_) => true,
            ComponentLayerId::TemplateCommon(_)
            | ComponentLayerId::TemplateCustomPresets(_)
            | ComponentLayerId::ComponentCommon(_)
            | ComponentLayerId::ComponentCustomPresets(_) => false,
        }
    }

    pub fn component_name(&self) -> Option<&ComponentName> {
        match self {
            ComponentLayerId::TemplateCommon(_)
            | ComponentLayerId::TemplateEnvironmentPresets(_)
            | ComponentLayerId::TemplateCustomPresets(_) => None,
            ComponentLayerId::ComponentCommon(component_name)
            | ComponentLayerId::ComponentEnvironmentPresets(component_name)
            | ComponentLayerId::ComponentCustomPresets(component_name) => Some(component_name),
        }
    }

    pub fn template_name(&self) -> Option<&TemplateName> {
        match self {
            ComponentLayerId::TemplateCommon(template_name)
            | ComponentLayerId::TemplateEnvironmentPresets(template_name)
            | ComponentLayerId::TemplateCustomPresets(template_name) => Some(template_name),
            ComponentLayerId::ComponentCommon(_)
            | ComponentLayerId::ComponentEnvironmentPresets(_)
            | ComponentLayerId::ComponentCustomPresets(_) => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ComponentLayerId::TemplateCommon(template_name)
            | ComponentLayerId::TemplateEnvironmentPresets(template_name)
            | ComponentLayerId::TemplateCustomPresets(template_name) => template_name.as_str(),
            ComponentLayerId::ComponentCommon(component_name)
            | ComponentLayerId::ComponentEnvironmentPresets(component_name)
            | ComponentLayerId::ComponentCustomPresets(component_name) => component_name.as_str(),
        }
    }

    fn parent_ids_from_raw_template_references(
        parent_ids: app_raw::LenientTokenList,
    ) -> Vec<ComponentLayerId> {
        parent_ids
            .into_vec()
            .into_iter()
            .map(|parent_id| Self::TemplateCustomPresets(TemplateName(parent_id.to_string())))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentLayer {
    id: ComponentLayerId,
    parents: Vec<ComponentLayerId>,
    properties: ComponentLayerPropertiesKind,
}

#[derive(Debug, Clone, Serialize)]
enum ComponentLayerPropertiesKind {
    Empty,
    Common(Box<ComponentLayerProperties>),
    Presets {
        presets: IndexMap<String, ComponentLayerProperties>,
        default_preset: String,
    },
}

#[derive(Debug, Clone)]
pub struct ComponentLayerApplyContext {
    env: minijinja::Environment<'static>,
    component_name: Option<ComponentName>,
}

impl ComponentLayerApplyContext {
    pub fn new(id: &ComponentLayerId) -> Self {
        Self {
            env: Self::new_template_env(),
            component_name: id.component_name().cloned(),
        }
    }

    fn new_template_env() -> minijinja::Environment<'static> {
        let mut env = minijinja::Environment::new();

        env.add_filter("to_snake_case", |str: &str| str.to_snake_case());

        env.add_filter("to_kebab_case", |str: &str| str.to_kebab_case());
        env.add_filter("to_lower_camel_case", |str: &str| str.to_lower_camel_case());
        env.add_filter("to_pascal_case", |str: &str| str.to_pascal_case());
        env.add_filter("to_shouty_kebab_case", |str: &str| {
            str.to_shouty_kebab_case()
        });
        env.add_filter("to_shouty_snake_case", |str: &str| {
            str.to_shouty_snake_case()
        });
        env.add_filter("to_snake_case", |str: &str| str.to_snake_case());
        env.add_filter("to_title_case", |str: &str| str.to_title_case());
        env.add_filter("to_train_case", |str: &str| str.to_train_case());
        env.add_filter("to_upper_camel_case", |str: &str| str.to_upper_camel_case());

        env
    }

    fn template_env(&self) -> &minijinja::Environment<'_> {
        &self.env
    }

    fn template_context(&self) -> Option<impl Serialize> {
        self.component_name.as_ref().map(|component_name| {
            minijinja::context! {
                componentName => component_name.0.as_str(),
                component_name => component_name.0.as_str(),
            }
        })
    }
}

impl Layer for ComponentLayer {
    type Id = ComponentLayerId;
    type Value = ComponentLayerProperties;
    type Selector = ComponentPresetSelector;
    type AppliedSelection = String;
    type ApplyContext = ComponentLayerApplyContext;
    type ApplyError = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn parent_layers(&self) -> &[Self::Id] {
        self.parents.as_slice()
    }

    fn apply_onto_parent(
        &self,
        ctx: &Self::ApplyContext,
        selector: &Self::Selector,
        value: &mut Self::Value,
    ) -> Result<(), Self::ApplyError> {
        let (property_layers_to_apply, selection): (
            Vec<&ComponentLayerProperties>,
            Option<Self::AppliedSelection>,
        ) = match &self.properties {
            ComponentLayerPropertiesKind::Empty => (vec![], None),
            ComponentLayerPropertiesKind::Common(properties) => (vec![properties], None),
            ComponentLayerPropertiesKind::Presets {
                presets,
                default_preset,
            } => {
                let select_default_preset = || -> Result<(
                    Vec<&ComponentLayerProperties>,
                    Option<Self::AppliedSelection>,
                ), String>{
                    Ok((
                        vec![presets.get(default_preset).ok_or_else(|| {
                            format!(
                                "Default preset '{}' for component layer '{}' not found!",
                                default_preset.log_color_highlight(),
                                self.id.to_string().log_color_highlight(),
                            )
                        })?],
                        Some(default_preset.clone()),
                    ))
                };

                if self.id.is_environment_preset() {
                    (
                        presets.get(&selector.environment.0).into_iter().collect(),
                        Some(format!("app-env:{}", selector.environment.0)),
                    )
                } else if selector.presets.is_empty() {
                    select_default_preset()?
                } else {
                    let mut selected_presets = Vec::new();
                    let mut selected_properties = Vec::new();
                    for preset in &selector.presets {
                        if let Some(properties) = presets.get(&preset.0) {
                            selected_presets.push(preset);
                            selected_properties.push(properties);
                        }
                    }

                    if selected_presets.is_empty() {
                        select_default_preset()?
                    } else {
                        (
                            selected_properties,
                            Some(selected_presets.iter().map(|p| &p.0).join(", ")),
                        )
                    }
                }
            }
        };
        let selection = selection.as_ref();
        let id = self.id();

        if !property_layers_to_apply.is_empty() {
            value.applied_layers.push((id.clone(), selection.cloned()))
        }

        for properties in property_layers_to_apply {
            let template_env = ctx.template_env();
            let template_ctx = self
                .id
                .is_template()
                .then(|| ctx.template_context())
                .flatten();
            let template_ctx = template_ctx.as_ref();

            value.source_wit.apply_layer(
                id,
                selection,
                properties
                    .source_wit
                    .value()
                    .render_or_clone(template_env, template_ctx)
                    .map_err(|err| format!("Failed to render sourceWit: {}", err))?,
            );

            value.generated_wit.apply_layer(
                id,
                selection,
                properties
                    .generated_wit
                    .value()
                    .render_or_clone(template_env, template_ctx)
                    .map_err(|err| format!("Failed to render generatedWit: {}", err))?,
            );

            value.component_wasm.apply_layer(
                id,
                selection,
                properties
                    .component_wasm
                    .value()
                    .render_or_clone(template_env, template_ctx)
                    .map_err(|err| format!("Failed to render componentWasm: {}", err))?,
            );

            value.linked_wasm.apply_layer(
                id,
                selection,
                properties
                    .linked_wasm
                    .value()
                    .render_or_clone(template_env, template_ctx)
                    .map_err(|err| format!("Failed to render linkedWasm: {}", err))?,
            );

            value.build.apply_layer(
                id,
                selection,
                (
                    properties.build_merge_mode.unwrap_or_default(),
                    properties
                        .build
                        .value()
                        .render_or_clone(template_env, template_ctx)
                        .map_err(|err| format!("Failed to render build: {}", err))?,
                ),
            );

            value.custom_commands.apply_layer(
                id,
                selection,
                (
                    MapMergeMode::Upsert,
                    properties
                        .custom_commands
                        .value()
                        .render_or_clone(template_env, template_ctx)
                        .map_err(|err| format!("Failed to render customCommands: {}", err))?,
                ),
            );

            value.clean.apply_layer(
                id,
                selection,
                (
                    VecMergeMode::Append,
                    properties
                        .clean
                        .value()
                        .render_or_clone(template_env, template_ctx)
                        .map_err(|err| format!("Failed to render clean: {}", err))?,
                ),
            );

            value
                .component_type
                .apply_layer(id, selection, *properties.component_type.value());

            value.files.apply_layer(
                id,
                selection,
                (
                    properties.files_merge_mode.unwrap_or_default(),
                    properties.files.value().clone(),
                ),
            );

            value.plugins.apply_layer(
                id,
                selection,
                (
                    properties.plugins_merge_mode.unwrap_or_default(),
                    properties.plugins.value().clone(),
                ),
            );

            value.env.apply_layer(
                id,
                selection,
                (
                    properties.env_merge_mode.unwrap_or_default(),
                    properties.env.value().clone(),
                ),
            );

            value.dependencies.apply_layer(
                id,
                selection,
                (
                    properties.dependencies_merge_mode.unwrap_or_default(),
                    properties.dependencies.value().clone(),
                ),
            )
        }

        Ok(())
    }

    fn root_id_to_context(id: &Self::Id) -> Self::ApplyContext {
        ComponentLayerApplyContext::new(id)
    }
}

#[derive(Clone, Debug)]
pub struct Component<'a> {
    component_name: &'a ComponentName,
    temp_dir: &'a Path,
    properties: &'a WithSource<(ComponentProperties, ComponentLayerProperties)>,
}

impl<'a> Component<'a> {
    pub fn name(&self) -> &ComponentName {
        self.component_name
    }

    pub fn component_type(&self) -> AppComponentType {
        self.properties().component_type
    }

    pub fn guess_language(&self) -> Option<GuestLanguage> {
        self.applied_layers().iter().find_map(|(id, _)| {
            id.template_name()
                .and_then(|template_name| match template_name.as_str() {
                    "ts" => Some(GuestLanguage::TypeScript),
                    "rust" => Some(GuestLanguage::Rust),
                    _ => None,
                })
        })
    }

    pub fn source(&self) -> &Path {
        &self.properties.source
    }

    pub fn applied_layers(&self) -> &[(ComponentLayerId, Option<String>)] {
        self.layer_properties().applied_layers.as_slice()
    }

    pub fn source_dir(&self) -> &Path {
        let parent = self.source().parent().unwrap_or_else(|| {
            panic!(
                "Failed to get parent for component, source: {}",
                self.source().display()
            )
        });
        if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        }
    }

    pub fn properties(&self) -> &ComponentProperties {
        &self.properties.value.0
    }

    pub fn layer_properties(&self) -> &ComponentLayerProperties {
        &self.properties.value.1
    }

    pub fn name_as_safe_path_elem(&self) -> String {
        self.component_name.as_str().replace(":", "_")
    }

    pub fn source_wit(&self) -> PathBuf {
        self.source_dir().join(&self.properties().source_wit)
    }

    pub fn generated_base_wit(&self) -> PathBuf {
        self.temp_dir
            .join("generated-base-wit")
            .join(self.name_as_safe_path_elem())
    }

    pub fn generated_base_wit_exports_package_dir(
        &self,
        exports_package_name: &wit_parser::PackageName,
    ) -> PathBuf {
        self.generated_base_wit()
            .join("deps")
            .join(format!(
                "{}_{}",
                exports_package_name.namespace, exports_package_name.name
            ))
            .join("exports.wit")
    }

    pub fn generated_wit(&self) -> PathBuf {
        self.source_dir()
            .join(self.properties().generated_wit.clone())
    }

    pub fn wasm(&self) -> PathBuf {
        self.source_dir()
            .join(self.properties().component_wasm.clone())
    }

    /// Temporary target of the component composition (linking) step
    pub fn temp_linked_wasm(&self) -> PathBuf {
        self.temp_dir
            .join("temp-linked-wasm")
            .join(format!("{}.wasm", self.component_name.as_str()))
    }

    /// The final linked component WASM
    pub fn final_linked_wasm(&self) -> PathBuf {
        self.properties()
            .linked_wasm
            .as_ref()
            .map(|linked_wasm| self.source_dir().join(linked_wasm))
            .unwrap_or_else(|| {
                self.temp_dir
                    .join("final-linked-wasm")
                    .join(format!("{}.wasm", self.component_name.as_str()))
            })
    }

    pub fn agent_type_extraction_source_wasm(&self) -> PathBuf {
        let custom_source = self.build_commands().iter().find_map(|step| match step {
            app_raw::BuildCommand::AgentWrapper(app_raw::GenerateAgentWrapper {
                based_on_compiled_wasm,
                ..
            }) => Some(based_on_compiled_wasm),
            app_raw::BuildCommand::ComposeAgentWrapper(app_raw::ComposeAgentWrapper {
                with_agent,
                ..
            }) => Some(with_agent),
            _ => None,
        });

        custom_source
            .map(|path| self.source_dir().join(path))
            .unwrap_or_else(|| self.final_linked_wasm())
    }

    /// File for storing extracted agent types
    pub fn extracted_agent_types(&self, source_wasm_path: &Path) -> PathBuf {
        self.temp_dir.join("extracted-agent-types").join(format!(
            "{}-{}.json",
            self.component_name.as_str(),
            blake3::hash(source_wasm_path.display().to_string().as_bytes()).to_hex()
        ))
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.properties().env
    }

    pub fn files(&self) -> &Vec<InitialComponentFile> {
        &self.properties().files
    }

    pub fn plugins(&self) -> &Vec<PluginInstallation> {
        &self.properties().plugins
    }

    fn client_base_build_dir(&self) -> PathBuf {
        self.temp_dir.join("client")
    }

    pub fn client_temp_build_dir(&self) -> PathBuf {
        self.client_base_build_dir()
            .join(self.name_as_safe_path_elem())
            .join("temp-build")
    }

    pub fn client_wasm(&self) -> PathBuf {
        self.client_base_build_dir()
            .join(self.name_as_safe_path_elem())
            .join("client.wasm")
    }

    pub fn client_wit(&self) -> PathBuf {
        self.client_base_build_dir()
            .join(self.name_as_safe_path_elem())
            .join("wit")
    }

    pub fn is_deployable(&self) -> bool {
        self.properties().component_type.is_deployable()
    }

    pub fn custom_commands(&self) -> &BTreeMap<String, Vec<app_raw::ExternalCommand>> {
        &self.properties().custom_commands
    }

    pub fn build_commands(&self) -> &Vec<app_raw::BuildCommand> {
        &self.properties().build
    }

    pub fn clean(&self) -> &Vec<String> {
        &self.properties().clean
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentLayerProperties {
    #[serde(
        serialize_with = "ComponentLayerProperties::serialize_applied_layers",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub applied_layers: Vec<(ComponentLayerId, Option<String>)>,

    pub source_wit: OptionalProperty<ComponentLayer, String>,
    pub generated_wit: OptionalProperty<ComponentLayer, String>,
    pub component_wasm: OptionalProperty<ComponentLayer, String>,
    pub linked_wasm: OptionalProperty<ComponentLayer, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    pub build: VecProperty<ComponentLayer, app_raw::BuildCommand>,
    pub custom_commands: MapProperty<ComponentLayer, String, Vec<app_raw::ExternalCommand>>,
    pub clean: VecProperty<ComponentLayer, String>,
    pub component_type: OptionalProperty<ComponentLayer, AppComponentType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    pub files: VecProperty<ComponentLayer, app_raw::InitialComponentFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    pub plugins: VecProperty<ComponentLayer, app_raw::PluginInstallation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    pub env: MapProperty<ComponentLayer, String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies_merge_mode: Option<VecMergeMode>,
    pub dependencies: VecProperty<ComponentLayer, app_raw::Dependency>,
}

impl From<app_raw::ComponentLayerProperties> for ComponentLayerProperties {
    fn from(value: app_raw::ComponentLayerProperties) -> Self {
        Self {
            applied_layers: vec![],
            source_wit: value.source_wit.into(),
            generated_wit: value.generated_wit.into(),
            component_wasm: value.component_wasm.into(),
            linked_wasm: value.linked_wasm.into(),
            build_merge_mode: value.build_merge_mode,
            build: value.build.into(),
            custom_commands: value.custom_commands.into(),
            clean: value.clean.into(),
            component_type: value.component_type.into(),
            files_merge_mode: value.files_merge_mode,
            files: value.files.unwrap_or_default().into(),
            plugins_merge_mode: value.plugins_merge_mode,
            plugins: value.plugins.unwrap_or_default().into(),
            env_merge_mode: value.env_merge_mode,
            env: value.env.unwrap_or_default().into(),
            dependencies_merge_mode: value.dependencies_merge_mode,
            dependencies: value.dependencies.unwrap_or_default().into(),
        }
    }
}

impl ComponentLayerProperties {
    pub fn compact_traces(&mut self) {
        self.source_wit.compact_trace();
        self.generated_wit.compact_trace();
        self.component_wasm.compact_trace();
        self.linked_wasm.compact_trace();
        self.build.compact_trace();
        self.custom_commands.compact_trace();
        self.clean.compact_trace();
        self.component_type.compact_trace();
        self.files.compact_trace();
        self.plugins.compact_trace();
        self.env.compact_trace();
        self.dependencies.compact_trace();
    }

    pub fn with_compacted_traces(&self) -> Self {
        let mut props = self.clone();
        props.compact_traces();
        props
    }

    pub fn serialize_applied_layers<S>(
        applied_layers: &[(ComponentLayerId, Option<String>)],
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        applied_layers
            .iter()
            .map(|(id, selection)| match selection {
                Some(selection) => {
                    format!("{}[{}]", id.name(), selection.as_str())
                }
                None => id.name().to_string(),
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
}

#[derive(Clone, Debug)]
pub struct ComponentProperties {
    pub source_wit: String,
    pub generated_wit: String,
    pub component_wasm: String,
    pub linked_wasm: Option<String>,
    pub build: Vec<app_raw::BuildCommand>,
    pub custom_commands: BTreeMap<String, Vec<app_raw::ExternalCommand>>,
    pub clean: Vec<String>,
    pub component_type: AppComponentType,
    pub files: Vec<InitialComponentFile>,
    pub plugins: Vec<PluginInstallation>,
    pub env: BTreeMap<String, String>,
    pub dependencies: BTreeSet<DependentComponent>,
}

impl ComponentProperties {
    fn from_merged(
        validation: &mut ValidationBuilder,
        source: &Path,
        merged: &ComponentLayerProperties,
    ) -> Self {
        let files =
            InitialComponentFile::from_raw_vec(validation, source, merged.files.value().clone());
        let plugins =
            PluginInstallation::from_raw_vec(validation, source, merged.plugins.value().clone());
        let dependencies = DependentComponent::from_raw_vec(
            validation,
            source,
            merged.dependencies.value().clone(),
        );

        let properties = Self {
            source_wit: merged.source_wit.value().clone().unwrap_or_default(),
            generated_wit: merged.generated_wit.value().clone().unwrap_or_default(),
            component_wasm: merged.component_wasm.value().clone().unwrap_or_default(),
            linked_wasm: merged.linked_wasm.value().clone(),
            build: merged.build.value().clone(),
            custom_commands: merged
                .custom_commands
                .value()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            clean: merged.clean.value().clone(),
            component_type: (*merged.component_type.value()).unwrap_or_default(),
            files,
            plugins,
            env: Self::validate_and_normalize_env(validation, merged.env.value()),
            dependencies,
        };

        for (name, value) in [
            ("sourceWit", &properties.source_wit),
            ("generatedWit", &properties.generated_wit),
            ("componentWasm", &properties.component_wasm),
        ] {
            if value.is_empty() {
                validation.add_error(format!(
                    "Property {} is empty or undefined",
                    name.log_color_highlight()
                ));
            }
        }

        properties
    }

    fn validate_and_normalize_env(
        validation: &mut ValidationBuilder,
        env: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
    ) -> BTreeMap<String, String> {
        env.into_iter()
            .map(|(key, value)| {
                let key = key.as_ref();
                let value = value.as_ref();

                let upper_case_key = key.to_uppercase();
                if upper_case_key.as_str() != key {
                    validation.add_error(format!(
                        "Only uppercase environment variable names are allowed, found: {}",
                        key.log_color_highlight()
                    ));
                }
                if upper_case_key.starts_with("GOLEM_") {
                    validation.add_warn(format!(
                        concat!(
                        "Using environment names starting with 'GOLEM_' ({}) is not recommended, ",
                        "as those are reserved for variables set by Golem and might be overridden."
                        ),
                        key.log_color_highlight()
                    ));
                }
                (upper_case_key, value.to_string())
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentFilePathWithPermissions {
    pub path: ComponentFilePath,
    pub permissions: ComponentFilePermissions,
}

impl ComponentFilePathWithPermissions {
    pub fn extend_path(&mut self, path: &str) -> Result<(), String> {
        self.path.extend(path)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialComponentFile {
    pub source: InitialComponentFileSource,
    pub target: ComponentFilePathWithPermissions,
}

impl InitialComponentFile {
    pub fn from_raw(
        validation: &mut ValidationBuilder,
        source: &Path,
        file: app_raw::InitialComponentFile,
    ) -> Option<InitialComponentFile> {
        let source = InitialComponentFileSource::new(&file.source_path, source)
            .map_err(|err| {
                validation.push_context("source", file.source_path.to_string());
                validation.add_error(err);
                validation.pop_context();
            })
            .ok()?;

        Some(InitialComponentFile {
            source,
            target: ComponentFilePathWithPermissions {
                path: file.target_path,
                permissions: file
                    .permissions
                    .unwrap_or(ComponentFilePermissions::ReadOnly),
            },
        })
    }

    pub fn from_raw_vec(
        validation: &mut ValidationBuilder,
        source: &Path,
        files: Vec<app_raw::InitialComponentFile>,
    ) -> Vec<Self> {
        files
            .into_iter()
            .filter_map(|file| InitialComponentFile::from_raw(validation, source, file))
            .collect::<Vec<_>>()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialComponentFileSource(Url);

impl InitialComponentFileSource {
    pub fn new(url_string: &str, relative_to: &Path) -> Result<Self, String> {
        // Try to parse the URL as an absolute URL
        let url = Url::parse(url_string).or_else(|_| {
            // If that fails, try to parse it as a relative path
            let canonical_relative_to = relative_to
                .parent()
                .expect("Failed to get parent")
                .canonicalize()
                .map_err(|_| {
                    format!(
                        "Failed to canonicalize relative path: {}",
                        relative_to.log_color_highlight()
                    )
                })?;

            let source = canonical_relative_to.join(PathBuf::from(url_string));
            Url::from_file_path(&source).map_err(|_| {
                format!(
                    "Failed to convert source ({}) to URL",
                    source.log_color_highlight(),
                )
            })
        })?;

        let source_path_scheme = url.scheme();
        let supported_schemes = ["http", "https", "file", ""];
        if !supported_schemes.contains(&source_path_scheme) {
            return Err(format!(
                "Unsupported source path scheme: {}, supported schemes {}:",
                source_path_scheme.log_color_highlight(),
                supported_schemes.join(", ")
            ));
        }
        Ok(Self(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn into_url(self) -> Url {
        self.0
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallation {
    pub account: Option<String>,
    pub name: String,
    pub version: String,
    pub parameters: HashMap<String, String>,
}

impl PluginInstallation {
    pub fn from_raw(
        _validation: &mut ValidationBuilder,
        _source: &Path,
        file: app_raw::PluginInstallation,
    ) -> Option<PluginInstallation> {
        Some(PluginInstallation {
            account: file.account,
            name: file.name,
            version: file.version,
            parameters: file.parameters,
        })
    }

    pub fn from_raw_vec(
        validation: &mut ValidationBuilder,
        source: &Path,
        files: Vec<app_raw::PluginInstallation>,
    ) -> Vec<Self> {
        files
            .into_iter()
            .filter_map(|file| PluginInstallation::from_raw(validation, source, file))
            .collect::<Vec<_>>()
    }
}

mod app_builder {
    use crate::fuzzy::FuzzySearch;
    use crate::log::LogColorize;
    use crate::model::app::{
        Application, ApplicationNameAndEnvironments, BinaryComponentSource, ComponentLayer,
        ComponentLayerId, ComponentLayerProperties, ComponentLayerPropertiesKind,
        ComponentPresetName, ComponentPresetSelector, ComponentProperties, DependencyType,
        PartitionedComponentPresets, TemplateName, WithSource, DEFAULT_TEMP_DIR,
    };
    use crate::model::app_raw;
    use crate::model::cascade::store::Store;
    use crate::model::http_api::HttpApiDeploymentDeployProperties;
    use crate::validation::{ValidatedResult, ValidationBuilder};
    use crate::{fs, fuzzy};
    use colored::Colorize;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::ComponentName;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::EnvironmentName;
    use golem_common::model::http_api_deployment::{
        HttpApiDeploymentAgentOptions, HttpApiDeploymentAgentSecurity, HttpApiDeploymentCreation,
        SecuritySchemeAgentSecurity, TestSessionHeaderAgentSecurity,
    };
    use indexmap::IndexMap;
    use itertools::Itertools;
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::fmt::Debug;
    use std::path::{Path, PathBuf};

    // Load full manifest EXCEPT environments
    pub fn build_application(
        application_name: WithSource<ApplicationName>,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        component_presets: ComponentPresetSelector,
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Application> {
        AppBuilder::build_app(application_name, environments, component_presets, apps)
    }

    // Load only environments
    pub fn build_environments(
        apps: &[app_raw::ApplicationWithSource],
    ) -> ValidatedResult<ApplicationNameAndEnvironments> {
        AppBuilder::build_environments(apps)
    }

    #[derive(Debug, PartialEq, Eq, Hash)]
    enum UniqueSourceCheckedEntityKey {
        App,
        Include,
        TempDir,
        WitDeps,
        CustomCommand(String),
        Template(TemplateName),
        Component(ComponentName),
        Environment(EnvironmentName),
        Bridge,
    }

    impl UniqueSourceCheckedEntityKey {
        fn entity_kind(&self) -> &'static str {
            let property = "Property";
            match self {
                UniqueSourceCheckedEntityKey::App => property,
                UniqueSourceCheckedEntityKey::Include => property,
                UniqueSourceCheckedEntityKey::TempDir => property,
                UniqueSourceCheckedEntityKey::WitDeps => property,
                UniqueSourceCheckedEntityKey::CustomCommand(_) => "Custom command",
                UniqueSourceCheckedEntityKey::Template(_) => "Template",
                UniqueSourceCheckedEntityKey::Component(_) => "Component",
                UniqueSourceCheckedEntityKey::Environment(_) => "Environment",
                UniqueSourceCheckedEntityKey::Bridge => "Bridge",
            }
        }

        fn entity_name(self) -> String {
            match self {
                UniqueSourceCheckedEntityKey::App => "app".log_color_highlight().to_string(),
                UniqueSourceCheckedEntityKey::Include => {
                    "include".log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::TempDir => {
                    "tempDir".log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::WitDeps => {
                    "witDeps".log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::CustomCommand(command_name) => {
                    command_name.log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::Template(template_name) => {
                    template_name.as_str().log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::Component(component_name) => {
                    component_name.as_str().log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::Environment(environment_name) => {
                    environment_name.0.log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::Bridge => "bridge".log_color_highlight().to_string(),
            }
        }
    }

    #[derive(Default)]
    struct AppBuilder {
        // For environment build
        app: Option<WithSource<ApplicationName>>,
        default_environment_names: BTreeSet<EnvironmentName>,
        environments: IndexMap<EnvironmentName, app_raw::Environment>,

        // For app build
        include: Vec<String>,
        temp_dir: Option<WithSource<String>>,
        wit_deps: WithSource<Vec<String>>,
        custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
        clean: Vec<WithSource<String>>,

        raw_component_names: HashSet<String>,
        component_names_to_source: BTreeMap<ComponentName, PathBuf>,
        component_custom_presets: BTreeSet<ComponentPresetName>,
        component_layer_store: Store<ComponentLayer>,

        components:
            BTreeMap<ComponentName, WithSource<(ComponentProperties, ComponentLayerProperties)>>,

        http_api_deployments: BTreeMap<
            EnvironmentName,
            BTreeMap<Domain, WithSource<HttpApiDeploymentDeployProperties>>,
        >,

        bridge_sdks: WithSource<app_raw::BridgeSdks>,

        all_sources: BTreeSet<PathBuf>,
        entity_sources: HashMap<UniqueSourceCheckedEntityKey, Vec<PathBuf>>,
    }

    impl AppBuilder {
        // NOTE: build_app DOES NOT include environments, those are preloaded with build_environments, so
        //       flows that do not use manifest otherwise won't get blocked by high-level validation errors,
        //       and we do not "steal" manifest loading logs from those which do use the manifest fully.
        fn build_app(
            application_name: WithSource<ApplicationName>,
            environments: BTreeMap<EnvironmentName, app_raw::Environment>,
            component_presets: ComponentPresetSelector,
            apps: Vec<app_raw::ApplicationWithSource>,
        ) -> ValidatedResult<Application> {
            let mut builder = Self::default();
            let mut validation = ValidationBuilder::default();

            for app in apps {
                builder.add_raw_app(&mut validation, app);
            }

            // TODO: atomic: validate presets used in envs and template references
            //               before component resolve, and skip if they are not valid
            builder.resolve_and_validate_components(&mut validation, &component_presets);
            builder.validate_dependency_targets(&mut validation);
            builder.validate_unique_sources(&mut validation);
            builder.validate_http_api_deployments(&mut validation, &environments);

            let resolved_temp_dir = {
                match builder.temp_dir.as_ref() {
                    Some(temp_dir) => temp_dir.source.join(&temp_dir.value),
                    None => Path::new(DEFAULT_TEMP_DIR).to_path_buf(),
                }
            };

            validation.build(Application {
                environments,
                component_preset_selector: component_presets,
                application_name,
                all_sources: builder.all_sources,
                temp_dir: builder.temp_dir,
                resolved_temp_dir,
                wit_deps: builder.wit_deps,
                components: builder.components,
                custom_commands: builder.custom_commands,
                clean: builder.clean,
                http_api_deployments: builder.http_api_deployments,
                bridge_sdks: builder.bridge_sdks,
            })
        }

        // NOTE: Unlike build_app, here we do not consume the source apps, so they can be
        //       used for build_app. For more info on this separation, see build_app.
        fn build_environments(
            apps: &[app_raw::ApplicationWithSource],
        ) -> ValidatedResult<ApplicationNameAndEnvironments> {
            let mut builder = Self::default();
            let mut validation = ValidationBuilder::default();

            for app in apps {
                builder.add_raw_app_environments_only(&mut validation, app);
            }

            if builder.default_environment_names.len() > 1 {
                validation.add_error(format!(
                    "Only one environment can be marked as default! Environments marked as default: {}",
                    builder.default_environment_names
                        .iter()
                        .map(|pn| pn.0.log_color_highlight())
                        .join(", ")
                ));
            } else if builder.default_environment_names.is_empty() {
                match builder.environments.len() {
                    0 => {
                        validation
                            .add_error("At least one environment has to be defined!".to_string());
                    }
                    _ => {
                        builder.environments.iter_mut().next().unwrap().1.default =
                            Some(app_raw::Marker);
                    }
                }
            }

            let application_name = match builder.app {
                Some(application_name) => application_name,
                None => {
                    validation.add_error(
                        format!(
                            "Application name not found. Please specify it in you root {} application manifest with the `{}` property!",
                            "golem.yaml".log_color_highlight(),
                            "app".log_color_highlight(),
                        ),
                    );
                    WithSource::new(
                        "<unknown>".into(),
                        ApplicationName("<undefined>".to_string()),
                    )
                }
            };

            validation.build(ApplicationNameAndEnvironments {
                application_name,
                environments: builder.environments.into_iter().collect(),
            })
        }

        fn add_entity_source(&mut self, key: UniqueSourceCheckedEntityKey, source: &Path) -> bool {
            let sources = self.entity_sources.entry(key).or_default();
            let is_first = sources.is_empty();
            sources.push(source.to_path_buf());
            is_first
        }

        fn add_raw_app(
            &mut self,
            validation: &mut ValidationBuilder,
            app: app_raw::ApplicationWithSource,
        ) {
            validation.with_context(
                vec![("source", app.source.to_string_lossy().to_string())],
                |validation| {
                    let app_source_dir = fs::parent_or_err(&app.source).expect("Failed to get parent");
                    self.all_sources.insert(app.source.clone());

                    if let Some(dir) = app.application.temp_dir {
                        if self
                            .add_entity_source(UniqueSourceCheckedEntityKey::TempDir, &app.source)
                        {
                            self.temp_dir =
                                Some(WithSource::new(app_source_dir.to_path_buf(), dir));
                        }
                    }

                    if !app.application.includes.is_empty()
                        && self
                        .add_entity_source(UniqueSourceCheckedEntityKey::Include, &app.source)
                    {
                        self.include = app.application.includes;
                    }

                    if !app.application.wit_deps.is_empty()
                        && self
                        .add_entity_source(UniqueSourceCheckedEntityKey::WitDeps, &app.source)
                    {
                        self.wit_deps =
                            WithSource::new(app_source_dir.to_path_buf(), app.application.wit_deps);
                    }

                    for (template_name, template) in app.application.component_templates {
                        let template_name = TemplateName::from(template_name);
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::Template(template_name.clone()),
                            &app.source,
                        ) {
                            self.add_component_template(validation, template_name, template);
                        }
                    }

                    for (component_name, component) in app.application.components {
                        let component_name = match ComponentName::try_from(component_name.as_str())
                        {
                            Ok(component_name) => component_name,
                            Err(err) => {
                                validation.add_error(format!(
                                    "Invalid component name: {}. {}",
                                    component_name.log_color_error_highlight(),
                                    err
                                ));
                                ComponentName(component_name)
                            }
                        };
                        let unique_key =
                            UniqueSourceCheckedEntityKey::Component(component_name.clone());
                        if self.add_entity_source(unique_key, &app.source) {
                            self.raw_component_names.insert(component_name.0.clone());
                            self.component_names_to_source
                                .insert(component_name.clone(), app.source.clone());
                            self.add_component(validation, component_name, component);
                        }
                    }

                    for (command_name, command) in app.application.custom_commands {
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::CustomCommand(command_name.clone()),
                            &app.source,
                        ) {
                            self.custom_commands.insert(
                                command_name,
                                WithSource::new(app_source_dir.to_path_buf(), command),
                            );
                        }
                    }

                    self.clean.extend(
                        app.application
                            .clean
                            .into_iter()
                            .map(|path| WithSource::new(app.source.to_path_buf(), path)),
                    );

                    if let Some(http_api) = app.application.http_api {
                        for (environment, deployments) in http_api.deployments {
                            for api_deployment in deployments {
                                let deployments =
                                    self.http_api_deployments.entry(environment.clone()).or_default();

                                let agents = api_deployment.agents
                                    .into_iter()
                                    .map(|(k, v)|
                                        (
                                            k,
                                            HttpApiDeploymentAgentOptions {
                                                security: resolve_agent_security(validation, &v)
                                            }
                                        )
                                    )
                                    .collect();

                                deployments.entry(api_deployment.domain).or_insert(WithSource::new(
                                    app.source.to_path_buf(),
                                    HttpApiDeploymentDeployProperties {
                                        webhooks_url: api_deployment.webhook_url.unwrap_or_else(HttpApiDeploymentCreation::default_webhooks_url),
                                        agents
                                    },
                                ));
                            }
                        }
                    }

                    if let Some(bridge) = app.application.bridge {
                        if self
                            .add_entity_source(UniqueSourceCheckedEntityKey::Bridge, app_source_dir)
                        {
                            self.bridge_sdks =
                                WithSource::new(app_source_dir.to_path_buf(), bridge);

                            for (target_language, sdk_targets) in
                                self.bridge_sdks.value.for_all_used_languages()
                            {
                                let sdk_targets = sdk_targets
                                    .agents
                                    .clone()
                                    .into_vec();
                                let non_unique_targets = sdk_targets.iter()
                                    .counts()
                                    .into_iter()
                                    .filter(|(_, count)| *count > 1)
                                    .collect::<Vec<_>>();

                                validation.with_context(
                                    vec![("bridge SDK language", target_language.to_string())],
                                    |validation| {
                                        if !non_unique_targets.is_empty() {
                                            validation.add_error(format!(
                                                "Duplicated bridge SDK agent targets: {}",
                                                non_unique_targets
                                                    .iter()
                                                    .map(|(target, _)| target
                                                        .log_color_error_highlight())
                                                    .join(", ")
                                            ));
                                        }

                                        if sdk_targets.len() > 1 && sdk_targets.iter().any(|t| t == "*") {
                                            validation.add_warn(format!(
                                                "Including \"*\" as language target will match all agents, no need for adding other targets: {}",
                                                sdk_targets
                                                    .iter()
                                                    .map(|target| target.log_color_highlight())
                                                    .join(", ")
                                            ));
                                        }
                                    },
                                );
                            }
                        }
                    }
                });
        }

        fn add_raw_app_environments_only(
            &mut self,
            validation: &mut ValidationBuilder,
            app: &app_raw::ApplicationWithSource,
        ) {
            validation.with_context(
                vec![("source", app.source.to_string_lossy().to_string())],
                |validation| {
                    if let Some(app_name) = &app.application.app {
                        if self.add_entity_source(UniqueSourceCheckedEntityKey::App, &app.source) {
                            let app_name = match app_name.parse::<ApplicationName>() {
                                Ok(app_name) => app_name,
                                Err(err) => {
                                    validation.add_error(format!(
                                        "Invalid application name: {}, {}",
                                        app_name.log_color_highlight(),
                                        err.log_color_error_highlight()
                                    ));
                                    ApplicationName(app_name.to_string())
                                }
                            };

                            self.app = Some(WithSource::new(app.source.clone(), app_name));
                        }
                    }

                    for (environment_name, environment) in &app.application.environments {
                        let environment_name = match environment_name.parse::<EnvironmentName>() {
                            Ok(environment_name) => environment_name,
                            Err(err) => {
                                validation.add_error(format!(
                                    "Invalid environment name: {}, {}",
                                    environment_name.log_color_highlight(),
                                    err.log_color_error_highlight()
                                ));
                                EnvironmentName(environment_name.clone())
                            }
                        };

                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::Environment(environment_name.clone()),
                            &app.source,
                        ) {
                            if environment.default == Some(app_raw::Marker) {
                                self.default_environment_names
                                    .insert(environment_name.clone());
                            };

                            self.environments
                                .insert(environment_name.clone(), environment.clone());
                            validation.with_context(
                                vec![("environment", environment_name.0)],
                                |_validation| {
                                    // TODO: atomic: validate environment
                                },
                            );
                        }
                    }
                },
            );
        }

        fn add_component_template(
            &mut self,
            validation: &mut ValidationBuilder,
            template_name: TemplateName,
            template: app_raw::ComponentTemplate,
        ) {
            validation.with_context(vec![("template", template_name.0.clone())], |validation| {
                if let Some(err) = self
                    .component_layer_store
                    .add_layer(ComponentLayer {
                        id: ComponentLayerId::TemplateCommon(template_name.clone()),
                        parents: ComponentLayerId::parent_ids_from_raw_template_references(
                            template.templates,
                        ),
                        properties: ComponentLayerPropertiesKind::Common(Box::new(
                            template.component_properties.into(),
                        )),
                    })
                    .err()
                {
                    validation.add_error(err.to_string())
                }

                let presets = PartitionedComponentPresets::new(template.presets);

                if let Some(err) = self
                    .component_layer_store
                    .add_layer(ComponentLayer {
                        id: ComponentLayerId::TemplateEnvironmentPresets(template_name.clone()),
                        parents: vec![ComponentLayerId::TemplateCommon(template_name.clone())],
                        properties: {
                            if presets.env_presets.is_empty() {
                                ComponentLayerPropertiesKind::Empty
                            } else {
                                ComponentLayerPropertiesKind::Presets {
                                    presets: presets.env_presets,
                                    default_preset: "".to_string(),
                                }
                            }
                        },
                    })
                    .err()
                {
                    validation.add_error(err.to_string())
                }

                if let Some(err) = self
                    .component_layer_store
                    .add_layer(ComponentLayer {
                        id: ComponentLayerId::TemplateCustomPresets(template_name.clone()),
                        parents: vec![ComponentLayerId::TemplateEnvironmentPresets(
                            template_name.clone(),
                        )],
                        properties: {
                            match presets.default_custom_preset {
                                Some(default_custom_preset) => {
                                    ComponentLayerPropertiesKind::Presets {
                                        presets: presets.custom_presets,
                                        default_preset: default_custom_preset,
                                    }
                                }
                                None => ComponentLayerPropertiesKind::Empty,
                            }
                        },
                    })
                    .err()
                {
                    validation.add_error(err.to_string())
                }
            });
        }

        fn add_component(
            &mut self,
            validation: &mut ValidationBuilder,
            component_name: ComponentName,
            component: app_raw::Component,
        ) {
            validation.with_context(
                vec![("component", component_name.0.clone())],
                |validation| {
                    if let Some(err) = self
                        .component_layer_store
                        .add_layer(ComponentLayer {
                            id: ComponentLayerId::ComponentCommon(component_name.clone()),
                            parents: ComponentLayerId::parent_ids_from_raw_template_references(
                                component.templates,
                            ),
                            properties: ComponentLayerPropertiesKind::Common(Box::new(
                                component.component_properties.into(),
                            )),
                        })
                        .err()
                    {
                        validation.add_error(err.to_string())
                    }

                    let presets = PartitionedComponentPresets::new(component.presets);

                    presets.custom_presets.keys().for_each(|preset_name| {
                        self.component_custom_presets
                            .insert(ComponentPresetName(preset_name.clone()));
                    });

                    if let Some(err) = self
                        .component_layer_store
                        .add_layer(ComponentLayer {
                            id: ComponentLayerId::ComponentEnvironmentPresets(
                                component_name.clone(),
                            ),
                            parents: vec![ComponentLayerId::ComponentCommon(
                                component_name.clone(),
                            )],
                            properties: {
                                if presets.env_presets.is_empty() {
                                    ComponentLayerPropertiesKind::Empty
                                } else {
                                    ComponentLayerPropertiesKind::Presets {
                                        presets: presets.env_presets,
                                        default_preset: "".to_string(),
                                    }
                                }
                            },
                        })
                        .err()
                    {
                        validation.add_error(err.to_string())
                    }

                    if let Some(err) = self
                        .component_layer_store
                        .add_layer(ComponentLayer {
                            id: ComponentLayerId::ComponentCustomPresets(component_name.clone()),
                            parents: vec![ComponentLayerId::ComponentEnvironmentPresets(
                                component_name.clone(),
                            )],
                            properties: {
                                match presets.default_custom_preset {
                                    Some(default_custom_preset) => {
                                        ComponentLayerPropertiesKind::Presets {
                                            presets: presets.custom_presets,
                                            default_preset: default_custom_preset,
                                        }
                                    }
                                    None => ComponentLayerPropertiesKind::Empty,
                                }
                            },
                        })
                        .err()
                    {
                        validation.add_error(err.to_string())
                    }
                },
            );
        }

        fn validate_unique_sources(&mut self, validation: &mut ValidationBuilder) {
            let entity_sources = std::mem::take(&mut self.entity_sources);
            entity_sources
                .into_iter()
                .filter(|(_, sources)| sources.len() > 1)
                .for_each(|(key, sources)| {
                    validation.add_error(format!(
                        "{} {} is defined in multiple sources: {}",
                        key.entity_kind(),
                        key.entity_name(),
                        sources
                            .into_iter()
                            .map(|s| s.log_color_highlight())
                            .join(", ")
                    ))
                })
        }

        fn validate_dependency_targets(&mut self, validation: &mut ValidationBuilder) {
            for (component_name, component) in &self.components {
                for target in &component.value.0.dependencies {
                    let invalid_source =
                        !self.component_names_to_source.contains_key(component_name);
                    let invalid_target = match &target.source {
                        BinaryComponentSource::AppComponent { name } => {
                            !self.component_names_to_source.contains_key(name)
                        }
                        BinaryComponentSource::LocalFile { path } => {
                            !std::fs::exists(path).unwrap_or(false)
                        }
                        BinaryComponentSource::Url { .. } => false,
                    };
                    let invalid_target_source = match (&target.dep_type, &target.source) {
                        (
                            DependencyType::DynamicWasmRpc,
                            BinaryComponentSource::AppComponent { .. },
                        ) => {
                            false // valid
                        }
                        (
                            DependencyType::StaticWasmRpc,
                            BinaryComponentSource::AppComponent { .. },
                        ) => {
                            false // valid
                        }
                        (DependencyType::Wasm, _) => {
                            false // valid
                        }
                        _ => true,
                    };

                    if invalid_source || invalid_target || invalid_target_source {
                        validation.with_context(
                            vec![("source", component.source.to_string_lossy().to_string())],
                            |validation| {
                                if invalid_source {
                                    validation.add_error(format!(
                                        "{} {} - {} references unknown component: {}\n\n{}",
                                        target.dep_type.describe(),
                                        component_name.as_str().log_color_highlight(),
                                        target.source.to_string().log_color_highlight(),
                                        component_name.as_str().log_color_error_highlight(),
                                        self.available_components(component_name.as_str())
                                    ))
                                }
                                if invalid_target {
                                    validation.add_error(format!(
                                        "{} {} - {} references unknown target component: {}\n\n{}",
                                        target.dep_type.describe(),
                                        component_name.as_str().log_color_highlight(),
                                        target.source.to_string().log_color_highlight(),
                                        target.source.to_string().log_color_error_highlight(),
                                        self.available_components(&target.source.to_string())
                                    ))
                                }
                                if invalid_target_source {
                                    validation.add_error(format!(
                                        "{} {} - {}: this dependency type only supports local component targets\n",
                                        target.dep_type.describe(),
                                        component_name.as_str().log_color_highlight(),
                                        target.source.to_string().log_color_highlight(),
                                    ))
                                }
                            },
                        );
                    }
                }
            }
        }

        fn resolve_and_validate_components(
            &mut self,
            validation: &mut ValidationBuilder,
            component_presets: &ComponentPresetSelector,
        ) {
            for (component_name, source) in self.component_names_to_source.clone() {
                validation.with_context(
                    vec![
                        ("source", source.to_string_lossy().to_string()),
                        ("component", component_name.to_string()),
                    ],
                    |validation| {
                        self.resolve_and_validate_component(
                            validation,
                            component_presets,
                            source,
                            component_name,
                        );
                    },
                );
            }
        }

        fn resolve_and_validate_component(
            &mut self,
            validation: &mut ValidationBuilder,
            component_presets: &ComponentPresetSelector,
            source: PathBuf,
            component_name: ComponentName,
        ) {
            match self.component_layer_store.value(
                &ComponentLayerId::ComponentCustomPresets(component_name.clone()),
                component_presets,
            ) {
                Ok(component_layer_properties) => {
                    let component_properties = ComponentProperties::from_merged(
                        validation,
                        &source,
                        &component_layer_properties,
                    );
                    self.components.insert(
                        component_name,
                        WithSource::new(source, (component_properties, component_layer_properties)),
                    );
                }
                Err(err) => validation.add_error(format!("Failed to resolve component: {err}")),
            }
        }

        fn validate_http_api_deployments(
            &self,
            validation: &mut ValidationBuilder,
            environments: &BTreeMap<EnvironmentName, app_raw::Environment>,
        ) {
            for (environment, api_deployments) in &self.http_api_deployments {
                if !environments.contains_key(environment) {
                    validation.add_warn(format!(
                        "Unknown environment in manifest: {}\n\n{}",
                        environment.0.log_color_highlight(),
                        self.available_profiles(
                            environments.keys().map(|p| p.0.as_str()),
                            &environment.0
                        )
                    ));
                }

                let mut unique_agents = HashSet::new();
                for api_deployment in api_deployments.values() {
                    for agent in api_deployment.value.agents.keys() {
                        if !unique_agents.insert(agent.clone()) {
                            validation.add_warn(format!(
                                "Agent deployed to multiple domains in environments: {}",
                                agent.0.log_color_highlight(),
                            ));
                        }
                    }
                }
            }
        }

        fn available_profiles<'a, I: IntoIterator<Item = &'a str>>(
            &self,
            available_profiles: I,
            unknown: &str,
        ) -> String {
            self.available_options_help("profiles", "profile names", unknown, available_profiles)
        }

        // TODO: atomic
        #[allow(unused)]
        fn available_templates(&self, _unknown: &str) -> String {
            // TODO: atomic
            /*self.available_options_help(
                "templates",
                "template names",
                unknown,
                self.templates.keys().map(|name| name.as_str()),
            )*/
            todo!()
        }

        fn available_components(&self, unknown: &str) -> String {
            self.available_options_help(
                "components",
                "component names",
                unknown,
                self.raw_component_names.iter().map(|name| name.as_str()),
            )
        }

        fn available_options_help<'a, I: IntoIterator<Item = &'a str>>(
            &self,
            entity_plural: &str,
            entity_name_plural: &str,
            unknown_option: &str,
            name_options: I,
        ) -> String {
            let options = name_options.into_iter().collect::<Vec<_>>();
            if options.is_empty() {
                return format!("No {entity_plural} are defined");
            }

            let fuzzy_search = FuzzySearch::new(options.iter().copied());

            let hint = match fuzzy_search.find(unknown_option) {
                Err(fuzzy::Error::Ambiguous {
                    highlighted_options,
                    ..
                }) => {
                    if highlighted_options.len() == 1 {
                        format!(
                            "Did you mean {}?\n\n",
                            highlighted_options[0].log_color_highlight()
                        )
                    } else {
                        format!(
                            "Did you mean one of {}?\n\n",
                            highlighted_options
                                .iter()
                                .map(|option| option.bold())
                                .join(",")
                        )
                    }
                }
                _ => "".to_string(),
            };

            format!(
                "{}{}\n{}",
                hint,
                format!("Available {entity_name_plural}:").log_color_help_group(),
                options.iter().map(|name| format!("- {name}")).join("\n")
            )
        }
    }

    #[allow(unused)]
    fn check_not_empty(
        validation: &mut ValidationBuilder,
        property_name: &str,
        value: &str,
    ) -> bool {
        let is_empty = value.is_empty();
        if is_empty {
            validation.add_error(format!(
                "Property {} is empty",
                property_name.log_color_highlight()
            ));
        }
        !is_empty
    }

    fn resolve_agent_security(
        validation: &mut ValidationBuilder,
        agent_options: &app_raw::HttpApiDeploymentAgentOptions,
    ) -> Option<HttpApiDeploymentAgentSecurity> {
        match (
            &agent_options.security_scheme,
            &agent_options.test_session_header_name,
        ) {
            (Some(_), Some(_)) => {
                validation.add_error(
                    "Only one of securityScheme and testSessionHeaderName may be provided".into(),
                );
                None
            }
            (Some(security_scheme), None) => Some(HttpApiDeploymentAgentSecurity::SecurityScheme(
                SecuritySchemeAgentSecurity {
                    security_scheme: security_scheme.clone(),
                },
            )),
            (None, Some(test_session_header_name)) => Some(
                HttpApiDeploymentAgentSecurity::TestSessionHeader(TestSessionHeaderAgentSecurity {
                    header_name: test_session_header_name.clone(),
                }),
            ),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::model::app::{Application, ApplicationNameAndEnvironments, ComponentPresetSelector};
    use crate::model::app_raw;
    use assert2::assert;
    use assert2::let_assert;
    use indoc::indoc;
    use std::path::PathBuf;
    use test_r::test;

    #[test]
    fn test_layer_non_matching_defaults() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            componentTemplates:
              malbogle:
                sourceWit: dummy-source.wit
                generatedWit: dummy-generated.wit
                componentWasm: dummy-component.wasm

            components:
              app:main:
                templates: malbogle
                presets:
                  a:
                    sourceWit: a.wit
                  b:
                    sourceWit: b.wit

        "# };

        let app = load_app(
            source,
            &ComponentPresetSelector {
                environment: "local".parse().unwrap(),
                presets: vec!["debug".parse().unwrap()],
            },
        );

        let component_name = "app:main".parse().unwrap();
        let component = app.component(&component_name);

        assert!(component.source_wit() == PathBuf::from("./a.wit"),);
    }

    fn load_app(source: &str, selector: &ComponentPresetSelector) -> Application {
        let raw_app =
            app_raw::ApplicationWithSource::from_yaml_string(PathBuf::from("golem.yaml"), source)
                .unwrap();
        let raw_apps = vec![raw_app];

        let (app_name_and_envs, warns, errors) =
            Application::environments_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let_assert!(
            Some(ApplicationNameAndEnvironments {
                application_name,
                environments
            }) = app_name_and_envs
        );

        let (app, warns, errors) =
            Application::from_raw_apps(application_name, environments, selector.clone(), raw_apps)
                .into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        app.unwrap()
    }
}
