// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::http_api::{HttpApiDeploymentDeployProperties, McpDeploymentDeployProperties};
use crate::bridge_gen::{
    BridgeMode, bridge_client_directory_name, bridge_client_directory_name_for_mode,
    tool_bridge_client_directory_name,
};
use crate::fs;
use crate::log::LogColorize;
use crate::model::app::app_builder::{build_application, build_application_preload};
use crate::model::cascade::layer::Layer;
use crate::model::cascade::property::Property;
use crate::model::cascade::property::json::JsonProperty;
use crate::model::cascade::property::map::{MapMergeMode, MapProperty};
use crate::model::cascade::property::optional::OptionalProperty;
use crate::model::cascade::property::vec::{VecMergeMode, VecProperty};
use crate::model::cascade::store::Store;
use crate::model::repl::ReplLanguage;
use crate::model::template::Template;
use crate::model::{GuestLanguage, app_raw};
use crate::validation::{ValidatedResult, ValidationBuilder};
use anyhow::{Context, anyhow};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath, ComponentName};
use golem_common::model::deployment::DeploymentRetryPolicyDefault;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::quota::{ResourceDefinitionCreation, ResourceName};
use golem_common::model::validate_lower_kebab_case_identifier;
use golem_common::schema::AgentTypeSchema;
use golem_common::schema::tool::Tool;
use heck::{
    ToKebabCase, ToLowerCamelCase, ToPascalCase, ToShoutyKebabCase, ToShoutySnakeCase, ToSnakeCase,
    ToTitleCase, ToTrainCase, ToUpperCamelCase,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Serialize, Serializer};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Formatter;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use url::Url;

const TEMP_DIR: &str = "golem-temp";
const APP_ENV_PRESET_PREFIX: &str = "app-env:";

#[derive(Clone, Debug, Default)]
pub struct BuildConfig {
    pub skip_up_to_date_checks: bool,
    pub skip_check: bool,
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

    pub fn with_skip_check(mut self, skip_check: bool) -> Self {
        self.skip_check = skip_check;
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
            !(matches!(step, AppBuildStep::Check) && self.skip_check)
        } else {
            self.steps_filter.contains(&step)
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApplicationConfig {
    pub offline: bool,
    pub dev_mode: bool,
    pub should_colorize: bool,
    pub enable_wasmtime_fs_cache: bool,
}

#[derive(Debug, Clone)]
pub struct LoadedRawApps {
    pub app_root_dir: PathBuf,
    pub calling_working_dir: PathBuf,
    pub raw_apps: Vec<app_raw::ApplicationWithSource>,
}

#[derive(Debug, Clone)]
pub enum ApplicationSourceMode {
    Automatic,
    ByRootManifest(PathBuf),
    Preloaded(LoadedRawApps),
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
    Check,
    Build,
    AddMetadata,
    GenBridge,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ToolName(String);

impl ToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ToolName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<&str> for ToolName {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err("Tool dependency name must not be empty".to_string())
        } else {
            Ok(Self(value.to_string()))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ComponentDependency {
    Agent(AgentTypeName),
    Tool(ToolName),
}

#[derive(Debug, Clone)]
pub struct BridgeSdkTarget {
    pub component_name: ComponentName,
    pub kind: BridgeSdkTargetKind,
    pub target_language: GuestLanguage,
    pub bridge_mode: BridgeMode,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum BridgeSdkTargetKind {
    Agent(AgentTypeSchema),
    Tool(Tool),
}

impl BridgeSdkTargetKind {
    pub fn display_name(&self) -> &str {
        match self {
            BridgeSdkTargetKind::Agent(agent_type) => agent_type.type_name.as_str(),
            BridgeSdkTargetKind::Tool(tool) => tool.name().unwrap_or_default(),
        }
    }

    pub fn as_agent(&self) -> Option<&AgentTypeSchema> {
        match self {
            BridgeSdkTargetKind::Agent(agent_type) => Some(agent_type),
            BridgeSdkTargetKind::Tool(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CustomBridgeSdkTarget {
    pub agent_type_names: HashSet<AgentTypeName>,
    pub target_language: Option<GuestLanguage>,
    pub output_dir: Option<PathBuf>,
}

pub fn includes_from_yaml_file(source: &Path) -> Vec<String> {
    manifest_metadata_from_yaml_file(source).includes
}

pub fn manifest_metadata_from_yaml_file(source: &Path) -> app_raw::ApplicationMetadata {
    fs::read_to_string(source)
        .ok()
        .and_then(|source| app_raw::ApplicationMetadata::from_yaml_str(source.as_str()).ok())
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

#[derive(Clone, Debug)]
pub struct ApplicationPreload {
    pub application_name: WithSource<ApplicationName>,
    pub environments: BTreeMap<EnvironmentName, app_raw::Environment>,
    pub local_server: Option<WithSource<app_raw::LocalServer>>,
}

#[derive(Clone, Debug)]
pub struct ResolvedLocalServer {
    pub router_addr: Option<String>,
    pub router_port: Option<u16>,
    pub custom_request_port: Option<u16>,
    pub mcp_port: Option<u16>,
    pub ports_file: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub agent_filesystem_root: Option<PathBuf>,
}

impl ResolvedLocalServer {
    pub fn from_raw_with_source(
        local_server: &WithSource<app_raw::LocalServer>,
        app_root_dir: &Path,
    ) -> Self {
        let base_dir = local_server.source.parent().unwrap_or(app_root_dir);
        Self::from_raw_with_base_dir(&local_server.value, base_dir)
    }

    pub fn from_raw_with_base_dir(local_server: &app_raw::LocalServer, base_dir: &Path) -> Self {
        Self {
            router_addr: local_server.router_addr.clone(),
            router_port: local_server.router_port,
            custom_request_port: local_server.custom_request_port,
            mcp_port: local_server.mcp_port,
            ports_file: local_server
                .ports_file
                .as_ref()
                .map(|path| fs::absolute_lexical_path_from_base_dir(path, base_dir)),
            data_dir: local_server
                .data_dir
                .as_ref()
                .map(|path| fs::absolute_lexical_path_from_base_dir(path, base_dir)),
            agent_filesystem_root: local_server
                .agent_filesystem_root
                .as_ref()
                .map(|path| fs::absolute_lexical_path_from_base_dir(path, base_dir)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Application {
    app_root_dir: PathBuf,

    // For template rendering
    app_root_dir_str: String,
    golem_temp_dir_str: String,
    cargo_workspace_mode: bool,

    application_name: WithSource<ApplicationName>,
    environments: BTreeMap<EnvironmentName, app_raw::Environment>,
    component_preset_selector: ComponentPresetSelector,
    all_sources: BTreeSet<PathBuf>,
    components:
        BTreeMap<ComponentName, WithSource<(ComponentProperties, ComponentLayerProperties)>>,
    agents: BTreeMap<AgentTypeName, WithSource<app_raw::Agent>>,
    component_layer_store: Store<ComponentLayer>,
    custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
    clean: Vec<WithSource<String>>,
    http_api_deployments:
        BTreeMap<EnvironmentName, BTreeMap<Domain, WithSource<HttpApiDeploymentDeployProperties>>>,
    mcp_deployments:
        BTreeMap<EnvironmentName, BTreeMap<Domain, WithSource<McpDeploymentDeployProperties>>>,
    agent_secrets_defaults: BTreeMap<EnvironmentName, WithSource<app_raw::JsonObject>>,
    retry_policy_defaults:
        BTreeMap<EnvironmentName, BTreeMap<String, WithSource<DeploymentRetryPolicyDefault>>>,
    resource_definition_defaults:
        BTreeMap<EnvironmentName, BTreeMap<ResourceName, WithSource<ResourceDefinitionCreation>>>,
    bridge_sdks: WithSource<app_raw::BridgeSdks>,
}

/// Resolves the cargo target directory for the cargo project rooted at `manifest_dir`.
///
/// The result is the value used for the `cargoTarget` template variable. Instead of
/// assuming `<manifest_dir>/target`, this asks cargo itself via `cargo metadata`, so it
/// honors `CARGO_TARGET_DIR`, `build.target-dir` in cargo config, and cargo wrapper
/// scripts that redirect the target directory. If cargo cannot be queried (no manifest,
/// cargo not installed, etc.) it falls back to the legacy `<manifest_dir>/target` location.
///
/// Results are memoized per `manifest_dir` for the lifetime of the process, since the
/// resolved target directory is stable for a given manifest.
fn resolve_cargo_target_dir(manifest_dir: &str) -> String {
    use std::sync::{Mutex, OnceLock};

    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some(cached) = cache.lock().unwrap().get(manifest_dir) {
        return cached.clone();
    }

    let resolved = resolve_cargo_target_dir_uncached(manifest_dir);
    cache
        .lock()
        .unwrap()
        .insert(manifest_dir.to_string(), resolved.clone());
    resolved
}

fn resolve_cargo_target_dir_uncached(manifest_dir: &str) -> String {
    let fallback = || format!("{manifest_dir}/target");

    let manifest_path = Path::new(manifest_dir).join("Cargo.toml");
    if !manifest_path.exists() {
        return fallback();
    }

    // Run cargo through whatever `cargo` is on PATH (including wrapper scripts) with the
    // manifest's directory as the working directory, so wrappers that derive the target
    // directory from the current directory resolve it correctly.
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .current_dir(manifest_dir)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            match serde_json::from_slice::<JsonValue>(&output.stdout) {
                Ok(metadata) => metadata
                    .get("target_directory")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string())
                    .unwrap_or_else(fallback),
                Err(_) => fallback(),
            }
        }
        _ => fallback(),
    }
}

impl Application {
    /// Returns the cargo manifest directory used to resolve the `cargoTarget` template
    /// variable for a component located at `component_dir`.
    ///
    /// In a cargo workspace the target directory is shared at the application root; otherwise
    /// it is resolved relative to the component directory. The directory is only used to query
    /// cargo (see [`resolve_cargo_target_dir`]) lazily, when a template actually references
    /// `cargoTarget`.
    fn cargo_manifest_dir_for(&self, component_dir: &str) -> String {
        if self.cargo_workspace_mode {
            self.app_root_dir_str.clone()
        } else {
            component_dir.to_string()
        }
    }

    pub fn preload_from_raw_apps(
        apps: &[app_raw::ApplicationWithSource],
    ) -> ValidatedResult<ApplicationPreload> {
        build_application_preload(apps)
    }

    pub fn language_templates_from_raw_apps(
        apps: &[app_raw::ApplicationWithSource],
    ) -> HashSet<GuestLanguage> {
        apps.iter()
            .flat_map(|app| {
                app.application
                    .component_templates
                    .values()
                    .map(|template| &template.templates)
                    .chain(
                        app.application
                            .components
                            .values()
                            .map(|component| &component.templates),
                    )
                    .flat_map(|templates| templates.clone().into_vec())
                    .filter_map(GuestLanguage::from_id_string)
            })
            .collect()
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

    pub fn app_root_dir(&self) -> &Path {
        &self.app_root_dir
    }

    pub fn from_raw_apps(
        root_dir: PathBuf,
        application_name: WithSource<ApplicationName>,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        local_server: Option<WithSource<app_raw::LocalServer>>,
        component_presets: ComponentPresetSelector,
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Self> {
        build_application(
            root_dir,
            application_name,
            environments,
            local_server,
            component_presets,
            apps,
        )
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

    pub fn agent_names(&self) -> impl Iterator<Item = &AgentTypeName> {
        self.agents.keys()
    }

    pub fn resolve_agents(
        &self,
        mapping: &BTreeMap<AgentTypeName, ComponentName>,
    ) -> anyhow::Result<Agents> {
        let mut resolved_by_type = BTreeMap::new();

        for (agent_type_name, component_name) in mapping {
            let component = self.component(component_name);
            let component_base = component.agent_base_properties();
            let (properties, layer_properties) = self
                .resolve_agent(component_name, agent_type_name, component_base)
                .with_context(|| {
                    format!(
                        "Failed to resolve agent '{}' for component '{}'",
                        agent_type_name.0,
                        component_name.as_str()
                    )
                })?;

            let source = self
                .agents
                .get(agent_type_name)
                .map(|agent| agent.source.clone())
                .unwrap_or_else(|| component.source().to_path_buf());

            resolved_by_type.insert(
                agent_type_name.clone(),
                ResolvedAgent {
                    component_name: component_name.clone(),
                    source,
                    properties,
                    layer_properties,
                },
            );
        }

        Ok(Agents { resolved_by_type })
    }

    fn resolve_agent(
        &self,
        component_name: &ComponentName,
        agent_type_name: &AgentTypeName,
        component_base: app_raw::AgentLayerProperties,
    ) -> anyhow::Result<(AgentProperties, AgentLayerProperties)> {
        let base_component_id = AgentLayerId::Component(component_name.clone());
        let mut agent_layer_store = Store::new();

        agent_layer_store
            .add_layer(AgentLayer {
                id: base_component_id.clone(),
                parents: vec![],
                properties: AgentLayerPropertiesKind::Common(Box::new(component_base)),
            })
            .map_err(|err| anyhow!(err.to_string()))
            .with_context(|| {
                format!(
                    "Failed to add base component agent layer '{}'",
                    base_component_id
                )
            })?;

        let target_id =
            if let Some(agent) = self.agents.get(agent_type_name).map(|agent| &agent.value) {
                let component = self.component(component_name);
                let template_apply_ctx = ComponentLayerApplyContext::new(
                    Some(component_name.clone()),
                    Some(self.app_root_dir_str.clone()),
                    Some(self.golem_temp_dir_str.clone()),
                    fs::path_to_str(component.component_dir())
                        .ok()
                        .map(|s| s.to_string()),
                    fs::path_to_str(component.component_dir())
                        .ok()
                        .map(|component_dir| self.cargo_manifest_dir_for(component_dir)),
                );

                let mut latest_parent_id = base_component_id.clone();
                for template_name in agent.templates.clone().into_vec() {
                    let template_layer_id =
                        ComponentLayerId::TemplateCustomPresets(template_name.clone());
                    let template_layer_props = self
                        .component_layer_store
                        .value(
                            &template_layer_id,
                            &self.component_preset_selector,
                            &template_apply_ctx,
                        )
                        .map_err(|err| anyhow!(err.to_string()))
                        .with_context(|| {
                            format!(
                                "Failed to resolve template layer '{}' for agent '{}'",
                                template_layer_id, agent_type_name.0
                            )
                        })?;

                    let template_agent_props = app_raw::AgentLayerProperties {
                        config: template_layer_props.config.value().clone(),
                        initial_card: template_layer_props.initial_card.value().clone(),
                        env_merge_mode: None,
                        env: Some(template_layer_props.env.value().clone()),
                        plugins_merge_mode: None,
                        plugins: Some(template_layer_props.plugins.value().clone()),
                        files_merge_mode: None,
                        files: Some(template_layer_props.files.value().clone()),
                    };

                    let template_id =
                        AgentLayerId::AgentTemplate(agent_type_name.clone(), template_name);
                    agent_layer_store
                        .add_layer(AgentLayer {
                            id: template_id.clone(),
                            parents: vec![latest_parent_id],
                            properties: AgentLayerPropertiesKind::Common(Box::new(
                                template_agent_props,
                            )),
                        })
                        .map_err(|err| anyhow!(err.to_string()))
                        .with_context(|| {
                            format!("Failed to add agent template layer '{}'", template_id)
                        })?;

                    latest_parent_id = template_id;
                }

                let partitioned = PartitionedAgentPresets::new(agent.presets.clone());

                let common_id = AgentLayerId::AgentCommon(agent_type_name.clone());
                agent_layer_store
                    .add_layer(AgentLayer {
                        id: common_id.clone(),
                        parents: vec![latest_parent_id],
                        properties: AgentLayerPropertiesKind::Common(Box::new(
                            agent.agent_layer_properties(),
                        )),
                    })
                    .map_err(|err| anyhow!(err.to_string()))
                    .with_context(|| format!("Failed to add agent common layer '{}'", common_id))?;

                let env_id = AgentLayerId::AgentEnvironmentPresets(agent_type_name.clone());
                agent_layer_store
                    .add_layer(AgentLayer {
                        id: env_id.clone(),
                        parents: vec![common_id],
                        properties: if partitioned.env_presets.is_empty() {
                            AgentLayerPropertiesKind::Empty
                        } else {
                            AgentLayerPropertiesKind::Presets {
                                presets: partitioned.env_presets,
                                default_preset: EMPTY_STR.to_string(),
                            }
                        },
                    })
                    .map_err(|err| anyhow!(err.to_string()))
                    .with_context(|| {
                        format!("Failed to add agent environment preset layer '{}'", env_id)
                    })?;

                let custom_id = AgentLayerId::AgentCustomPresets(agent_type_name.clone());
                agent_layer_store
                    .add_layer(AgentLayer {
                        id: custom_id.clone(),
                        parents: vec![env_id],
                        properties: match partitioned.default_custom_preset {
                            Some(default_custom_preset) => AgentLayerPropertiesKind::Presets {
                                presets: partitioned.custom_presets,
                                default_preset: default_custom_preset,
                            },
                            None => AgentLayerPropertiesKind::Empty,
                        },
                    })
                    .map_err(|err| anyhow!(err.to_string()))
                    .with_context(|| {
                        format!("Failed to add agent custom preset layer '{}'", custom_id)
                    })?;

                custom_id
            } else {
                base_component_id
            };

        let resolved = AgentLayerProperties::from_store(
            &target_id,
            &self.component_preset_selector,
            &agent_layer_store,
        )
        .map_err(|err| anyhow!(err))
        .with_context(|| format!("Failed to resolve final agent layer '{}'", target_id))?;

        Ok((AgentProperties::from_resolved(&resolved), resolved))
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

    pub fn on_demand_common_dir() -> PathBuf {
        Path::new(TEMP_DIR).join("common")
    }

    pub fn on_demand_common_dir_for_language(language: GuestLanguage) -> PathBuf {
        Self::on_demand_common_dir().join(language.id())
    }

    pub fn temp_dir(&self) -> &Path {
        Path::new(TEMP_DIR)
    }

    pub fn task_result_marker_dir(&self) -> PathBuf {
        self.temp_dir().join("task-results")
    }

    pub fn deployment_agent_secret_defaults(
        &self,
        environment: &EnvironmentName,
    ) -> Option<WithSource<app_raw::JsonObject>> {
        self.agent_secrets_defaults.get(environment).cloned()
    }

    pub fn deployment_retry_policy_defaults(
        &self,
        environment: &EnvironmentName,
    ) -> Vec<DeploymentRetryPolicyDefault> {
        self.retry_policy_defaults
            .get(environment)
            .map(|defaults_by_name| {
                defaults_by_name
                    .values()
                    .map(|default| default.value.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn resource_definition_defaults(
        &self,
        environment: &EnvironmentName,
    ) -> Vec<ResourceDefinitionCreation> {
        self.resource_definition_defaults
            .get(environment)
            .map(|defaults_by_name| {
                defaults_by_name
                    .values()
                    .map(|with_source| with_source.value.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn bridge_sdks(&self) -> &app_raw::BridgeSdks {
        &self.bridge_sdks.value
    }

    pub fn bridge_sdks_source(&self) -> &Path {
        &self.bridge_sdks.source
    }

    pub fn bridge_sdk_dir(
        &self,
        agent_type_name: &AgentTypeName,
        language: GuestLanguage,
        mode: BridgeMode,
    ) -> PathBuf {
        let output_dir = match mode {
            BridgeMode::External => self
                .bridge_sdks
                .value
                .for_language(language)
                .and_then(|sdk| sdk.output_dir.as_ref()),
            BridgeMode::Guest => self
                .bridge_sdks
                .value
                .for_language(language)
                .and_then(|sdk| sdk.guest.as_ref())
                .and_then(|sdk| sdk.output_dir.as_ref()),
        };

        match output_dir {
            Some(output_dir) => self
                .bridge_sdks
                .source
                .join(output_dir)
                .join(bridge_client_directory_name_for_mode(agent_type_name, mode)),
            None => match mode {
                BridgeMode::External => self
                    .temp_dir()
                    .join("bridge-sdk")
                    .join(language.id())
                    .join(bridge_client_directory_name(agent_type_name)),
                BridgeMode::Guest => self
                    .temp_dir()
                    .join("bridge-sdk")
                    .join(language.id())
                    .join(mode.id())
                    .join(bridge_client_directory_name_for_mode(agent_type_name, mode)),
            },
        }
    }

    pub fn dependency_bridge_sdk_dir(
        &self,
        agent_type_name: &AgentTypeName,
        language: GuestLanguage,
    ) -> PathBuf {
        self.temp_dir()
            .join("bridge-sdk")
            .join(language.id())
            .join(BridgeMode::Guest.id())
            .join(bridge_client_directory_name_for_mode(
                agent_type_name,
                BridgeMode::Guest,
            ))
    }

    pub fn tool_bridge_sdk_dir(&self, tool_name: &str, language: GuestLanguage) -> PathBuf {
        let output_dir = self
            .bridge_sdks
            .value
            .for_language(language)
            .and_then(|sdk| sdk.guest.as_ref())
            .and_then(|sdk| sdk.output_dir.as_ref());

        match output_dir {
            Some(output_dir) => self
                .bridge_sdks
                .source
                .join(output_dir)
                .join(tool_bridge_client_directory_name(tool_name)),
            None => self
                .temp_dir()
                .join("bridge-sdk")
                .join(language.id())
                .join(BridgeMode::Guest.id())
                .join(tool_bridge_client_directory_name(tool_name)),
        }
    }

    pub fn dependency_tool_bridge_sdk_dir(
        &self,
        tool_name: &str,
        language: GuestLanguage,
    ) -> PathBuf {
        self.temp_dir()
            .join("bridge-sdk")
            .join(language.id())
            .join(BridgeMode::Guest.id())
            .join(tool_bridge_client_directory_name(tool_name))
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

    pub fn http_api_deployments(
        &self,
        environment: &EnvironmentName,
    ) -> Option<&BTreeMap<Domain, WithSource<HttpApiDeploymentDeployProperties>>> {
        self.http_api_deployments.get(environment)
    }

    pub fn mcp_deployments(
        &self,
        environment: &EnvironmentName,
    ) -> Option<&BTreeMap<Domain, WithSource<McpDeploymentDeployProperties>>> {
        self.mcp_deployments.get(environment)
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

#[derive(Debug, Clone)]
struct PartitionedAgentPresets {
    custom_presets: IndexMap<String, app_raw::AgentLayerProperties>,
    default_custom_preset: Option<String>,
    env_presets: IndexMap<String, app_raw::AgentLayerProperties>,
}

impl PartitionedAgentPresets {
    fn new(presets: IndexMap<String, app_raw::AgentPreset>) -> Self {
        let mut default_custom_preset = None;
        let mut custom_presets = IndexMap::new();
        let mut env_presets = IndexMap::new();

        for (preset_name, preset) in presets {
            match preset_name.strip_prefix(APP_ENV_PRESET_PREFIX) {
                Some(env_name) => {
                    env_presets.insert(env_name.to_string(), preset.into_agent_layer_properties());
                }
                None => {
                    if preset.default == Some(app_raw::Marker) || default_custom_preset.is_none() {
                        default_custom_preset = Some(preset_name.clone());
                    }
                    custom_presets.insert(preset_name, preset.into_agent_layer_properties());
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

impl PartitionedComponentPresets {
    fn new(presets: IndexMap<String, app_raw::ComponentPreset>) -> Self {
        let mut default_custom_preset = None;
        let mut custom_presets = IndexMap::new();
        let mut env_presets = IndexMap::new();

        for (preset_name, preset) in presets {
            match preset_name.strip_prefix(APP_ENV_PRESET_PREFIX) {
                Some(env_name) => {
                    env_presets.insert(
                        env_name.to_string(),
                        preset.into_component_layer_properties().into(),
                    );
                }
                None => {
                    if preset.default == Some(app_raw::Marker) || default_custom_preset.is_none() {
                        default_custom_preset = Some(preset_name.clone());
                    }
                    custom_presets
                        .insert(preset_name, preset.into_component_layer_properties().into());
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
    TemplateCommon(String),
    TemplateEnvironmentPresets(String),
    TemplateCustomPresets(String),
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

    pub fn template_name(&self) -> Option<&str> {
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
            .map(Self::TemplateCustomPresets)
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

const EMPTY_STR: &str = "";

#[derive(Debug, Clone)]
pub struct ComponentLayerApplyContext {
    env: minijinja::Environment<'static>,
    component_name: Option<ComponentName>,
    app_root_dir: Option<String>,
    golem_temp_dir: Option<String>,
    component_dir: Option<String>,
    component_dir_rel: Option<String>,
    cargo_manifest_dir: Option<String>,
    moonbit_build_package_path: Option<String>,
}

impl ComponentLayerApplyContext {
    pub fn new(
        component_name: Option<ComponentName>,
        app_root_dir: Option<String>,
        golem_temp_dir: Option<String>,
        component_dir: Option<String>,
        cargo_manifest_dir: Option<String>,
    ) -> Self {
        // Compute the component directory path relative to the app root, normalized
        // for portability (uses unix-style separators on Windows, avoids
        // canonicalization, and compares paths in their normalized form so that
        // surface differences like `./` or trailing slashes do not change the
        // outcome). Falls back to an empty string if either path is missing or the
        // component directory is not actually under the app root.
        let component_dir_rel = match (app_root_dir.as_deref(), component_dir.as_deref()) {
            (Some(app_root), Some(comp_dir)) => {
                let app_root = fs::normalize_for_comparison(std::path::Path::new(app_root));
                let comp_dir = fs::normalize_for_comparison(std::path::Path::new(comp_dir));
                Some(
                    fs::strip_prefix_or_err(&comp_dir, &app_root)
                        .ok()
                        .and_then(|rel| fs::path_to_unix_str(rel).ok())
                        .unwrap_or_default(),
                )
            }
            _ => None,
        };

        // Subpath, relative to `_build/wasm/<profile>/build/`, where `moon build`
        // emits the .wasm artifact for this component. `moon build` writes either
        // `<root_pkg>.wasm` for a root-level package or `<dir>/<dir>.wasm` for a
        // sub-package. By convention the moon module name equals the application
        // name, which equals the part of `componentName` before `:`. Pre-computing
        // this here lets the moonbit template avoid embedding that branching.
        let moonbit_build_package_path = match (
            component_dir_rel.as_deref(),
            component_name.as_ref().map(|n| n.0.as_str()),
        ) {
            (Some(""), Some(name)) => {
                let root_pkg = name.split(':').next().unwrap_or(name);
                Some(format!("{root_pkg}.wasm"))
            }
            (Some(rel), _) if !rel.is_empty() => Some(format!("{rel}/{rel}.wasm")),
            _ => None,
        };

        Self {
            env: Self::new_template_env(),
            component_name,
            app_root_dir,
            golem_temp_dir,
            component_dir,
            component_dir_rel,
            cargo_manifest_dir,
            moonbit_build_package_path,
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

    fn template_context(&self) -> minijinja::Value {
        minijinja::Value::from_object(ComponentLayerTemplateContext {
            component_name: self.component_name.as_ref().map(|name| name.0.clone()),
            app_root_dir: self.app_root_dir.clone().unwrap_or_default(),
            golem_temp_dir: self.golem_temp_dir.clone().unwrap_or_default(),
            component_dir: self.component_dir.clone().unwrap_or_default(),
            component_dir_rel: self.component_dir_rel.clone().unwrap_or_default(),
            cargo_manifest_dir: self.cargo_manifest_dir.clone(),
            moonbit_build_package_path: self.moonbit_build_package_path.clone().unwrap_or_default(),
        })
    }
}

/// Lazy template context for [`ComponentLayerApplyContext`].
///
/// All values are resolved up-front except `cargoTarget`, which is only resolved
/// (by querying cargo, see [`resolve_cargo_target_dir`]) when a template actually
/// references it. This keeps cargo-free projects (e.g. TypeScript components whose
/// manifests never use `cargoTarget`) from requiring cargo to be installed.
#[derive(Debug)]
struct ComponentLayerTemplateContext {
    component_name: Option<String>,
    app_root_dir: String,
    golem_temp_dir: String,
    component_dir: String,
    component_dir_rel: String,
    cargo_manifest_dir: Option<String>,
    moonbit_build_package_path: String,
}

impl minijinja::value::Object for ComponentLayerTemplateContext {
    fn get_value(self: &std::sync::Arc<Self>, key: &minijinja::Value) -> Option<minijinja::Value> {
        match key.as_str()? {
            "componentName" | "component_name" => Some(
                self.component_name
                    .as_deref()
                    .map(minijinja::Value::from)
                    .unwrap_or_else(|| minijinja::Value::from(())),
            ),
            "appRootDir" => Some(minijinja::Value::from(self.app_root_dir.as_str())),
            "golemTempDir" => Some(minijinja::Value::from(self.golem_temp_dir.as_str())),
            "componentDir" => Some(minijinja::Value::from(self.component_dir.as_str())),
            "componentDirRel" => Some(minijinja::Value::from(self.component_dir_rel.as_str())),
            "cargoTarget" => Some(minijinja::Value::from(
                self.cargo_manifest_dir
                    .as_deref()
                    .map(resolve_cargo_target_dir)
                    .unwrap_or_default(),
            )),
            "moonbitBuildPackagePath" => Some(minijinja::Value::from(
                self.moonbit_build_package_path.as_str(),
            )),
            _ => None,
        }
    }

    fn enumerate(self: &std::sync::Arc<Self>) -> minijinja::value::Enumerator {
        minijinja::value::Enumerator::Str(&[
            "componentName",
            "component_name",
            "appRootDir",
            "golemTempDir",
            "componentDir",
            "componentDirRel",
            "cargoTarget",
            "moonbitBuildPackagePath",
        ])
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
            let template_ctx = self.id.is_template().then(|| ctx.template_context());
            let template_ctx = template_ctx.as_ref();

            value.component_wasm.apply_layer(
                id,
                selection,
                properties
                    .component_wasm
                    .value()
                    .render_or_clone(template_env, template_ctx)
                    .map_err(|err| format!("Failed to render componentWasm: {}", err))?,
            );

            value.output_wasm.apply_layer(
                id,
                selection,
                properties
                    .output_wasm
                    .value()
                    .render_or_clone(template_env, template_ctx)
                    .map_err(|err| format!("Failed to render outputWasm: {}", err))?,
            );

            value.dependency_agents.apply_layer(
                id,
                selection,
                (
                    VecMergeMode::Append,
                    properties
                        .dependency_agents
                        .value()
                        .render_or_clone(template_env, template_ctx)
                        .map_err(|err| format!("Failed to render dependencies: {}", err))?,
                ),
            );

            value.dependency_tools.apply_layer(
                id,
                selection,
                (
                    VecMergeMode::Append,
                    properties
                        .dependency_tools
                        .value()
                        .render_or_clone(template_env, template_ctx)
                        .map_err(|err| format!("Failed to render dependencies: {}", err))?,
                ),
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
                .config
                .apply_layer(id, selection, properties.config.value().clone());

            value.env.apply_layer(
                id,
                selection,
                (
                    properties.env_merge_mode.unwrap_or_default(),
                    properties.env.value().clone(),
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

            value.files.apply_layer(
                id,
                selection,
                (
                    properties.files_merge_mode.unwrap_or_default(),
                    properties.files.value().clone(),
                ),
            );
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
struct ResolvedAgent {
    component_name: ComponentName,
    source: PathBuf,
    properties: AgentProperties,
    layer_properties: AgentLayerProperties,
}

#[derive(Clone, Debug)]
pub struct Agent<'a> {
    agent_type_name: &'a AgentTypeName,
    resolved: &'a ResolvedAgent,
}

impl<'a> Agent<'a> {
    pub fn name(&self) -> &AgentTypeName {
        self.agent_type_name
    }

    pub fn component_name(&self) -> &ComponentName {
        &self.resolved.component_name
    }

    pub fn source(&self) -> &Path {
        &self.resolved.source
    }

    pub fn properties(&self) -> &AgentProperties {
        &self.resolved.properties
    }

    pub fn layer_properties(&self) -> &AgentLayerProperties {
        &self.resolved.layer_properties
    }

    pub fn applied_layers(&self) -> &[(AgentLayerId, Option<String>)] {
        self.layer_properties().applied_layers.as_slice()
    }

    pub fn config(&self) -> Option<&JsonValue> {
        self.resolved.properties.config.as_ref()
    }

    pub fn initial_card(&self) -> Option<&app_raw::ManifestInitialCard> {
        self.resolved.properties.initial_card.as_ref()
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.resolved.properties.env
    }

    pub fn plugins(&self) -> &[app_raw::PluginInstallation] {
        &self.resolved.properties.plugins
    }

    pub fn files(&self) -> &[app_raw::InitialComponentFile] {
        &self.resolved.properties.files
    }
}

#[derive(Clone, Debug)]
pub struct Agents {
    resolved_by_type: BTreeMap<AgentTypeName, ResolvedAgent>,
}

impl Agents {
    pub fn agent<'a>(&'a self, agent_type_name: &'a AgentTypeName) -> Option<Agent<'a>> {
        self.resolved_by_type
            .get(agent_type_name)
            .map(|resolved| Agent {
                agent_type_name,
                resolved,
            })
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

    // TODO: FCL: cleanup this, and make lang ids reserved for template names
    pub fn guess_language(&self) -> Option<GuestLanguage> {
        self.applied_layers().iter().find_map(|(id, _)| {
            id.template_name()
                .and_then(|template_name| match template_name {
                    "ts" => Some(GuestLanguage::TypeScript),
                    "rust" => Some(GuestLanguage::Rust),
                    "scala" => Some(GuestLanguage::Scala),
                    "moonbit" => Some(GuestLanguage::MoonBit),
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

    // The manifest component dir property
    pub fn dir(&self) -> Option<&Path> {
        self.properties().dir.as_deref()
    }

    // Fully resolved component dir
    pub fn component_dir(&self) -> &Path {
        &self.properties.value.0.component_dir
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

    pub fn wasm(&self) -> PathBuf {
        self.component_dir()
            .join(self.properties().component_wasm.clone())
    }

    /// The final output component WASM
    pub fn final_wasm(&self) -> PathBuf {
        self.properties()
            .output_wasm
            .as_ref()
            .map(|output_wasm| self.component_dir().join(output_wasm))
            .unwrap_or_else(|| {
                self.temp_dir
                    .join("final-wasm")
                    .join(format!("{}.wasm", self.component_name.as_str()))
            })
    }

    pub fn agent_type_extraction_source_wasm(&self) -> PathBuf {
        self.final_wasm()
    }

    /// File for storing extracted component metadata (agent types and tools)
    pub fn extracted_component_metadata(&self, source_wasm_path: &Path) -> PathBuf {
        self.temp_dir
            .join("extracted-component-metadata")
            .join(format!(
                "{}-{}.json",
                self.component_name.as_str(),
                blake3::hash(source_wasm_path.display().to_string().as_bytes()).to_hex()
            ))
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.properties().env
    }

    pub fn config(&self) -> &Option<JsonValue> {
        &self.properties().config
    }

    pub fn files(&self) -> &Vec<InitialComponentFile> {
        &self.properties().files
    }

    pub fn plugins(&self) -> &Vec<PluginInstallation> {
        &self.properties().plugins
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

    pub fn agent_base_properties(&self) -> app_raw::AgentLayerProperties {
        app_raw::AgentLayerProperties {
            config: self.layer_properties().config.value().clone(),
            initial_card: self.layer_properties().initial_card.value().clone(),
            env_merge_mode: None,
            env: Some(self.layer_properties().env.value().clone()),
            plugins_merge_mode: None,
            plugins: Some(self.layer_properties().plugins.value().clone()),
            files_merge_mode: None,
            files: Some(self.layer_properties().files.value().clone()),
        }
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

    pub component_wasm: OptionalProperty<ComponentLayer, String>,
    pub output_wasm: OptionalProperty<ComponentLayer, String>,
    pub dependency_agents: VecProperty<ComponentLayer, String>,
    pub dependency_tools: VecProperty<ComponentLayer, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    pub build: VecProperty<ComponentLayer, app_raw::BuildCommand>,
    pub custom_commands: MapProperty<ComponentLayer, String, Vec<app_raw::ExternalCommand>>,
    pub clean: VecProperty<ComponentLayer, String>,
    pub config: JsonProperty<ComponentLayer>,
    pub initial_card: OptionalProperty<ComponentLayer, app_raw::ManifestInitialCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    pub env: MapProperty<ComponentLayer, String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    pub plugins: VecProperty<ComponentLayer, app_raw::PluginInstallation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    pub files: VecProperty<ComponentLayer, app_raw::InitialComponentFile>,
}

impl From<app_raw::ComponentLayerProperties> for ComponentLayerProperties {
    fn from(value: app_raw::ComponentLayerProperties) -> Self {
        Self {
            applied_layers: vec![],
            component_wasm: value.component_wasm.into(),
            output_wasm: value.output_wasm.into(),
            dependency_agents: value.dependencies.agents.into(),
            dependency_tools: value.dependencies.tools.into(),
            build_merge_mode: value.build_merge_mode,
            build: value.build.into(),
            custom_commands: value.custom_commands.into(),
            clean: value.clean.into(),
            config: value.agent_properties.config.into(),
            initial_card: value.agent_properties.initial_card.into(),
            env_merge_mode: value.agent_properties.env_merge_mode,
            env: value.agent_properties.env.unwrap_or_default().into(),
            plugins_merge_mode: value.agent_properties.plugins_merge_mode,
            plugins: value.agent_properties.plugins.unwrap_or_default().into(),
            files_merge_mode: value.agent_properties.files_merge_mode,
            files: value.agent_properties.files.unwrap_or_default().into(),
        }
    }
}

impl ComponentLayerProperties {
    pub fn compact_traces(&mut self) {
        self.component_wasm.compact_trace();
        self.output_wasm.compact_trace();
        self.dependency_agents.compact_trace();
        self.dependency_tools.compact_trace();
        self.build.compact_trace();
        self.custom_commands.compact_trace();
        self.clean.compact_trace();
        self.config.compact_trace();
        self.initial_card.compact_trace();
        self.env.compact_trace();
        self.plugins.compact_trace();
        self.files.compact_trace();
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentLayerId {
    Component(ComponentName),
    AgentTemplate(AgentTypeName, String),
    AgentCommon(AgentTypeName),
    AgentEnvironmentPresets(AgentTypeName),
    AgentCustomPresets(AgentTypeName),
}

impl Display for AgentLayerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentLayerId::Component(component_name) => {
                write!(f, "component:{component_name}")
            }
            AgentLayerId::AgentTemplate(agent_type_name, template_name) => {
                write!(f, "agent:{}:template:{}", agent_type_name.0, template_name)
            }
            AgentLayerId::AgentEnvironmentPresets(agent_type_name) => {
                write!(f, "agent:{}:environment-presets", agent_type_name.0)
            }
            AgentLayerId::AgentCustomPresets(agent_type_name) => {
                write!(f, "agent:{}:custom-presets", agent_type_name.0)
            }
            AgentLayerId::AgentCommon(agent_type_name) => {
                write!(f, "agent:{}:common", agent_type_name.0)
            }
        }
    }
}

impl AgentLayerId {
    pub fn name(&self) -> String {
        self.to_string()
    }

    pub fn is_environment_preset(&self) -> bool {
        matches!(self, AgentLayerId::AgentEnvironmentPresets(_))
    }
}

#[derive(Debug, Clone, Serialize)]
struct AgentLayer {
    id: AgentLayerId,
    parents: Vec<AgentLayerId>,
    properties: AgentLayerPropertiesKind,
}

#[derive(Debug, Clone, Serialize)]
enum AgentLayerPropertiesKind {
    Empty,
    Common(Box<app_raw::AgentLayerProperties>),
    Presets {
        presets: IndexMap<String, app_raw::AgentLayerProperties>,
        default_preset: String,
    },
}

impl Layer for AgentLayer {
    type Id = AgentLayerId;
    type Value = AgentLayerProperties;
    type Selector = ComponentPresetSelector;
    type AppliedSelection = String;
    type ApplyContext = ();
    type ApplyError = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn parent_layers(&self) -> &[Self::Id] {
        &self.parents
    }

    fn apply_onto_parent(
        &self,
        _ctx: &Self::ApplyContext,
        selector: &Self::Selector,
        value: &mut Self::Value,
    ) -> Result<(), Self::ApplyError> {
        let (property_layers_to_apply, selection) = match &self.properties {
            AgentLayerPropertiesKind::Empty => (vec![], None),
            AgentLayerPropertiesKind::Common(properties) => (vec![properties.as_ref()], None),
            AgentLayerPropertiesKind::Presets {
                presets,
                default_preset,
            } => {
                if self.id.is_environment_preset() {
                    (
                        presets
                            .get(&selector.environment.0)
                            .into_iter()
                            .collect::<Vec<_>>(),
                        Some(format!("{APP_ENV_PRESET_PREFIX}{}", selector.environment.0)),
                    )
                } else {
                    let selected = selector
                        .presets
                        .iter()
                        .filter_map(|preset_name| {
                            presets
                                .get(preset_name.0.as_str())
                                .map(|preset| (preset, preset_name.0.as_str()))
                        })
                        .collect::<Vec<_>>();

                    if selected.is_empty() {
                        (
                            presets.get(default_preset).into_iter().collect::<Vec<_>>(),
                            Some(default_preset.to_string()),
                        )
                    } else {
                        (
                            selected
                                .iter()
                                .map(|(preset, _)| *preset)
                                .collect::<Vec<_>>(),
                            Some(selected.iter().map(|(_, name)| *name).join(", ")),
                        )
                    }
                }
            }
        };

        let selection = selection.as_ref();
        let id = self.id();
        if !property_layers_to_apply.is_empty() {
            value.applied_layers.push((id.clone(), selection.cloned()));
        }

        for properties in property_layers_to_apply {
            value
                .config
                .apply_layer(id, selection, properties.config.clone());
            value
                .initial_card
                .apply_layer(id, selection, properties.initial_card.clone());
            value.env.apply_layer(
                id,
                selection,
                (
                    properties.env_merge_mode.unwrap_or_default(),
                    properties.env.clone().unwrap_or_default(),
                ),
            );
            value.plugins.apply_layer(
                id,
                selection,
                (
                    properties.plugins_merge_mode.unwrap_or_default(),
                    properties.plugins.clone().unwrap_or_default(),
                ),
            );
            value.files.apply_layer(
                id,
                selection,
                (
                    properties.files_merge_mode.unwrap_or_default(),
                    properties.files.clone().unwrap_or_default(),
                ),
            );
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentLayerProperties {
    #[serde(
        serialize_with = "AgentLayerProperties::serialize_applied_layers",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub applied_layers: Vec<(AgentLayerId, Option<String>)>,
    config: JsonProperty<AgentLayer>,
    initial_card: OptionalProperty<AgentLayer, app_raw::ManifestInitialCard>,
    env: MapProperty<AgentLayer, String, String>,
    plugins: VecProperty<AgentLayer, app_raw::PluginInstallation>,
    files: VecProperty<AgentLayer, app_raw::InitialComponentFile>,
}

impl AgentLayerProperties {
    fn from_store(
        id: &AgentLayerId,
        selector: &ComponentPresetSelector,
        store: &Store<AgentLayer>,
    ) -> Result<Self, String> {
        store
            .value(id, selector, &())
            .map_err(|err| err.to_string())
    }

    pub fn compact_traces(&mut self) {
        self.config.compact_trace();
        self.initial_card.compact_trace();
        self.env.compact_trace();
        self.plugins.compact_trace();
        self.files.compact_trace();
    }

    pub fn with_compacted_traces(&self) -> Self {
        let mut props = self.clone();
        props.compact_traces();
        props
    }

    pub fn serialize_applied_layers<S>(
        applied_layers: &[(AgentLayerId, Option<String>)],
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
pub struct AgentProperties {
    pub config: Option<JsonValue>,
    pub initial_card: Option<app_raw::ManifestInitialCard>,
    pub env: BTreeMap<String, String>,
    pub plugins: Vec<app_raw::PluginInstallation>,
    pub files: Vec<app_raw::InitialComponentFile>,
}

impl AgentProperties {
    fn from_resolved(layer_properties: &AgentLayerProperties) -> Self {
        Self {
            config: layer_properties.config.value().clone(),
            initial_card: layer_properties.initial_card.value().clone(),
            env: layer_properties
                .env
                .value()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            plugins: layer_properties.plugins.value().clone(),
            files: layer_properties.files.value().clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComponentProperties {
    pub dir: Option<PathBuf>, // Relative path starting from the defining golem.yaml
    pub component_dir: PathBuf, // Resolved canonical component path
    pub component_wasm: String,
    pub output_wasm: Option<String>,
    pub dependencies: Vec<ComponentDependency>,
    pub build: Vec<app_raw::BuildCommand>,
    pub custom_commands: BTreeMap<String, Vec<app_raw::ExternalCommand>>,
    pub clean: Vec<String>,
    pub files: Vec<InitialComponentFile>,
    pub plugins: Vec<PluginInstallation>,
    pub env: BTreeMap<String, String>,
    pub config: Option<JsonValue>,
    pub initial_card: Option<app_raw::ManifestInitialCard>,
}

impl ComponentProperties {
    fn from_merged(
        validation: &mut ValidationBuilder,
        source: &Path,
        dir: Option<PathBuf>,
        component_dir: PathBuf,
        merged: &ComponentLayerProperties,
    ) -> Self {
        let files =
            InitialComponentFile::from_raw_vec(validation, source, merged.files.value().clone());
        let plugins =
            PluginInstallation::from_raw_vec(validation, source, merged.plugins.value().clone());

        let properties = Self {
            dir,
            component_dir,
            component_wasm: merged.component_wasm.value().clone().unwrap_or_default(),
            output_wasm: merged.output_wasm.value().clone(),
            dependencies: Self::validate_dependencies(
                validation,
                merged.dependency_agents.value(),
                merged.dependency_tools.value(),
            ),
            build: merged.build.value().clone(),
            custom_commands: merged
                .custom_commands
                .value()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            clean: merged.clean.value().clone(),
            files,
            plugins,
            env: Self::validate_and_normalize_env(validation, merged.env.value().iter()),
            config: merged.config.value().clone(),
            initial_card: merged.initial_card.value().clone(),
        };

        for (name, value) in [("componentWasm", &properties.component_wasm)] {
            if value.is_empty() {
                validation.add_error(format!(
                    "Property {} is empty or undefined",
                    name.log_color_highlight()
                ));
            }
        }

        properties
    }

    fn validate_dependencies(
        validation: &mut ValidationBuilder,
        agent_dependencies: &[String],
        tool_dependencies: &[String],
    ) -> Vec<ComponentDependency> {
        let agents = agent_dependencies
            .iter()
            .map(|dependency| ComponentDependency::Agent(AgentTypeName(dependency.clone())));
        let tools = tool_dependencies.iter().map(|dependency| {
            ComponentDependency::Tool(match ToolName::try_from(dependency.as_str()) {
                Ok(tool_name) => tool_name,
                Err(err) => {
                    validation.add_error(format!(
                        "Invalid tool dependency name: {}. {}",
                        dependency.log_color_error_highlight(),
                        err
                    ));
                    ToolName(dependency.clone())
                }
            })
        });

        agents.chain(tools).collect()
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
pub struct CanonicalFilePathWithPermissions {
    pub path: CanonicalFilePath,
    pub permissions: AgentFilePermissions,
}

impl CanonicalFilePathWithPermissions {
    pub fn extend_path(&mut self, path: &str) -> Result<(), String> {
        self.path.extend(path)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialComponentFile {
    pub source: InitialComponentFileSource,
    pub target: CanonicalFilePathWithPermissions,
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
            target: CanonicalFilePathWithPermissions {
                path: file.target_path,
                permissions: file.permissions.unwrap_or(AgentFilePermissions::ReadOnly),
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
            let relative_parent = relative_to.parent().expect("Failed to get parent");
            let absolute_relative_to =
                fs::absolute_lexical_path(relative_parent).map_err(|_| {
                    format!(
                        "Failed to resolve relative path: {}",
                        relative_to.log_color_highlight()
                    )
                })?;

            let source = fs::absolute_lexical_path_from_base_dir(
                Path::new(url_string),
                &absolute_relative_to,
            );
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
    use super::ResourceDefinitionCreation;
    use super::ResourceName;
    use crate::app::edit;
    use crate::fuzzy::FuzzySearch;
    use crate::log::LogColorize;
    use crate::model::app::{
        Application, ApplicationPreload, ComponentDependency, ComponentLayer,
        ComponentLayerApplyContext, ComponentLayerId, ComponentLayerProperties,
        ComponentLayerPropertiesKind, ComponentPresetName, ComponentPresetSelector,
        ComponentProperties, PartitionedComponentPresets, TEMP_DIR, WithSource,
    };
    use crate::model::app_raw;
    use crate::model::cascade::store::Store;
    use crate::model::http_api::{
        HttpApiDeploymentDeployProperties, McpDeploymentAgentOptions, McpDeploymentDeployProperties,
    };
    use crate::validation::{ValidatedResult, ValidationBuilder};
    use crate::{fs, fuzzy};
    use colored::Colorize;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::ComponentName;
    use golem_common::model::deployment::DeploymentRetryPolicyDefault;
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
        root_dir: PathBuf,
        application_name: WithSource<ApplicationName>,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        local_server: Option<WithSource<app_raw::LocalServer>>,
        component_presets: ComponentPresetSelector,
        apps: Vec<app_raw::ApplicationWithSource>,
    ) -> ValidatedResult<Application> {
        AppBuilder::build_app(
            root_dir,
            application_name,
            environments,
            local_server,
            component_presets,
            apps,
        )
    }

    // Load manifest fields needed before full application loading.
    pub fn build_application_preload(
        apps: &[app_raw::ApplicationWithSource],
    ) -> ValidatedResult<ApplicationPreload> {
        AppBuilder::build_application_preload(apps)
    }

    #[derive(Debug, PartialEq, Eq, Hash)]
    enum UniqueSourceCheckedEntityKey {
        App,
        Include,
        CustomCommand(String),
        Template(String),
        Component(ComponentName),
        Agent(AgentTypeName),
        Environment(EnvironmentName),
        SecretDefaults(EnvironmentName),
        RetryPolicyDefaults(EnvironmentName),
        ResourceDefaults(EnvironmentName),
        Bridge,
        LocalServer,
    }

    impl UniqueSourceCheckedEntityKey {
        fn entity_kind(&self) -> &'static str {
            let property = "Property";
            match self {
                UniqueSourceCheckedEntityKey::App => property,
                UniqueSourceCheckedEntityKey::Include => property,
                UniqueSourceCheckedEntityKey::CustomCommand(_) => "Custom command",
                UniqueSourceCheckedEntityKey::Template(_) => "Template",
                UniqueSourceCheckedEntityKey::Component(_) => "Component",
                UniqueSourceCheckedEntityKey::Agent(_) => "Agent",
                UniqueSourceCheckedEntityKey::Environment(_) => "Environment",
                UniqueSourceCheckedEntityKey::SecretDefaults(_) => property,
                UniqueSourceCheckedEntityKey::RetryPolicyDefaults(_) => property,
                UniqueSourceCheckedEntityKey::ResourceDefaults(_) => property,
                UniqueSourceCheckedEntityKey::Bridge => "Bridge",
                UniqueSourceCheckedEntityKey::LocalServer => property,
            }
        }

        fn entity_name(self) -> String {
            match self {
                UniqueSourceCheckedEntityKey::App => "app".log_color_highlight().to_string(),
                UniqueSourceCheckedEntityKey::Include => {
                    "include".log_color_highlight().to_string()
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
                UniqueSourceCheckedEntityKey::Agent(agent_name) => {
                    agent_name.0.log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::Environment(environment_name) => {
                    environment_name.0.log_color_highlight().to_string()
                }
                UniqueSourceCheckedEntityKey::SecretDefaults(environment_name) => {
                    format!(
                        "{}.{}",
                        "secretDefaults".log_color_highlight(),
                        environment_name.0.log_color_highlight()
                    )
                }
                UniqueSourceCheckedEntityKey::RetryPolicyDefaults(environment_name) => {
                    format!(
                        "{}.{}",
                        "retryPolicyDefaults".log_color_highlight(),
                        environment_name.0.log_color_highlight()
                    )
                }
                UniqueSourceCheckedEntityKey::ResourceDefaults(environment_name) => {
                    format!(
                        "{}.{}",
                        "resourceDefaults".log_color_highlight(),
                        environment_name.0.log_color_highlight()
                    )
                }
                UniqueSourceCheckedEntityKey::Bridge => "bridge".log_color_highlight().to_string(),
                UniqueSourceCheckedEntityKey::LocalServer => {
                    "localServer".log_color_highlight().to_string()
                }
            }
        }
    }

    fn resolve_http_api_domain(
        validation: &mut ValidationBuilder,
        deployment: &app_raw::HttpApiDeployment,
        environment_name: &EnvironmentName,
        environment: Option<&app_raw::Environment>,
        local_server: Option<&WithSource<app_raw::LocalServer>>,
        source: &Path,
    ) -> Option<Domain> {
        resolve_deployment_domain(
            validation,
            deployment.domain.as_ref(),
            deployment.subdomain.as_ref(),
            environment_name,
            environment,
            local_server.map(|local_server| &local_server.value),
            |local_server| local_server.and_then(|local_server| local_server.custom_request_port),
            crate::config::DEFAULT_LOCAL_CUSTOM_REQUEST_PORT,
            crate::config::CLOUD_HTTP_API_DOMAIN,
            "HTTP API",
            source,
        )
    }

    fn resolve_mcp_domain(
        validation: &mut ValidationBuilder,
        deployment: &app_raw::McpDeployment,
        environment_name: &EnvironmentName,
        environment: Option<&app_raw::Environment>,
        local_server: Option<&WithSource<app_raw::LocalServer>>,
        source: &Path,
    ) -> Option<Domain> {
        resolve_deployment_domain(
            validation,
            deployment.domain.as_ref(),
            deployment.subdomain.as_ref(),
            environment_name,
            environment,
            local_server.map(|local_server| &local_server.value),
            |local_server| local_server.and_then(|local_server| local_server.mcp_port),
            crate::config::DEFAULT_LOCAL_MCP_PORT,
            crate::config::CLOUD_MCP_DOMAIN,
            "MCP",
            source,
        )
    }

    fn resolve_deployment_domain(
        validation: &mut ValidationBuilder,
        domain: Option<&app_raw::DeploymentDomain>,
        subdomain: Option<&app_raw::DeploymentSubdomain>,
        environment_name: &EnvironmentName,
        environment: Option<&app_raw::Environment>,
        local_server: Option<&app_raw::LocalServer>,
        local_port: impl Fn(Option<&app_raw::LocalServer>) -> Option<u16>,
        default_local_port: u16,
        cloud_domain: &str,
        deployment_kind: &str,
        source: &Path,
    ) -> Option<Domain> {
        match (domain, subdomain) {
            (Some(domain), None) => {
                let raw = domain.as_str();
                Some(Domain(raw.to_string()))
            }
            (None, Some(subdomain)) => {
                let label = subdomain.as_str();
                if !is_valid_deployment_subdomain(label) {
                    validation.add_error(invalid_deployment_subdomain_error(label, source));
                    return None;
                }

                let Some(environment) = environment else {
                    validation.add_error(format!(
                        "Cannot resolve {} deployment subdomain {} in {} because environment {} is not defined.",
                        deployment_kind,
                        label.log_color_highlight(),
                        source.display().to_string().log_color_highlight(),
                        environment_name.0.log_color_highlight(),
                    ));
                    return None;
                };

                let resolved_domain = match environment.server.as_ref() {
                    Some(app_raw::Server::Custom(_)) => {
                        validation.add_error(format!(
                            "Cannot use {} deployment subdomain {} in {} for custom server environment {}. Use the {} field with a full domain instead.",
                            deployment_kind,
                            label.log_color_highlight(),
                            source.display().to_string().log_color_highlight(),
                            environment_name.0.log_color_highlight(),
                            "domain".log_color_highlight(),
                        ));
                        return None;
                    }
                    Some(app_raw::Server::Builtin(app_raw::BuiltinServer::Cloud)) => {
                        Domain(format!("{label}.{cloud_domain}"))
                    }
                    Some(app_raw::Server::Builtin(app_raw::BuiltinServer::Local)) | None => {
                        let local_port = local_port(local_server).unwrap_or(default_local_port);
                        Domain(format!("{label}.localhost:{}", local_port))
                    }
                };

                Some(resolved_domain)
            }
            (Some(_), Some(subdomain)) => {
                validation.add_error(format!(
                    "Deployment in {} cannot define both {} and {}. Use {} for a full domain, or {} for a built-in local/cloud server subdomain.",
                    source.display().to_string().log_color_highlight(),
                    "domain".log_color_highlight(),
                    "subdomain".log_color_highlight(),
                    "domain".log_color_highlight(),
                    "subdomain".log_color_highlight(),
                ));
                if !is_valid_deployment_subdomain(subdomain.as_str()) {
                    validation.add_error(invalid_deployment_subdomain_error(
                        subdomain.as_str(),
                        source,
                    ));
                }
                None
            }
            (None, None) => {
                validation.add_error(format!(
                    "Deployment in {} must define either {} or {}.",
                    source.display().to_string().log_color_highlight(),
                    "domain".log_color_highlight(),
                    "subdomain".log_color_highlight(),
                ));
                None
            }
        }
    }

    fn is_valid_deployment_subdomain(label: &str) -> bool {
        !label.is_empty()
            && label.len() <= 63
            && label
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
            && label
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            && label
                .chars()
                .last()
                .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    }

    fn invalid_deployment_subdomain_error(label: &str, source: &Path) -> String {
        format!(
            "Invalid deployment subdomain {} in {}. Use a single lowercase DNS label without dots, ports, or URL schemes.",
            label.log_color_highlight(),
            source.display().to_string().log_color_highlight(),
        )
    }

    #[derive(Default)]
    struct AppBuilder {
        // For environment build
        app: Option<WithSource<ApplicationName>>,
        default_environment_names: BTreeSet<EnvironmentName>,
        environments: IndexMap<EnvironmentName, app_raw::Environment>,
        local_server: Option<WithSource<app_raw::LocalServer>>,
        deployment_domain_local_server: Option<WithSource<app_raw::LocalServer>>,

        // "Consts" for component templating
        app_root_dir_str: String,
        golem_temp_dir_str: String,
        cargo_workspace_mode: bool,

        // For app build
        include: Vec<String>,
        custom_commands: HashMap<String, WithSource<Vec<app_raw::ExternalCommand>>>,
        clean: Vec<WithSource<String>>,

        raw_component_names: HashSet<String>,
        component_names_to_source_and_dir: BTreeMap<ComponentName, (PathBuf, Option<PathBuf>)>,
        component_custom_presets: BTreeSet<ComponentPresetName>,
        component_layer_store: Store<ComponentLayer>,

        components:
            BTreeMap<ComponentName, WithSource<(ComponentProperties, ComponentLayerProperties)>>,
        agents: BTreeMap<AgentTypeName, WithSource<app_raw::Agent>>,

        http_api_deployments: BTreeMap<
            EnvironmentName,
            BTreeMap<Domain, WithSource<HttpApiDeploymentDeployProperties>>,
        >,

        mcp_deployments:
            BTreeMap<EnvironmentName, BTreeMap<Domain, WithSource<McpDeploymentDeployProperties>>>,

        bridge_sdks: WithSource<app_raw::BridgeSdks>,

        agent_secret_defaults: BTreeMap<EnvironmentName, WithSource<app_raw::JsonObject>>,

        retry_policy_defaults:
            BTreeMap<EnvironmentName, BTreeMap<String, WithSource<DeploymentRetryPolicyDefault>>>,

        resource_definition_defaults: BTreeMap<
            EnvironmentName,
            BTreeMap<ResourceName, WithSource<ResourceDefinitionCreation>>,
        >,

        all_sources: BTreeSet<PathBuf>,
        entity_sources: HashMap<UniqueSourceCheckedEntityKey, Vec<PathBuf>>,
    }

    impl AppBuilder {
        // NOTE: build_app receives preloaded application fields, so flows that do not otherwise
        //       need the full manifest are not blocked by high-level validation errors,
        //       and we do not "steal" manifest loading logs from those which do use the manifest fully.
        fn build_app(
            app_root_dir: PathBuf,
            application_name: WithSource<ApplicationName>,
            environments: BTreeMap<EnvironmentName, app_raw::Environment>,
            local_server: Option<WithSource<app_raw::LocalServer>>,
            component_presets: ComponentPresetSelector,
            apps: Vec<app_raw::ApplicationWithSource>,
        ) -> ValidatedResult<Application> {
            let mut validation = ValidationBuilder::default();
            let mut builder = Self {
                environments: environments.clone().into_iter().collect(),
                deployment_domain_local_server: local_server,
                ..Self::default()
            };

            match Ok::<&PathBuf, anyhow::Error>(&app_root_dir).and_then(|app_root_dir| {
                Ok((
                    fs::path_to_str(app_root_dir).map(|path| path.to_string())?,
                    fs::path_to_str(&app_root_dir.join(TEMP_DIR)).map(|path| path.to_string())?,
                    edit::cargo_toml::is_workspace_manifest(
                        &fs::read_to_string(app_root_dir.join("Cargo.toml")).unwrap_or_default(),
                    )
                    .unwrap_or(false),
                ))
            }) {
                Ok((app_root_dir_str, golem_temp_dir_str, cargo_workspace_mode)) => {
                    builder.app_root_dir_str = app_root_dir_str;
                    builder.golem_temp_dir_str = golem_temp_dir_str;
                    builder.cargo_workspace_mode = cargo_workspace_mode;
                }
                Err(err) => {
                    return ValidatedResult::from_error(format!(
                        "Failed to get app root directory: {}",
                        err
                    ));
                }
            }

            for app in apps {
                builder.add_raw_app(&mut validation, app);
            }

            // TODO: atomic: validate presets used in envs and template references
            //               before component resolve, and skip if they are not valid
            builder.resolve_and_validate_components(&mut validation, &component_presets);
            builder.validate_unique_sources(&mut validation);
            builder.validate_http_api_deployments(&mut validation, &environments);

            validation.build(Application {
                app_root_dir,
                app_root_dir_str: builder.app_root_dir_str,
                golem_temp_dir_str: builder.golem_temp_dir_str,
                cargo_workspace_mode: builder.cargo_workspace_mode,
                environments,
                component_preset_selector: component_presets,
                application_name,
                all_sources: builder.all_sources,
                components: builder.components,
                agents: builder.agents,
                component_layer_store: builder.component_layer_store,
                custom_commands: builder.custom_commands,
                clean: builder.clean,
                http_api_deployments: builder.http_api_deployments,
                mcp_deployments: builder.mcp_deployments,
                agent_secrets_defaults: builder.agent_secret_defaults,
                retry_policy_defaults: builder.retry_policy_defaults,
                resource_definition_defaults: builder.resource_definition_defaults,
                bridge_sdks: builder.bridge_sdks,
            })
        }

        // NOTE: Unlike build_app, here we do not consume the source apps, so they can be
        //       used for build_app. For more info on this separation, see build_app.
        fn build_application_preload(
            apps: &[app_raw::ApplicationWithSource],
        ) -> ValidatedResult<ApplicationPreload> {
            let mut builder = Self::default();
            let mut validation = ValidationBuilder::default();

            for app in apps {
                builder.add_raw_app_preload_only(&mut validation, app);
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

            validation.build(ApplicationPreload {
                application_name,
                environments: builder.environments.into_iter().collect(),
                local_server: builder.local_server,
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

                    if !app.application.includes.is_empty()
                        && self
                        .add_entity_source(UniqueSourceCheckedEntityKey::Include, &app.source)
                    {
                        self.include = app.application.includes;
                    }

                    for (template_name, template) in app.application.component_templates {
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
                            self.component_names_to_source_and_dir
                                .insert(component_name.clone(), (app.source.clone(), component.dir.as_ref().map(PathBuf::from)));
                            self.add_component(validation, component_name, component);
                        }
                    }

                    for (agent_type_name, agent_properties) in app.application.agents {
                        // TODO: atl: resolve and store effective agent properties here using
                        // agent templates/presets and flattened component fallback layers.
                        let unique_key = UniqueSourceCheckedEntityKey::Agent(agent_type_name.clone());
                        if self.add_entity_source(unique_key, &app.source) {
                            self.agents.insert(
                                agent_type_name,
                                WithSource::new(app.source.clone(), agent_properties),
                            );
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
                                let Some(domain) = resolve_http_api_domain(
                                    validation,
                                    &api_deployment,
                                    &environment,
                                    self.environments.get(&environment),
                                    self.deployment_domain_local_server
                                        .as_ref()
                                        .or(self.local_server.as_ref()),
                                    &app.source,
                                ) else {
                                    continue;
                                };

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

                                deployments.entry(domain).or_insert(WithSource::new(
                                    app.source.to_path_buf(),
                                    HttpApiDeploymentDeployProperties {
                                        webhooks_prefix: HttpApiDeploymentCreation::normalize_webhooks_prefix(
                                            api_deployment
                                                .webhook_url
                                                .unwrap_or_else(HttpApiDeploymentCreation::default_webhooks_prefix),
                                        ),
                                        openapi_prefix: HttpApiDeploymentCreation::normalize_openapi_endpoint_prefix(api_deployment.openapi_endpoint.unwrap_or(HttpApiDeploymentCreation::default_openapi_endpoint_prefix())),
                                        agents,
                                    },
                                ));
                            }
                        }
                    }

                    if let Some(mcp) = app.application.mcp {
                        for (environment, deployments) in mcp.deployments {
                            for mcp_deployment in deployments {
                                let Some(domain) = resolve_mcp_domain(
                                    validation,
                                    &mcp_deployment,
                                    &environment,
                                    self.environments.get(&environment),
                                    self.deployment_domain_local_server
                                        .as_ref()
                                        .or(self.local_server.as_ref()),
                                    &app.source,
                                ) else {
                                    continue;
                                };

                                let mcp_deployments =
                                    self.mcp_deployments.entry(environment.clone()).or_default();

                                let agents = mcp_deployment.agents
                                    .into_iter()
                                    .map(|(k, v)| (k, McpDeploymentAgentOptions {
                                        security_scheme: v.security_scheme,
                                    }))
                                    .collect();

                                mcp_deployments.entry(domain).or_insert(WithSource::new(
                                    app.source.to_path_buf(),
                                    McpDeploymentDeployProperties { agents },
                                ));
                            }
                        }
                    }

                    for (environment, environment_secret_defaults) in app.application.secret_defaults {
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::SecretDefaults(environment.clone()),
                            &app.source,
                        ) {
                            self.agent_secret_defaults.insert(
                                environment.clone(),
                                WithSource::new(app.source.to_path_buf(), environment_secret_defaults),
                            );
                        }
                    }

                    for (environment, env_retry_policy_defaults) in app.application.retry_policy_defaults {
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::RetryPolicyDefaults(environment.clone()),
                            &app.source,
                        ) {
                            for (retry_policy_name, retry_policy) in env_retry_policy_defaults {
                                let entry = self
                                    .retry_policy_defaults
                                    .entry(environment.clone())
                                    .or_default();
                                entry.insert(
                                    retry_policy_name.clone(),
                                    WithSource::new(
                                        app.source.to_path_buf(),
                                        DeploymentRetryPolicyDefault {
                                            name: retry_policy_name,
                                            priority: retry_policy.priority,
                                            predicate: retry_policy.predicate.into(),
                                            policy: retry_policy.policy.into(),
                                        },
                                    ),
                                );
                            }
                        }
                    }

                    for (environment, resource_defs) in app.application.resource_defaults {
                        if self.add_entity_source(
                            UniqueSourceCheckedEntityKey::ResourceDefaults(environment.clone()),
                            &app.source,
                        ) {
                            for (resource_name, resource) in resource_defs {
                                let entry = self
                                    .resource_definition_defaults
                                    .entry(environment.clone())
                                    .or_default();
                                entry.insert(
                                    resource_name.clone(),
                                    WithSource::new(
                                        app.source.to_path_buf(),
                                        ResourceDefinitionCreation {
                                            name: resource_name,
                                            limit: resource.limit,
                                            enforcement_action: resource.enforcement_action,
                                            unit: resource.unit,
                                            units: resource.units,
                                        },
                                    ),
                                );
                            }
                        }
                    }

                    if let Some(bridge) = app.application.bridge
                        && self
                            .add_entity_source(UniqueSourceCheckedEntityKey::Bridge, app_source_dir)
                        {
                            self.bridge_sdks =
                                WithSource::new(app_source_dir.to_path_buf(), bridge);

                            for (target_language, bridge_mode, sdk_targets) in
                                self.bridge_sdks.value.for_all_used_modes()
                            {
                                let sdk_targets = sdk_targets.agents.clone().into_vec();
                                let non_unique_targets = sdk_targets.iter()
                                    .counts()
                                    .into_iter()
                                    .filter(|(_, count)| *count > 1)
                                    .collect::<Vec<_>>();

                                validation.with_context(
                                    vec![
                                        ("bridge SDK language", target_language.to_string()),
                                        ("bridge SDK mode", bridge_mode.to_string()),
                                    ],
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

                            for (target_language, sdk_targets) in
                                self.bridge_sdks.value.for_all_languages()
                            {
                                if target_language != crate::model::GuestLanguage::Rust
                                    && sdk_targets.is_some_and(|targets| targets.guest.is_some())
                                {
                                    validation.with_context(
                                        vec![("bridge SDK language", target_language.to_string())],
                                        |validation| {
                                            validation.add_error(format!(
                                                "guest bridge mode is not supported for {} yet",
                                                target_language.to_string().log_color_error_highlight()
                                            ));
                                        },
                                    );
                                }
                            }
                        }
                });
        }

        fn add_raw_app_preload_only(
            &mut self,
            validation: &mut ValidationBuilder,
            app: &app_raw::ApplicationWithSource,
        ) {
            validation.with_context(
                vec![("source", app.source.to_string_lossy().to_string())],
                |validation| {
                    if let Some(app_name) = &app.application.app
                        && self.add_entity_source(UniqueSourceCheckedEntityKey::App, &app.source)
                    {
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

                    if let Some(local_server) = &app.application.local_server {
                        match &self.local_server {
                            Some(existing) => validation.add_error(format!(
                                "{} {} is defined in multiple sources: {}, {}",
                                UniqueSourceCheckedEntityKey::LocalServer.entity_kind(),
                                UniqueSourceCheckedEntityKey::LocalServer.entity_name(),
                                existing.source.log_color_highlight(),
                                app.source.log_color_highlight()
                            )),
                            None => {
                                Self::validate_local_server_ports(
                                    validation,
                                    local_server,
                                    &app.source,
                                );
                                self.local_server =
                                    Some(WithSource::new(app.source.clone(), local_server.clone()));
                            }
                        }
                    }
                },
            );
        }

        fn validate_local_server_ports(
            validation: &mut ValidationBuilder,
            local_server: &app_raw::LocalServer,
            source: &Path,
        ) {
            fn validate_port(
                validation: &mut ValidationBuilder,
                value: Option<u16>,
                manifest_field_name: &str,
                cli_flag_name: &str,
                source: &Path,
            ) {
                if value == Some(0) {
                    validation.add_error(format!(
                        "{} in {} must be nonzero. Port 0 is only allowed when passed directly as {} to {}.",
                        manifest_field_name.log_color_highlight(),
                        source.display().to_string().log_color_highlight(),
                        format!("{cli_flag_name} 0").log_color_highlight(),
                        "golem server run".log_color_highlight(),
                    ));
                }
            }

            validate_port(
                validation,
                local_server.router_port,
                "localServer.routerPort",
                "--router-port",
                source,
            );
            validate_port(
                validation,
                local_server.custom_request_port,
                "localServer.customRequestPort",
                "--custom-request-port",
                source,
            );
            validate_port(
                validation,
                local_server.mcp_port,
                "localServer.mcpPort",
                "--mcp-port",
                source,
            );
        }

        fn add_component_template(
            &mut self,
            validation: &mut ValidationBuilder,
            template_name: String,
            template: app_raw::ComponentTemplate,
        ) {
            validation.with_context(
                vec![("template", template_name.to_string())],
                |validation| {
                    if let Some(err) = self
                        .component_layer_store
                        .add_layer(ComponentLayer {
                            id: ComponentLayerId::TemplateCommon(template_name.clone()),
                            parents: ComponentLayerId::parent_ids_from_raw_template_references(
                                template.templates.clone(),
                            ),
                            properties: ComponentLayerPropertiesKind::Common(Box::new(
                                template.component_layer_properties().into(),
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
                },
            );
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
                                component.templates.clone(),
                            ),
                            properties: ComponentLayerPropertiesKind::Common(Box::new(
                                component.component_layer_properties().into(),
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

        fn resolve_and_validate_components(
            &mut self,
            validation: &mut ValidationBuilder,
            component_presets: &ComponentPresetSelector,
        ) {
            for (component_name, (source, dir)) in self.component_names_to_source_and_dir.clone() {
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
                            dir,
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
            dir: Option<PathBuf>,
            component_name: ComponentName,
        ) {
            let component_dir = match fs::parent_or_err(&source)
                .and_then(fs::absolute_lexical_path)
                .map(|path| {
                    let path = dir.as_ref().map(|dir| path.join(dir)).unwrap_or(path);
                    fs::normalize_path_lexically(&path)
                }) {
                Ok(path) => path,
                Err(err) => {
                    validation.add_error(err.to_string());
                    return;
                }
            };

            let component_dir_str =
                match fs::path_to_str(&component_dir).map(|path| path.to_string()) {
                    Ok(path) => path,
                    Err(err) => {
                        validation.add_error(err.to_string());
                        return;
                    }
                };

            let ctx = ComponentLayerApplyContext::new(
                Some(component_name.clone()),
                Some(self.app_root_dir_str.clone()),
                Some(self.golem_temp_dir_str.clone()),
                Some(component_dir_str.clone()),
                Some(if self.cargo_workspace_mode {
                    self.app_root_dir_str.clone()
                } else {
                    component_dir_str.clone()
                }),
            );

            match self.component_layer_store.value(
                &ComponentLayerId::ComponentCustomPresets(component_name.clone()),
                component_presets,
                &ctx,
            ) {
                Ok(component_layer_properties) => {
                    let component_properties = ComponentProperties::from_merged(
                        validation,
                        &source,
                        dir,
                        component_dir,
                        &component_layer_properties,
                    );
                    self.validate_component_dependencies(
                        validation,
                        &component_name,
                        &component_properties.dependencies,
                    );
                    self.components.insert(
                        component_name,
                        WithSource::new(source, (component_properties, component_layer_properties)),
                    );
                }
                Err(err) => validation.add_error(format!("Failed to resolve component: {err}")),
            }
        }

        fn validate_component_dependencies(
            &self,
            _validation: &mut ValidationBuilder,
            _component_name: &ComponentName,
            _dependencies: &[ComponentDependency],
        ) {
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
    use crate::bridge_gen::{
        BridgeMode, bridge_client_directory_name, bridge_client_directory_name_for_mode,
    };
    use crate::fs;
    use crate::model::app::{
        Application, ApplicationPreload, ComponentDependency, ComponentPresetSelector, ToolName,
        includes_from_yaml_file,
    };
    use crate::model::app_raw;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::component::ComponentName;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::EnvironmentName;
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;
    use tempfile::TempDir;
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
                componentWasm: dummy-component.wasm

            components:
              app:main:
                templates: malbogle
                presets:
                  a:
                    componentWasm: a.wasm
                  b:
                    componentWasm: b.wasm

        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        let component_name = parse_component_name("app:main");
        let component = app.component(&component_name);

        assert_eq!(component.wasm(), app_tmp_dir.path().join("a.wasm"));
    }

    #[test]
    fn test_component_resolution_order_is_common_env_custom() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              app:main:
                componentWasm: common.wasm
                env:
                  KEY: common
                  COMMON_ONLY: common
                presets:
                  app-env:local:
                    componentWasm: env.wasm
                    env:
                      KEY: env
                      ENV_ONLY: env
                  debug:
                    componentWasm: custom.wasm
                    env:
                      KEY: custom
                      CUSTOM_ONLY: custom
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        let component_name = parse_component_name("app:main");
        let component = app.component(&component_name);

        assert_eq!(component.wasm(), app_tmp_dir.path().join("custom.wasm"));
        assert_eq!(
            component.env().get("KEY").cloned(),
            Some("custom".to_string())
        );
        assert_eq!(
            component.env().get("COMMON_ONLY").cloned(),
            Some("common".to_string())
        );
        assert_eq!(
            component.env().get("ENV_ONLY").cloned(),
            Some("env".to_string())
        );
        assert_eq!(
            component.env().get("CUSTOM_ONLY").cloned(),
            Some("custom".to_string())
        );
        assert_eq!(
            component_applied_layers_trace(&component),
            vec![
                "app:main".to_string(),
                "app:main[app-env:local]".to_string(),
                "app:main[debug]".to_string(),
            ]
        );
    }

    #[test]
    fn test_component_environment_preset_selected_by_environment_name() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
              cloud:
                server: cloud

            components:
              app:main:
                componentWasm: common.wasm
                env:
                  COMMON_ONLY: common
                presets:
                  app-env:local:
                    componentWasm: local.wasm
                    env:
                      ENV_ONLY: local
                  app-env:cloud:
                    componentWasm: cloud.wasm
                    env:
                      ENV_ONLY: cloud
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "cloud", &[]);

        let component_name = parse_component_name("app:main");
        let component = app.component(&component_name);

        assert_eq!(component.wasm(), app_tmp_dir.path().join("cloud.wasm"));
        assert_eq!(
            component.env().get("COMMON_ONLY").cloned(),
            Some("common".to_string())
        );
        assert_eq!(
            component.env().get("ENV_ONLY").cloned(),
            Some("cloud".to_string())
        );
        assert_eq!(
            component_applied_layers_trace(&component),
            vec![
                "app:main".to_string(),
                "app:main[app-env:cloud]".to_string(),
            ]
        );
    }

    #[test]
    fn test_component_custom_template_single_is_applied() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug

            componentTemplates:
              custom-template:
                componentWasm: template-common.wasm
                env:
                  KEY: template-common
                  TEMPLATE_COMMON_ONLY: template-common
                presets:
                  app-env:local:
                    componentWasm: template-env.wasm
                    env:
                      KEY: template-env
                      TEMPLATE_ENV_ONLY: template-env
                  debug:
                    componentWasm: template-custom.wasm
                    env:
                      KEY: template-custom
                      TEMPLATE_CUSTOM_ONLY: template-custom

            components:
              app:main:
                templates: custom-template
                env:
                  COMPONENT_ONLY: component
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        let component_name = parse_component_name("app:main");
        let component = app.component(&component_name);

        assert_eq!(
            component_applied_layers_trace(&component),
            vec![
                "custom-template".to_string(),
                "custom-template[app-env:local]".to_string(),
                "custom-template[debug]".to_string(),
                "app:main".to_string(),
            ]
        );
        assert_eq!(
            component.wasm(),
            app_tmp_dir.path().join("template-custom.wasm")
        );
        assert_eq!(
            component.env().get("KEY").cloned(),
            Some("template-custom".to_string())
        );
        assert_eq!(
            component.env().get("TEMPLATE_COMMON_ONLY").cloned(),
            Some("template-common".to_string())
        );
        assert_eq!(
            component.env().get("TEMPLATE_ENV_ONLY").cloned(),
            Some("template-env".to_string())
        );
        assert_eq!(
            component.env().get("TEMPLATE_CUSTOM_ONLY").cloned(),
            Some("template-custom".to_string())
        );
        assert_eq!(
            component.env().get("COMPONENT_ONLY").cloned(),
            Some("component".to_string())
        );
    }

    #[test]
    fn test_component_custom_template_multiple_are_applied_in_order() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug

            componentTemplates:
              template-a:
                componentWasm: template-a-common.wasm
                env:
                  KEY: template-a-common
                presets:
                  app-env:local:
                    componentWasm: template-a-env.wasm
                    env:
                      KEY: template-a-env
                  debug:
                    componentWasm: template-a-custom.wasm
                    env:
                      KEY: template-a-custom
                      A_ONLY: template-a-custom
              template-b:
                componentWasm: template-b-common.wasm
                env:
                  KEY: template-b-common
                presets:
                  app-env:local:
                    componentWasm: template-b-env.wasm
                    env:
                      KEY: template-b-env
                  debug:
                    componentWasm: template-b-custom.wasm
                    env:
                      KEY: template-b-custom
                      B_ONLY: template-b-custom

            components:
              app:main:
                templates: [template-a, template-b]
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        let component_name = parse_component_name("app:main");
        let component = app.component(&component_name);

        assert_eq!(
            component.wasm(),
            app_tmp_dir.path().join("template-b-custom.wasm")
        );
        assert_eq!(
            component.env().get("KEY").cloned(),
            Some("template-b-custom".to_string())
        );
        assert_eq!(
            component.env().get("A_ONLY").cloned(),
            Some("template-a-custom".to_string())
        );
        assert_eq!(
            component.env().get("B_ONLY").cloned(),
            Some("template-b-custom".to_string())
        );
        assert_eq!(
            component_applied_layers_trace(&component),
            vec![
                "template-a".to_string(),
                "template-a[app-env:local]".to_string(),
                "template-a[debug]".to_string(),
                "template-b".to_string(),
                "template-b[app-env:local]".to_string(),
                "template-b[debug]".to_string(),
                "app:main".to_string(),
            ]
        );
    }

    #[test]
    fn test_root_level_agents_are_accepted() {
        let source = indoc! { r#"
            app: hello-app

            agents:
              test-agent:
                config:
                  a: 1

            components:
              app:main:
                componentWasm: dummy-component.wasm
        "# };

        let result = app_raw::Application::from_yaml_str(source);
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn bridge_sdk_output_dir_is_base_for_per_agent_external_targets() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            bridge:
              rust:
                agents:
                  - AlphaAgent
                  - BetaAgent
                outputDir: bridge-sdk/rust
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &[]);
        let alpha_agent = parse_agent_type_name("AlphaAgent");
        let beta_agent = parse_agent_type_name("BetaAgent");

        assert_eq!(
            app.bridge_sdk_dir(
                &alpha_agent,
                crate::model::GuestLanguage::Rust,
                BridgeMode::External
            ),
            app_tmp_dir
                .path()
                .join("bridge-sdk/rust")
                .join(bridge_client_directory_name(&alpha_agent))
        );
        assert_eq!(
            app.bridge_sdk_dir(
                &beta_agent,
                crate::model::GuestLanguage::Rust,
                BridgeMode::External
            ),
            app_tmp_dir
                .path()
                .join("bridge-sdk/rust")
                .join(bridge_client_directory_name(&beta_agent))
        );
    }

    #[test]
    fn bridge_sdk_output_dir_is_mode_separated_for_guest_targets() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            bridge:
              rust:
                agents:
                  - AlphaAgent
                guest:
                  agents:
                    - AlphaAgent
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);
        let alpha_agent = parse_agent_type_name("AlphaAgent");

        assert_eq!(
            app.bridge_sdk_dir(
                &alpha_agent,
                crate::model::GuestLanguage::Rust,
                BridgeMode::External
            ),
            app.temp_dir()
                .join("bridge-sdk")
                .join("rust")
                .join(bridge_client_directory_name(&alpha_agent))
        );
        assert_eq!(
            app.bridge_sdk_dir(
                &alpha_agent,
                crate::model::GuestLanguage::Rust,
                BridgeMode::Guest
            ),
            app.temp_dir()
                .join("bridge-sdk")
                .join("rust")
                .join("guest")
                .join(bridge_client_directory_name_for_mode(
                    &alpha_agent,
                    BridgeMode::Guest
                ))
        );
    }

    #[test]
    fn bridge_sdk_output_dir_uses_guest_output_dir_as_per_agent_base() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            bridge:
              rust:
                guest:
                  agents:
                    - AlphaAgent
                  outputDir: bridge-sdk/rust-guest
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &[]);
        let alpha_agent = parse_agent_type_name("AlphaAgent");

        assert_eq!(
            app.bridge_sdk_dir(
                &alpha_agent,
                crate::model::GuestLanguage::Rust,
                BridgeMode::Guest
            ),
            app_tmp_dir.path().join("bridge-sdk/rust-guest").join(
                bridge_client_directory_name_for_mode(&alpha_agent, BridgeMode::Guest)
            )
        );
    }

    #[test]
    fn bridge_sdk_guest_tools_are_accepted_and_use_tool_bridge_dir() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            bridge:
              rust:
                guest:
                  tools:
                    - MyTool
                  outputDir: bridge-sdk/rust-guest
        "# };

        let (app, app_tmp_dir) = load_app_for_env(source, "local", &[]);

        assert_eq!(
            app.tool_bridge_sdk_dir("MyTool", crate::model::GuestLanguage::Rust),
            app_tmp_dir.path().join("bridge-sdk/rust-guest").join(
                crate::bridge_gen::tool_bridge_client_directory_name("MyTool")
            )
        );

        let used_modes = app.bridge_sdks().for_all_used_modes();
        assert_eq!(used_modes.len(), 1);
        assert_eq!(used_modes[0].0, crate::model::GuestLanguage::Rust);
        assert_eq!(used_modes[0].1, BridgeMode::Guest);
    }

    #[test]
    fn component_dependencies_are_agent_and_tool_guest_bridge_targets() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm
                dependencies:
                  agents:
                    - ShoppingCart
                  tools:
                    - grep
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);
        let component_name = parse_component_name("app:main");
        let component = app.component(&component_name);
        let dependencies = &component.properties().dependencies;

        assert_eq!(
            dependencies,
            &vec![
                ComponentDependency::Agent(parse_agent_type_name("ShoppingCart")),
                ComponentDependency::Tool(ToolName::try_from("grep").unwrap()),
            ]
        );
    }

    #[test]
    fn non_rust_guest_bridge_mode_is_rejected() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            bridge:
              ts:
                guest:
                  agents: SomeAgent
        "# };

        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();
        let raw_apps =
            vec![app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path).unwrap()];
        let (preload, warns, errors) = Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = preload
        else {
            panic!("expected Some(ApplicationPreload)")
        };

        let (_app, _warns, errors) = Application::from_raw_apps(
            std::env::current_dir().unwrap(),
            application_name,
            environments,
            local_server,
            selector("local", &[]),
            raw_apps,
        )
        .into_product();

        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("guest bridge mode is not supported for TypeScript yet"),
            "unexpected error: {}",
            errors[0]
        );
    }

    #[test]
    fn non_rust_guest_bridge_config_without_agents_is_rejected() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            bridge:
              ts:
                agents: ExternalAgent
                outputDir: bridge/ts
                guest:
                  outputDir: bridge/ts-guest
        "# };

        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();
        let raw_apps =
            vec![app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path).unwrap()];
        let (preload, warns, errors) = Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = preload
        else {
            panic!("expected Some(ApplicationPreload)")
        };

        let (_app, warns, errors) = Application::from_raw_apps(
            std::env::current_dir().unwrap(),
            application_name,
            environments,
            local_server,
            selector("local", &[]),
            raw_apps,
        )
        .into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("guest bridge mode is not supported for TypeScript yet"),
            "unexpected error: {}",
            errors[0]
        );
    }

    #[test]
    fn test_agent_resolution_order() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm
                config:
                  fallback: "fallback"
                  nested:
                    from_component: true
                    deep:
                      keep: "component"
                  replacedByScalar:
                    should: "be replaced"
                env:
                  KEY: fallback
                  ONLY_FALLBACK: fb

            agents:
              test-agent:
                presets:
                  app-env:local:
                    config:
                      env: "env"
                      nested:
                        from_env: true
                      replacedByScalar:
                        still: "object"
                    env:
                      KEY: env
                  custom:
                    config:
                      custom: "custom"
                      nested:
                        deep:
                          keep: "custom"
                          plus: "custom"
                      replacedByScalar: "scalar"
                config:
                  common: "common"
                  nested:
                    from_common: true
                    deep:
                      keep: "common"
                env:
                  KEY: common
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &["custom"]);

        with_resolved_agent(&app, "app:main", "test-agent", |agent| {
            assert_eq!(
                agent.config().cloned(),
                Some(json!({
                    "fallback": "fallback",
                    "env": "env",
                    "custom": "custom",
                    "common": "common",
                    "nested": {
                        "from_component": true,
                        "from_env": true,
                        "from_common": true,
                        "deep": {
                            "keep": "custom",
                            "plus": "custom"
                        }
                    },
                    "replacedByScalar": "scalar"
                }))
            );

            assert_eq!(agent.env().get("KEY").cloned(), Some("env".to_string()));
            assert_eq!(
                agent.env().get("ONLY_FALLBACK").cloned(),
                Some("fb".to_string())
            );

            assert_eq!(agent.applied_layers().len(), 4);
        });
    }

    #[test]
    fn test_agent_resolution_order_is_common_env_custom() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              app:main:
                componentWasm: dummy-component.wasm
                config:
                  level: component
                env:
                  KEY: component

            agents:
              test-agent:
                config:
                  level: common
                  commonOnly: true
                env:
                  KEY: common
                  COMMON_ONLY: common
                presets:
                  app-env:local:
                    config:
                      level: env
                      envOnly: true
                    env:
                      KEY: env
                      ENV_ONLY: env
                  debug:
                    config:
                      level: custom
                      customOnly: true
                    env:
                      KEY: custom
                      CUSTOM_ONLY: custom
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        with_resolved_agent(&app, "app:main", "test-agent", |agent| {
            assert_eq!(
                agent.config().cloned(),
                Some(json!({
                    "level": "custom",
                    "commonOnly": true,
                    "envOnly": true,
                    "customOnly": true
                }))
            );
            assert_eq!(agent.env().get("KEY").cloned(), Some("custom".to_string()));
            assert_eq!(
                agent.env().get("COMMON_ONLY").cloned(),
                Some("common".to_string())
            );
            assert_eq!(
                agent.env().get("ENV_ONLY").cloned(),
                Some("env".to_string())
            );
            assert_eq!(
                agent.env().get("CUSTOM_ONLY").cloned(),
                Some("custom".to_string())
            );
            assert_eq!(
                agent_applied_layers_trace(agent),
                vec![
                    "component:app:main".to_string(),
                    "agent:test-agent:common".to_string(),
                    "agent:test-agent:environment-presets[app-env:local]".to_string(),
                    "agent:test-agent:custom-presets[debug]".to_string(),
                ]
            );
        });
    }

    #[test]
    fn test_agent_custom_preset_falls_back_to_default() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            agents:
              test-agent:
                config:
                  source: common
                presets:
                  default-custom:
                    default: true
                    config:
                      selected: default-custom
                  another-custom:
                    config:
                      selected: another-custom
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &["missing"]);

        with_resolved_agent(&app, "app:main", "test-agent", |agent| {
            assert_eq!(
                agent.config().cloned(),
                Some(json!({
                    "source": "common",
                    "selected": "default-custom"
                }))
            );
            assert_eq!(
                agent_applied_layers_trace(agent),
                vec![
                    "component:app:main".to_string(),
                    "agent:test-agent:common".to_string(),
                    "agent:test-agent:custom-presets[default-custom]".to_string(),
                ]
            );
        });
    }

    #[test]
    fn test_agent_environment_preset_selected_by_environment_name() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
              cloud:
                server: cloud

            components:
              app:main:
                componentWasm: dummy-component.wasm

            agents:
              test-agent:
                config:
                  source: common
                presets:
                  app-env:local:
                    config:
                      envPreset: local
                  app-env:cloud:
                    config:
                      envPreset: cloud
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "cloud", &[]);

        with_resolved_agent(&app, "app:main", "test-agent", |agent| {
            assert_eq!(
                agent.config().cloned(),
                Some(json!({
                    "source": "common",
                    "envPreset": "cloud"
                }))
            );
            assert_eq!(
                agent_applied_layers_trace(agent),
                vec![
                    "component:app:main".to_string(),
                    "agent:test-agent:common".to_string(),
                    "agent:test-agent:environment-presets[app-env:cloud]".to_string(),
                ]
            );
        });
    }

    #[test]
    fn test_agent_custom_template_single_is_applied() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug

            componentTemplates:
              template-for-agent:
                config:
                  source: template-common
                  templateCommonOnly: true
                presets:
                  app-env:local:
                    config:
                      source: template-env
                      templateEnvOnly: true
                  debug:
                    config:
                      source: template-custom
                      templateCustomOnly: true

            components:
              app:main:
                componentWasm: dummy-component.wasm

            agents:
              test-agent:
                templates: template-for-agent
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        with_resolved_agent(&app, "app:main", "test-agent", |agent| {
            assert_eq!(
                agent.config().cloned(),
                Some(json!({
                    "source": "template-custom",
                    "templateCommonOnly": true,
                    "templateEnvOnly": true,
                    "templateCustomOnly": true
                }))
            );
            assert_eq!(
                agent_applied_layers_trace(agent),
                vec![
                    "component:app:main".to_string(),
                    "agent:test-agent:template:template-for-agent".to_string(),
                    "agent:test-agent:common".to_string(),
                ]
            );
        });
    }

    #[test]
    fn test_agent_custom_template_multiple_are_applied_in_order() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local
                componentPresets: debug

            componentTemplates:
              template-a:
                config:
                  from: template-a-common
                  fromACommon: true
                presets:
                  app-env:local:
                    config:
                      from: template-a-env
                      fromAEnv: true
                  debug:
                    config:
                      from: template-a-custom
                      fromACustom: true
              template-b:
                config:
                  from: template-b-common
                  fromBCommon: true
                presets:
                  app-env:local:
                    config:
                      from: template-b-env
                      fromBEnv: true
                  debug:
                    config:
                      from: template-b-custom
                      fromBCustom: true

            components:
              app:main:
                componentWasm: dummy-component.wasm

            agents:
              test-agent:
                templates: [template-a, template-b]
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &["debug"]);

        with_resolved_agent(&app, "app:main", "test-agent", |agent| {
            assert_eq!(
                agent.config().cloned(),
                Some(json!({
                    "from": "template-b-custom",
                    "fromACommon": true,
                    "fromAEnv": true,
                    "fromACustom": true,
                    "fromBCommon": true,
                    "fromBEnv": true,
                    "fromBCustom": true
                }))
            );
            assert_eq!(
                agent_applied_layers_trace(agent),
                vec![
                    "component:app:main".to_string(),
                    "agent:test-agent:template:template-a".to_string(),
                    "agent:test-agent:template:template-b".to_string(),
                    "agent:test-agent:common".to_string(),
                ]
            );
        });
    }

    #[test]
    fn test_agent_resolution_fails_on_unknown_template() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            components:
              app:main:
                componentWasm: dummy-component.wasm

            agents:
              test-agent:
                templates: missing-template
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);

        let err = resolve_agents_for(&app, "app:main", "test-agent").unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("missing-template"),
            "error should mention missing template: {message}"
        );
        assert!(
            message.contains("test-agent"),
            "error should mention agent name: {message}"
        );
    }

    #[test]
    fn test_agent_resolution_fails_on_duplicate_template_layer() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            componentTemplates:
              shared-template:
                config:
                  source: template

            components:
              app:main:
                componentWasm: dummy-component.wasm

            agents:
              test-agent:
                templates: [shared-template, shared-template]
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);

        let err = resolve_agents_for(&app, "app:main", "test-agent").unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("Layer already exists") || message.contains("already exists"),
            "error should mention duplicate layer: {message}"
        );
        assert!(
            message.contains("shared-template"),
            "error should mention duplicate template name: {message}"
        );
    }

    #[test]
    fn deployment_subdomains_resolve_for_local_and_cloud() {
        let source = indoc! { r#"
            app: hello-app

            localServer:
              customRequestPort: 9016
              mcpPort: 9017

            environments:
              local:
                server: local
              implicit:
                default: true
              cloud:
                server: cloud

            httpApi:
              deployments:
                local:
                  - subdomain: hello-api
                implicit:
                  - subdomain: implicit-api
                cloud:
                  - subdomain: hello-api

            mcp:
              deployments:
                local:
                  - subdomain: hello-mcp
                implicit:
                  - subdomain: implicit-mcp
                cloud:
                  - subdomain: hello-mcp
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);

        assert!(
            app.http_api_deployments(&EnvironmentName("local".to_string()))
                .unwrap()
                .contains_key(&Domain("hello-api.localhost:9016".to_string()))
        );
        assert!(
            app.http_api_deployments(&EnvironmentName("cloud".to_string()))
                .unwrap()
                .contains_key(&Domain("hello-api.apps.golem.cloud".to_string()))
        );
        assert!(
            app.http_api_deployments(&EnvironmentName("implicit".to_string()))
                .unwrap()
                .contains_key(&Domain("implicit-api.localhost:9016".to_string()))
        );
        assert!(
            app.mcp_deployments(&EnvironmentName("local".to_string()))
                .unwrap()
                .contains_key(&Domain("hello-mcp.localhost:9017".to_string()))
        );
        assert!(
            app.mcp_deployments(&EnvironmentName("cloud".to_string()))
                .unwrap()
                .contains_key(&Domain("hello-mcp.mcps.golem.cloud".to_string()))
        );
        assert!(
            app.mcp_deployments(&EnvironmentName("implicit".to_string()))
                .unwrap()
                .contains_key(&Domain("implicit-mcp.localhost:9017".to_string()))
        );
    }

    #[test]
    fn deployment_domains_keep_full_domains_unchanged() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            httpApi:
              deployments:
                local:
                  - domain: api.example.com

            mcp:
              deployments:
                local:
                  - domain: mcp.example.com
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);

        assert!(
            app.http_api_deployments(&EnvironmentName("local".to_string()))
                .unwrap()
                .contains_key(&Domain("api.example.com".to_string()))
        );
        assert!(
            app.mcp_deployments(&EnvironmentName("local".to_string()))
                .unwrap()
                .contains_key(&Domain("mcp.example.com".to_string()))
        );
    }

    #[test]
    fn deployment_subdomains_use_default_local_ports_without_local_server() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            httpApi:
              deployments:
                local:
                  - subdomain: hello-api

            mcp:
              deployments:
                local:
                  - subdomain: hello-mcp
        "# };

        let (app, _app_tmp_dir) = load_app_for_env(source, "local", &[]);

        assert!(
            app.http_api_deployments(&EnvironmentName("local".to_string()))
                .unwrap()
                .contains_key(&Domain("hello-api.localhost:9006".to_string()))
        );
        assert!(
            app.mcp_deployments(&EnvironmentName("local".to_string()))
                .unwrap()
                .contains_key(&Domain("hello-mcp.localhost:9007".to_string()))
        );
    }

    #[test]
    fn local_server_rejects_zero_manifest_ports() {
        let source = indoc! { r#"
            app: hello-app

            localServer:
              routerPort: 0
              customRequestPort: 0
              mcpPort: 0
              portsFile: .golem/ports.json

            environments:
              local:
                server: local
        "# };

        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();
        let raw_apps = vec![
            app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path)
                .expect("raw manifest should parse"),
        ];

        let (app_name_and_envs, warns, errors) =
            Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(app_name_and_envs.is_none());
        assert_eq!(errors.len(), 3, "\n{}", errors.join("\n\n"));
        assert!(
            errors
                .iter()
                .any(|error| error.contains("localServer.routerPort")
                    && error.contains("--router-port 0")),
            "\n{}",
            errors.join("\n\n")
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("localServer.customRequestPort")
                    && error.contains("--custom-request-port 0")),
            "\n{}",
            errors.join("\n\n")
        );
        assert!(
            errors.iter().any(
                |error| error.contains("localServer.mcpPort") && error.contains("--mcp-port 0")
            ),
            "\n{}",
            errors.join("\n\n")
        );
    }

    #[test]
    fn deployment_subdomains_reject_invalid_values() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            httpApi:
              deployments:
                local:
                  - subdomain: hello.api
                  - subdomain: Hello
                  - subdomain: http://hello
        "# };

        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();
        let raw_apps = vec![
            app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path)
                .expect("raw manifest should parse"),
        ];

        let (app_name_and_envs, warns, errors) =
            Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = app_name_and_envs
        else {
            panic!("expected Some(ApplicationPreload)")
        };

        let (_app, warns, errors) = Application::from_raw_apps(
            std::env::current_dir().unwrap(),
            application_name,
            environments,
            local_server,
            selector("local", &[]),
            raw_apps,
        )
        .into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert_eq!(errors.len(), 3, "\n{}", errors.join("\n\n"));
        assert!(
            errors
                .iter()
                .all(|error| error.contains("Invalid deployment subdomain"))
        );
    }

    #[test]
    fn deployment_subdomains_reject_custom_server_environments() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              custom:
                server:
                  url: http://localhost:9881
                  workerUrl: http://localhost:9881
                  allowInsecure: true
                  auth:
                    staticToken: token

            httpApi:
              deployments:
                custom:
                  - subdomain: hello-api
        "# };

        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();
        let raw_apps = vec![
            app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path)
                .expect("raw manifest should parse"),
        ];

        let (app_name_and_envs, warns, errors) =
            Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = app_name_and_envs
        else {
            panic!("expected Some(ApplicationPreload)")
        };

        let (_app, warns, errors) = Application::from_raw_apps(
            std::env::current_dir().unwrap(),
            application_name,
            environments,
            local_server,
            selector("custom", &[]),
            raw_apps,
        )
        .into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert_eq!(errors.len(), 1, "\n{}", errors.join("\n\n"));
        assert!(
            errors[0].contains("Cannot use HTTP API deployment subdomain"),
            "\n{}",
            errors.join("\n\n")
        );
    }

    #[test]
    fn deployment_entries_require_exactly_one_domain_field() {
        let source = indoc! { r#"
            app: hello-app

            environments:
              local:
                server: local

            httpApi:
              deployments:
                local:
                  - domain: api.example.com
                    subdomain: hello-api
                  - agents: {}
        "# };

        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();
        let raw_apps = vec![
            app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path)
                .expect("raw manifest should parse"),
        ];

        let (app_name_and_envs, warns, errors) =
            Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = app_name_and_envs
        else {
            panic!("expected Some(ApplicationPreload)")
        };

        let (_app, warns, errors) = Application::from_raw_apps(
            std::env::current_dir().unwrap(),
            application_name,
            environments,
            local_server,
            selector("local", &[]),
            raw_apps,
        )
        .into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert_eq!(errors.len(), 2, "\n{}", errors.join("\n\n"));
        assert!(
            errors
                .iter()
                .any(|error| error.contains("cannot define both"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must define either"))
        );
    }

    fn component_applied_layers_trace(component: &crate::model::app::Component<'_>) -> Vec<String> {
        component
            .applied_layers()
            .iter()
            .map(|(id, selection)| match selection {
                Some(selection) => format!("{}[{selection}]", id.name()),
                None => id.name().to_string(),
            })
            .collect()
    }

    fn agent_applied_layers_trace(agent: crate::model::app::Agent<'_>) -> Vec<String> {
        agent
            .applied_layers()
            .iter()
            .map(|(id, selection)| match selection {
                Some(selection) => format!("{}[{selection}]", id.name()),
                None => id.name(),
            })
            .collect()
    }

    fn selector(environment: &str, presets: &[&str]) -> ComponentPresetSelector {
        ComponentPresetSelector {
            environment: environment.parse().unwrap(),
            presets: presets
                .iter()
                .map(|preset| preset.parse().unwrap())
                .collect(),
        }
    }

    fn parse_component_name(name: &str) -> ComponentName {
        name.parse().unwrap()
    }

    fn parse_agent_type_name(name: &str) -> AgentTypeName {
        name.parse().unwrap()
    }

    fn load_app_for_env(
        source: &str,
        environment: &str,
        presets: &[&str],
    ) -> (Application, TempDir) {
        load_app(source, &selector(environment, presets))
    }

    fn with_resolved_agent<T>(
        app: &Application,
        component_name_str: &str,
        agent_type_name_str: &str,
        test_fn: impl FnOnce(crate::model::app::Agent<'_>) -> T,
    ) -> T {
        let agent_type_name = parse_agent_type_name(agent_type_name_str);
        let resolved_agents = resolve_agents_for(app, component_name_str, agent_type_name_str)
            .unwrap_or_else(|err| {
                panic!("Failed to resolve test agent '{agent_type_name_str}': {err}")
            });
        let agent = resolved_agents.agent(&agent_type_name).unwrap();
        test_fn(agent)
    }

    fn resolve_agents_for(
        app: &Application,
        component_name_str: &str,
        agent_type_name_str: &str,
    ) -> anyhow::Result<crate::model::app::Agents> {
        let component_name = parse_component_name(component_name_str);
        let agent_type_name = parse_agent_type_name(agent_type_name_str);
        let mapping = BTreeMap::from([(agent_type_name, component_name)]);
        app.resolve_agents(&mapping)
    }

    fn load_app(source: &str, selector: &ComponentPresetSelector) -> (Application, TempDir) {
        let tmp_dir = tempfile::tempdir().unwrap();

        let golem_yaml_path = tmp_dir.path().join("golem.yaml");
        fs::write(&golem_yaml_path, source).unwrap();

        let raw_app = app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path).unwrap();
        let raw_apps = vec![raw_app];

        let (app_name_and_envs, warns, errors) =
            Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = app_name_and_envs
        else {
            panic!("expected Some(ApplicationPreload)")
        };

        let (app, warns, errors) = Application::from_raw_apps(
            std::env::current_dir().unwrap(),
            application_name,
            environments,
            local_server,
            selector.clone(),
            raw_apps,
        )
        .into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        (app.unwrap(), tmp_dir)
    }

    #[test]
    fn local_server_is_singleton_across_manifest_sources() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let root = tmp_dir.path().join("golem.yaml");
        let included = tmp_dir.path().join("included.yaml");

        fs::write(
            &root,
            indoc! {r#"
                app: test-app
                localServer:
                  routerPort: 9882
                environments:
                  local:
                    server: local
            "#},
        )
        .unwrap();
        fs::write(
            &included,
            indoc! {r#"
                localServer:
                  routerPort: 9883
            "#},
        )
        .unwrap();

        let raw_apps = vec![
            app_raw::ApplicationWithSource::from_yaml_file(&root).unwrap(),
            app_raw::ApplicationWithSource::from_yaml_file(&included).unwrap(),
        ];

        let (_app_name_and_envs, warns, errors) =
            Application::preload_from_raw_apps(&raw_apps).into_product();

        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("localServer"),
            "unexpected error: {}",
            errors[0]
        );
    }

    #[test]
    fn includes_loader_is_lenient_to_unknown_top_level_fields() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let golem_yaml_path = tmp_dir.path().join("golem.yaml");

        fs::write(
            &golem_yaml_path,
            indoc! {r#"
                manifestVersion: 1.5.0-dev.1
                includes:
                  - ./shared/*.yaml
                futureMigrationHints:
                  message: planned
            "#},
        )
        .unwrap();

        assert_eq!(
            includes_from_yaml_file(&golem_yaml_path),
            vec!["./shared/*.yaml".to_string()]
        );
    }
}
