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

use crate::fs;
use crate::log::LogColorize;
use crate::model::cascade::property::map::MapMergeMode;
use crate::model::cascade::property::vec::VecMergeMode;
use crate::model::component::AppComponentType;
use crate::model::format::Format;
use anyhow::{anyhow, Context};
use golem_common::model::component::{ComponentFilePath, ComponentFilePermissions};
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::http_api_definition::{GatewayBindingType, HttpApiDefinitionName};
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::path::PathBuf;
use url::Url;

#[derive(Clone, Debug)]
pub struct ApplicationWithSource {
    pub source: PathBuf,
    pub application: Application,
}

impl ApplicationWithSource {
    pub fn from_yaml_file(file: PathBuf) -> anyhow::Result<Self> {
        Self::from_yaml_string(file.clone(), fs::read_to_string(file.clone())?)
            .with_context(|| anyhow!("Failed to load source {}", file.log_color_highlight()))
    }

    pub fn from_yaml_string(source: PathBuf, string: String) -> serde_yaml::Result<Self> {
        Ok(Self {
            source,
            application: Application::from_yaml_str(string.as_str())?,
        })
    }

    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Application {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wit_deps: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub component_templates: IndexMap<String, ComponentTemplate>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub components: IndexMap<String, Component>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api: Option<HttpApi>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environments: IndexMap<String, Environment>,
}

impl Application {
    pub fn from_yaml_str(yaml: &str) -> serde_yaml::Result<Self> {
        serde_yaml::from_str(yaml)
    }

    pub fn to_yaml_string(&self) -> String {
        serde_yaml::to_string(self).expect("Failed to serialize Application as YAML")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentTemplate {
    #[serde(default, skip_serializing_if = "TemplateReferences::is_empty")]
    pub templates: TemplateReferences,
    #[serde(flatten)]
    pub component_properties: ComponentLayerProperties,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentLayerProperties>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    #[serde(default, skip_serializing_if = "TemplateReferences::is_empty")]
    pub templates: TemplateReferences,
    #[serde(flatten)]
    pub component_properties: ComponentLayerProperties,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentLayerProperties>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApi {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub definitions: IndexMap<HttpApiDefinitionName, HttpApiDefinition>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub deployments: IndexMap<EnvironmentName, Vec<HttpApiDeployment>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDefinition {
    // TODO: atomic: drop?
    pub version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<HttpApiDefinitionRoute>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDefinitionRoute {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    pub binding: HttpApiDefinitionBinding,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HttpApiDefinitionBindingType {
    #[default]
    Default,
    CorsPreflight,
    FileServer,
    HttpHandler,
    SwaggerUi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDefinitionBinding {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<HttpApiDefinitionBindingType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
}

impl HttpApiDefinitionBinding {
    pub fn to_diffable(&self) -> diff::HttpApiDefinitionBinding {
        diff::HttpApiDefinitionBinding {
            binding_type: match self.type_.unwrap_or_default() {
                HttpApiDefinitionBindingType::Default => GatewayBindingType::Worker,
                HttpApiDefinitionBindingType::CorsPreflight => GatewayBindingType::CorsPreflight,
                HttpApiDefinitionBindingType::FileServer => GatewayBindingType::FileServer,
                HttpApiDefinitionBindingType::HttpHandler => GatewayBindingType::HttpHandler,
                HttpApiDefinitionBindingType::SwaggerUi => GatewayBindingType::SwaggerUi,
            },
            component_name: self.component_name.clone(),
            worker_name: None, // TODO: atomic: check if we have restore it (and if it's agent compatible now)
            idempotency_key: self.idempotency_key.clone(),
            invocation_context: self.invocation_context.clone(),
            response: self.response.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDeployment {
    pub domain: Domain,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub server: Option<Server>,
    #[serde(skip_serializing_if = "Presets::is_empty", default)]
    pub component_presets: Presets,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cli: Option<CliOptions>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deployment: Option<DeploymentOptions>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum Server {
    Builtin(BuiltinServer),
    Custom(Box<CustomServer>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum BuiltinServer {
    Local,
    Cloud,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomServer {
    pub account: Option<String>,
    pub url: Url,
    pub worker_url: Option<Url>,
    pub allow_insecure: Option<bool>,
    pub auth: CustomServerAuth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum CustomServerAuth {
    OAuth2 { oauth2: Marker },
    Static { static_token: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CliOptions {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub format: Option<Format>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auto_confirm: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub redeploy_agents: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset: Option<Marker>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentOptions {
    // TODO: atomic
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialComponentFile {
    pub source_path: String,
    pub target_path: ComponentFilePath,
    pub permissions: Option<ComponentFilePermissions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
#[derive(Default)]
pub enum Presets {
    #[default]
    None,
    String(String),
    List(Vec<String>),
}

impl Presets {
    pub fn is_empty(&self) -> bool {
        match self {
            Presets::None => true,
            Presets::String(s) => Self::parse(s).next().is_none(),
            Presets::List(l) => l.is_empty(),
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::None => vec![],
            Self::String(s) => Self::parse(&s).collect(),
            Self::List(l) => l,
        }
    }

    fn parse(s: &str) -> impl Iterator<Item = String> + use<'_> {
        s.split([',', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
#[derive(Default)]
pub enum TemplateReferences {
    #[default]
    None,
    String(String),
    List(Vec<String>),
}

impl TemplateReferences {
    pub fn is_empty(&self) -> bool {
        match self {
            TemplateReferences::None => true,
            TemplateReferences::String(s) => Self::parse(s).next().is_none(),
            TemplateReferences::List(l) => l.is_empty(),
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::None => vec![],
            Self::String(s) => Self::parse(&s).collect(),
            Self::List(l) => l,
        }
    }

    fn parse(s: &str) -> impl Iterator<Item = String> + use<'_> {
        s.split([',', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentLayerProperties {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_wit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_wit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildCommand>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_type: Option<AppComponentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<Dependency>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, untagged)]
pub enum BuildCommand {
    External(ExternalCommand),
    QuickJSCrate(GenerateQuickJSCrate),
    QuickJSDTS(GenerateQuickJSDTS),
    AgentWrapper(GenerateAgentWrapper),
    ComposeAgentWrapper(ComposeAgentWrapper),
    InjectToPrebuiltQuickJs(InjectToPrebuiltQuickJs),
}

impl BuildCommand {
    pub fn dir(&self) -> Option<&str> {
        match self {
            BuildCommand::External(cmd) => cmd.dir.as_deref(),
            BuildCommand::QuickJSCrate(_) => None,
            BuildCommand::QuickJSDTS(_) => None,
            BuildCommand::AgentWrapper(_) => None,
            BuildCommand::ComposeAgentWrapper(_) => None,
            BuildCommand::InjectToPrebuiltQuickJs(_) => None,
        }
    }

    pub fn targets(&self) -> Vec<String> {
        match self {
            BuildCommand::External(cmd) => cmd.targets.clone(),
            BuildCommand::QuickJSCrate(cmd) => vec![cmd.generate_quickjs_crate.clone()],
            BuildCommand::QuickJSDTS(cmd) => vec![cmd.generate_quickjs_dts.clone()],
            BuildCommand::AgentWrapper(cmd) => vec![cmd.generate_agent_wrapper.clone()],
            BuildCommand::ComposeAgentWrapper(cmd) => vec![cmd.to.clone()],
            BuildCommand::InjectToPrebuiltQuickJs(cmd) => vec![cmd.into.clone()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rmdirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mkdirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateQuickJSCrate {
    pub generate_quickjs_crate: String,
    pub wit: String,
    pub js_modules: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateQuickJSDTS {
    pub generate_quickjs_dts: String,
    pub wit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerateAgentWrapper {
    /// The target path of the generated wrapper component
    pub generate_agent_wrapper: String,
    /// The path of the compiled WASM component containing the dynamic golem:agent implementation
    pub based_on_compiled_wasm: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposeAgentWrapper {
    /// The target path of the generated wrapper component
    pub compose_agent_wrapper: String,
    /// The path of the compiled WASM component implementing golem:agent
    pub with_agent: String,
    /// The path of the resulting composed WASM component
    pub to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InjectToPrebuiltQuickJs {
    /// The path to the prebuilt QuickJS WASM file that loads a JS module through a get-script import
    pub inject_to_prebuilt_quickjs: String,
    /// The path to the JS module
    pub module: String,
    /// The path to the intermediate WASM containing the JS module
    pub module_wasm: String,
    /// The path to the output WASM component containing the injected JS module
    pub into: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dependency {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallation {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker;

impl Serialize for Marker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for Marker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match bool::deserialize(deserializer)? {
            true => Ok(Marker),
            false => Err(serde::de::Error::custom(
                "value must be `true`, `false` is not allowed",
            )),
        }
    }
}
