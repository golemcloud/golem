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

use crate::log::LogColorize;
use crate::model::GuestLanguage;
use crate::model::cascade::property::map::MapMergeMode;
use crate::model::cascade::property::vec::VecMergeMode;
use crate::model::format::Format;
use crate::{APP_MANIFEST_JSON_SCHEMA, fs};
use anyhow::{Context, anyhow};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::quota::ResourceDefinitionCreation;
use golem_common::model::security_scheme::SecuritySchemeName;
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use strum::IntoEnumIterator;
use url::Url;

struct NoopRetriever;

impl jsonschema::Retrieve for NoopRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(format!("External schema retrieval is disabled: {uri}").into())
    }
}

static JSON_SCHEMA_VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str::<serde_json::Value>(APP_MANIFEST_JSON_SCHEMA)
        .expect("Invalid Application manifest JSON schema: cannot parse as JSON");
    jsonschema::options()
        .with_retriever(NoopRetriever)
        .build(&schema)
        .expect("Invalid Application manifest JSON schema: cannot create validator")
});

#[derive(Clone, Debug)]
pub struct ApplicationWithSource {
    pub source: PathBuf,
    pub application: Application,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApplicationMetadata {
    pub manifest_version: Option<String>,
    pub includes: Vec<String>,
}

impl ApplicationMetadata {
    pub fn from_yaml_str(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }
}

impl ApplicationWithSource {
    pub fn from_yaml_file(file: &Path) -> anyhow::Result<Self> {
        Self::from_yaml_string(file.to_path_buf(), &fs::read_to_string(file)?)
            .with_context(|| anyhow!("Failed to load source {}", file.log_color_highlight()))
    }

    pub fn from_yaml_string(source: PathBuf, string: &str) -> anyhow::Result<Self> {
        Ok(Self {
            source,
            application: Application::from_yaml_str(string)?,
        })
    }

    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Application {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub includes: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub component_templates: IndexMap<String, ComponentTemplate>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub components: IndexMap<String, Component>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<AgentTypeName, Agent>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api: Option<HttpApi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<Mcp>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environments: IndexMap<String, Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<BridgeSdks>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub secret_defaults: IndexMap<EnvironmentName, Vec<EnvironmentAgentSecret>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub retry_policy_defaults: IndexMap<EnvironmentName, Vec<EnvironmentRetryPolicyDefault>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub resource_defaults: IndexMap<EnvironmentName, Vec<ResourceDefinitionCreation>>,
}

#[derive(Debug)]
struct JsonSchemaValidationError {
    schema: String,
    error: String,
}

#[derive(Debug)]
pub struct DeserializationError {
    serde_yaml_error: serde_yaml::Error,
    json_schema_validation_errors_by_path: BTreeMap<String, Vec<JsonSchemaValidationError>>,
}

impl DeserializationError {
    fn new(
        serde_yaml_error: serde_yaml::Error,
        json_schema_evaluation: Option<jsonschema::Evaluation>,
    ) -> Self {
        match json_schema_evaluation {
            Some(evaluation) => {
                if evaluation.flag().valid {
                    serde_yaml_error.into()
                } else {
                    let mut schema_errors =
                        BTreeMap::<String, Vec<JsonSchemaValidationError>>::new();

                    for error in evaluation.iter_errors() {
                        let path = format!(".{}", error.instance_location);
                        schema_errors
                            .entry(path)
                            .or_default()
                            .push(JsonSchemaValidationError {
                                schema: error.schema_location.to_string(),
                                error: error.error.to_string(),
                            })
                    }

                    Self {
                        serde_yaml_error,
                        json_schema_validation_errors_by_path: schema_errors,
                    }
                }
            }
            None => serde_yaml_error.into(),
        }
    }
}

impl From<serde_yaml::Error> for DeserializationError {
    fn from(value: serde_yaml::Error) -> Self {
        Self {
            serde_yaml_error: value,
            json_schema_validation_errors_by_path: BTreeMap::new(),
        }
    }
}

impl Display for DeserializationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.json_schema_validation_errors_by_path.is_empty() {
            write!(f, "{}", self.serde_yaml_error)
        } else {
            writeln!(f, "Failed to deserialize application manifest:")?;
            writeln!(
                f,
                "  {}",
                "YAML deserialization error:".log_color_help_group()
            )?;
            writeln!(
                f,
                "    {}",
                self.serde_yaml_error
                    .to_string()
                    .log_color_error_highlight()
            )?;

            if !self.json_schema_validation_errors_by_path.is_empty() {
                writeln!(
                    f,
                    "  {}",
                    "Schema validation hint(s):".log_color_help_group()
                )?;
                for (path, errors) in &self.json_schema_validation_errors_by_path {
                    writeln!(f, "    path: {}", path.log_color_highlight())?;
                    for error in errors {
                        writeln!(f, "      - schema: {}", error.schema)?;
                        writeln!(f, "        error: {}", error.error.log_color_warn())?;
                    }
                }
            }

            Ok(())
        }
    }
}

impl Error for DeserializationError {}

impl Application {
    pub fn from_yaml_str(yaml: &str) -> Result<Self, DeserializationError> {
        match serde_yaml::from_str::<Self>(yaml) {
            Ok(app) => Ok(app),
            Err(err) => Err(DeserializationError::new(
                err,
                serde_yaml::from_str::<serde_json::Value>(yaml)
                    .ok()
                    .map(json_value_without_null_fields)
                    .map(|app| JSON_SCHEMA_VALIDATOR.evaluate(&app)),
            )),
        }
    }

    pub fn to_yaml_string(&self) -> String {
        serde_yaml::to_string(self).expect("Failed to serialize Application as YAML")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentTemplate {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildCommand>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentPreset>,
}

impl ComponentTemplate {
    pub fn component_layer_properties(&self) -> ComponentLayerProperties {
        ComponentLayerProperties {
            component_wasm: self.component_wasm.clone(),
            output_wasm: self.output_wasm.clone(),
            build_merge_mode: self.build_merge_mode,
            build: self.build.clone(),
            custom_commands: self.custom_commands.clone(),
            clean: self.clean.clone(),
            agent_properties: AgentLayerProperties {
                config: self.config.clone(),
                env_merge_mode: self.env_merge_mode,
                env: self.env.clone(),
                wasi_config_merge_mode: self.wasi_config_merge_mode,
                wasi_config: self.wasi_config.clone(),
                plugins_merge_mode: self.plugins_merge_mode,
                plugins: self.plugins.clone(),
                files_merge_mode: self.files_merge_mode,
                files: self.files.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildCommand>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentPreset>,
}

impl Component {
    pub fn component_layer_properties(&self) -> ComponentLayerProperties {
        ComponentLayerProperties {
            component_wasm: self.component_wasm.clone(),
            output_wasm: self.output_wasm.clone(),
            build_merge_mode: self.build_merge_mode,
            build: self.build.clone(),
            custom_commands: self.custom_commands.clone(),
            clean: self.clean.clone(),
            agent_properties: AgentLayerProperties {
                config: self.config.clone(),
                env_merge_mode: self.env_merge_mode,
                env: self.env.clone(),
                wasi_config_merge_mode: self.wasi_config_merge_mode,
                wasi_config: self.wasi_config.clone(),
                plugins_merge_mode: self.plugins_merge_mode,
                plugins: self.plugins.clone(),
                files_merge_mode: self.files_merge_mode,
                files: self.files.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentPreset {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildCommand>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
}

impl ComponentPreset {
    pub fn into_component_layer_properties(self) -> ComponentLayerProperties {
        ComponentLayerProperties {
            component_wasm: self.component_wasm,
            output_wasm: self.output_wasm,
            build_merge_mode: self.build_merge_mode,
            build: self.build,
            custom_commands: self.custom_commands,
            clean: self.clean,
            agent_properties: AgentLayerProperties {
                config: self.config,
                env_merge_mode: self.env_merge_mode,
                env: self.env,
                wasi_config_merge_mode: self.wasi_config_merge_mode,
                wasi_config: self.wasi_config,
                plugins_merge_mode: self.plugins_merge_mode,
                plugins: self.plugins,
                files_merge_mode: self.files_merge_mode,
                files: self.files,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, AgentPreset>,
}

impl Agent {
    pub fn agent_layer_properties(&self) -> AgentLayerProperties {
        AgentLayerProperties {
            config: self.config.clone(),
            env_merge_mode: self.env_merge_mode,
            env: self.env.clone(),
            wasi_config_merge_mode: self.wasi_config_merge_mode,
            wasi_config: self.wasi_config.clone(),
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins.clone(),
            files_merge_mode: self.files_merge_mode,
            files: self.files.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPreset {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasi_config: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
}

impl AgentPreset {
    pub fn into_agent_layer_properties(self) -> AgentLayerProperties {
        AgentLayerProperties {
            config: self.config,
            env_merge_mode: self.env_merge_mode,
            env: self.env,
            wasi_config_merge_mode: self.wasi_config_merge_mode,
            wasi_config: self.wasi_config,
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins,
            files_merge_mode: self.files_merge_mode,
            files: self.files,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentAgentSecret {
    pub path: Vec<String>,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentRetryPolicyDefault {
    pub name: String,
    pub priority: u32,
    pub predicate: golem_common::model::retry_policy::Predicate,
    pub policy: golem_common::model::retry_policy::RetryPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApi {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub deployments: IndexMap<EnvironmentName, Vec<HttpApiDeployment>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDeployment {
    pub domain: Domain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<AgentTypeName, HttpApiDeploymentAgentOptions>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mcp {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub deployments: IndexMap<EnvironmentName, Vec<McpDeployment>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpDeployment {
    pub domain: Domain,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<AgentTypeName, McpDeploymentAgentOptions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpDeploymentAgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default: Option<Marker>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub server: Option<Server>,
    #[serde(skip_serializing_if = "LenientTokenList::is_empty", default)]
    pub component_presets: LenientTokenList,
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
    pub url: Url,
    pub worker_url: Option<Url>,
    pub allow_insecure: Option<bool>,
    pub auth: CustomServerAuth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
pub enum CustomServerAuth {
    OAuth2 {
        oauth2: Marker,
    },
    #[serde[rename_all = "camelCase"]]
    Static {
        static_token: String,
    },
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
    pub compatibility_check: Option<bool>,
    pub version_check: Option<bool>,
    pub security_overrides: Option<bool>,
}

impl DeploymentOptions {
    pub fn new_local() -> Self {
        Self {
            compatibility_check: Some(false),
            version_check: Some(false),
            security_overrides: Some(true),
        }
    }

    pub fn new_cloud() -> Self {
        Self {
            compatibility_check: None,
            version_check: None,
            security_overrides: None,
        }
    }

    pub fn with_defaults_from(mut self, other: Self) -> Self {
        if self.compatibility_check.is_none() {
            self.compatibility_check = other.compatibility_check;
        }
        if self.version_check.is_none() {
            self.version_check = other.version_check;
        }
        if self.security_overrides.is_none() {
            self.security_overrides = other.security_overrides;
        }
        self
    }

    pub fn compatibility_check(&self) -> bool {
        self.compatibility_check.unwrap_or(true)
    }

    pub fn version_check(&self) -> bool {
        // TODO: atomic: switch to true, once versioning is implemented
        self.version_check.unwrap_or(false)
    }

    pub fn security_overrides(&self) -> bool {
        self.security_overrides.unwrap_or(false)
    }

    pub fn to_diffable(&self) -> diff::Environment {
        diff::Environment {
            compatibility_check: self.compatibility_check(),
            version_check: self.version_check(),
            security_overrides: self.security_overrides(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialComponentFile {
    pub source_path: String,
    pub target_path: CanonicalFilePath,
    pub permissions: Option<AgentFilePermissions>,
}

// Common component-level fields merged from templates/components/presets.
// This helper intentionally stays outside serde parsing: using flatten here would weaken
// strict unknown-field checks on manifest-facing structs that use deny_unknown_fields.
#[derive(Clone, Debug)]
pub struct ComponentLayerProperties {
    pub component_wasm: Option<String>,
    pub output_wasm: Option<String>,
    pub build_merge_mode: Option<VecMergeMode>,
    pub build: Vec<BuildCommand>,
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    pub clean: Vec<String>,
    pub agent_properties: AgentLayerProperties,
}

// Common agent-level fields merged from templates/agents/presets.
// This helper intentionally stays outside serde parsing: using flatten here would weaken
// strict unknown-field checks on manifest-facing structs that use deny_unknown_fields.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLayerProperties {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_merge_mode: Option<VecMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InitialComponentFile>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpApiDeploymentAgentOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<SecuritySchemeName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_session_header_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, untagged)]
pub enum BuildCommand {
    External(ExternalCommand),
    QuickJSCrate(GenerateQuickJSCrate),
    QuickJSDTS(GenerateQuickJSDTS),
    InjectToPrebuiltQuickJs(InjectToPrebuiltQuickJs),
    PreinitializeJs(PreinitializeJs),
}

impl BuildCommand {
    pub fn dir(&self) -> Option<&str> {
        match self {
            BuildCommand::External(cmd) => cmd.dir.as_deref(),
            BuildCommand::QuickJSCrate(_) => None,
            BuildCommand::QuickJSDTS(_) => None,
            BuildCommand::InjectToPrebuiltQuickJs(_) => None,
            BuildCommand::PreinitializeJs(_) => None,
        }
    }

    pub fn targets(&self) -> Vec<String> {
        match self {
            BuildCommand::External(cmd) => cmd.targets.clone(),
            BuildCommand::QuickJSCrate(cmd) => vec![cmd.generate_quickjs_crate.clone()],
            BuildCommand::QuickJSDTS(cmd) => vec![cmd.generate_quickjs_dts.clone()],
            BuildCommand::InjectToPrebuiltQuickJs(cmd) => vec![cmd.into.clone()],
            BuildCommand::PreinitializeJs(cmd) => vec![cmd.into.clone()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
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
pub struct InjectToPrebuiltQuickJs {
    /// The path to the prebuilt QuickJS WASM file with a binary slot for JS injection
    pub inject_to_prebuilt_quickjs: String,
    /// The path to the JS module
    pub module: String,
    /// The path to the output WASM component containing the injected JS module
    pub into: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreinitializeJs {
    /// The path to the input WASM component to pre-initialize
    pub preinitialize_js: String,
    /// The path to the pre-initialized output WASM component
    pub into: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInstallation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<BridgeSdkLanguageTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<BridgeSdkLanguageTargets>,
}

impl BridgeSdks {
    pub fn for_language(&self, language: GuestLanguage) -> Option<&BridgeSdkLanguageTargets> {
        match language {
            GuestLanguage::Rust => self.rust.as_ref(),
            GuestLanguage::TypeScript => self.ts.as_ref(),
            GuestLanguage::Scala | GuestLanguage::MoonBit => None,
        }
    }

    pub fn for_all_languages(
        &self,
    ) -> impl Iterator<Item = (GuestLanguage, Option<&BridgeSdkLanguageTargets>)> {
        GuestLanguage::iter().map(|lang| (lang, self.for_language(lang)))
    }

    pub fn for_all_used_languages(
        &self,
    ) -> impl Iterator<Item = (GuestLanguage, &BridgeSdkLanguageTargets)> {
        self.for_all_languages().filter_map(|(lang, targets)| {
            targets.and_then(|targets| (!targets.agents.is_empty()).then_some((lang, targets)))
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdkLanguageTargets {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub agents: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase", deny_unknown_fields)]
#[derive(Default)]
pub enum LenientTokenList {
    #[default]
    None,
    String(String),
    List(Vec<String>),
}

impl LenientTokenList {
    pub fn is_empty(&self) -> bool {
        match self {
            LenientTokenList::None => true,
            LenientTokenList::String(s) => Self::parse(s).next().is_none(),
            LenientTokenList::List(l) => l.is_empty(),
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::None => vec![],
            Self::String(s) => Self::parse(&s).collect(),
            Self::List(l) => l,
        }
    }

    pub fn into_set(self) -> BTreeSet<String> {
        match self {
            Self::None => BTreeSet::new(),
            Self::String(s) => Self::parse(&s).collect(),
            Self::List(l) => l.into_iter().collect(),
        }
    }

    fn parse(s: &str) -> impl Iterator<Item = String> + use<'_> {
        s.split([',', '\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

fn json_value_without_null_fields(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => value,
        serde_json::Value::Array(array) => array
            .into_iter()
            .map(json_value_without_null_fields)
            .collect::<Vec<_>>()
            .into(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .filter_map(|(k, v)| {
                if v != serde_json::Value::Null {
                    Some((k, json_value_without_null_fields(v)))
                } else {
                    None
                }
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::model::app_raw::{Application, JSON_SCHEMA_VALIDATOR};
    use crate::model::cascade::property::map::MapMergeMode;
    use crate::model::cascade::property::vec::VecMergeMode;
    use crate::model::format::Format;
    use golem_common::base_model::retry_policy::{
        ApiAddDelayPolicy, ApiBooleanValue, ApiClampPolicy, ApiCountBoxPolicy,
        ApiExponentialPolicy, ApiFibonacciPolicy, ApiFilteredOnPolicy, ApiImmediatePolicy,
        ApiIntegerValue, ApiJitterPolicy, ApiNeverPolicy, ApiPeriodicPolicy, ApiPredicate,
        ApiPredicateFalse, ApiPredicateNot, ApiPredicatePair, ApiPredicateTrue, ApiPredicateValue,
        ApiPropertyComparison, ApiPropertyExistence, ApiPropertyPattern, ApiPropertyPrefix,
        ApiPropertySetCheck, ApiPropertySubstring, ApiRetryPolicy, ApiRetryPolicyPair,
        ApiTextValue, ApiTimeBoxPolicy,
    };
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::EnvironmentName;
    use golem_common::model::quota::{
        EnforcementAction, ResourceCapacityLimit, ResourceConcurrencyLimit,
        ResourceDefinitionCreation, ResourceLimit, ResourceName, ResourceRateLimit, TimePeriod,
    };
    use golem_common::model::security_scheme::SecuritySchemeName;
    use indexmap::IndexMap;
    use proptest::prelude::*;
    use proptest::string::string_regex;
    use serde_json::Value;
    use std::collections::HashMap;
    #[allow(unused_imports)]
    use test_r::test;
    use url::Url;

    fn arb_opt<T: Clone + std::fmt::Debug + 'static>(
        strategy: BoxedStrategy<T>,
    ) -> BoxedStrategy<Option<T>> {
        prop_oneof![3 => strategy.prop_map(Some), 2 => Just(None)].boxed()
    }

    fn arb_ident() -> BoxedStrategy<String> {
        string_regex("[a-z][a-z0-9_-]{0,12}").unwrap().boxed()
    }

    fn arb_dns_label() -> BoxedStrategy<String> {
        string_regex("[a-z][a-z0-9-]{0,12}").unwrap().boxed()
    }

    fn arb_semver() -> BoxedStrategy<String> {
        (0u8..=9, 0u8..=9, 0u8..=9)
            .prop_map(|(a, b, c)| format!("{a}.{b}.{c}"))
            .boxed()
    }

    fn arb_json_value() -> BoxedStrategy<Value> {
        let leaf = prop_oneof![
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            arb_ident().prop_map(Value::String),
        ];

        leaf.prop_recursive(3, 64, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..=3).prop_map(Value::Array),
                prop::collection::btree_map(arb_ident(), inner, 0..=3)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
        .boxed()
    }

    fn arb_token_list_model() -> BoxedStrategy<LenientTokenList> {
        prop_oneof![
            Just(LenientTokenList::None),
            arb_ident().prop_map(LenientTokenList::String),
            prop::collection::vec(arb_ident(), 1..=3).prop_map(LenientTokenList::List),
        ]
        .boxed()
    }

    fn arb_map_merge_mode_model() -> BoxedStrategy<MapMergeMode> {
        prop_oneof![
            Just(MapMergeMode::Upsert),
            Just(MapMergeMode::Replace),
            Just(MapMergeMode::Remove),
        ]
        .boxed()
    }

    fn arb_vec_merge_mode_model() -> BoxedStrategy<VecMergeMode> {
        prop_oneof![
            Just(VecMergeMode::Append),
            Just(VecMergeMode::Prepend),
            Just(VecMergeMode::Replace),
        ]
        .boxed()
    }

    fn arb_string_index_map_model() -> BoxedStrategy<IndexMap<String, String>> {
        prop::collection::vec((arb_ident(), arb_ident()), 0..=3)
            .prop_map(IndexMap::from_iter)
            .boxed()
    }

    fn arb_url_model() -> BoxedStrategy<Url> {
        arb_dns_label()
            .prop_filter_map("valid url", |host| {
                Url::parse(&format!("https://{host}.example.com")).ok()
            })
            .boxed()
    }

    fn arb_external_command_model() -> BoxedStrategy<ExternalCommand> {
        (
            arb_ident(),
            arb_opt(arb_ident()),
            arb_string_index_map_model(),
            prop::collection::vec(arb_ident(), 0..=2),
            prop::collection::vec(arb_ident(), 0..=2),
            prop::collection::vec(arb_ident(), 0..=2),
            prop::collection::vec(arb_ident(), 0..=2),
        )
            .prop_map(
                |(command, dir, env, rmdirs, mkdirs, sources, targets)| ExternalCommand {
                    command,
                    dir,
                    env,
                    rmdirs,
                    mkdirs,
                    sources,
                    targets,
                },
            )
            .boxed()
    }

    fn arb_build_commands_model() -> BoxedStrategy<Vec<BuildCommand>> {
        prop::collection::vec(
            arb_external_command_model().prop_map(BuildCommand::External),
            0..=3,
        )
        .boxed()
    }

    fn arb_plugin_installation_model() -> BoxedStrategy<PluginInstallation> {
        (
            arb_opt(arb_ident()),
            arb_ident(),
            string_regex("[0-9]+\\.[0-9]+\\.[0-9]+(-[a-z0-9.]+)?").unwrap(),
            arb_string_index_map_model(),
        )
            .prop_map(|(account, name, version, parameters)| PluginInstallation {
                account,
                name,
                version,
                parameters: parameters.into_iter().collect::<HashMap<_, _>>(),
            })
            .boxed()
    }

    fn arb_initial_component_file_model() -> BoxedStrategy<InitialComponentFile> {
        (arb_ident(), arb_ident(), any::<bool>())
            .prop_map(
                |(source_path, target_name, writable)| InitialComponentFile {
                    source_path,
                    target_path: CanonicalFilePath::from_abs_str(&format!("/{target_name}"))
                        .unwrap(),
                    permissions: Some(if writable {
                        AgentFilePermissions::ReadWrite
                    } else {
                        AgentFilePermissions::ReadOnly
                    }),
                },
            )
            .boxed()
    }

    fn arb_component_preset_model() -> BoxedStrategy<ComponentPreset> {
        (
            any::<bool>(),
            arb_opt(arb_ident()),
            arb_opt(arb_ident()),
            (
                arb_opt(arb_vec_merge_mode_model()),
                arb_build_commands_model(),
                prop::collection::vec(
                    (
                        arb_ident(),
                        prop::collection::vec(arb_external_command_model(), 0..=2),
                    ),
                    0..=2,
                )
                .prop_map(IndexMap::from_iter),
                prop::collection::vec(arb_ident(), 0..=3),
            ),
            (
                arb_opt(arb_json_value()),
                arb_opt(arb_map_merge_mode_model()),
                arb_opt(arb_string_index_map_model()),
                arb_opt(arb_map_merge_mode_model()),
                arb_opt(arb_string_index_map_model()),
            ),
            (
                arb_opt(arb_vec_merge_mode_model()),
                arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
                arb_opt(arb_vec_merge_mode_model()),
                arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
            ),
        )
            .prop_map(
                |(
                    is_default,
                    component_wasm,
                    output_wasm,
                    (build_merge_mode, build, custom_commands, clean),
                    (config, env_merge_mode, env, wasi_config_merge_mode, wasi_config),
                    (plugins_merge_mode, plugins, files_merge_mode, files),
                )| ComponentPreset {
                    default: is_default.then_some(Marker),
                    component_wasm,
                    output_wasm,
                    build_merge_mode,
                    build,
                    custom_commands,
                    clean,
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                },
            )
            .boxed()
    }

    fn arb_component_template_model() -> BoxedStrategy<ComponentTemplate> {
        (
            arb_token_list_model(),
            arb_opt(arb_ident()),
            arb_opt(arb_ident()),
            (
                arb_opt(arb_vec_merge_mode_model()),
                arb_build_commands_model(),
                prop::collection::vec(
                    (
                        arb_ident(),
                        prop::collection::vec(arb_external_command_model(), 0..=2),
                    ),
                    0..=2,
                )
                .prop_map(IndexMap::from_iter),
                prop::collection::vec(arb_ident(), 0..=3),
            ),
            (
                arb_opt(arb_json_value()),
                arb_opt(arb_map_merge_mode_model()),
                arb_opt(arb_string_index_map_model()),
                arb_opt(arb_map_merge_mode_model()),
                arb_opt(arb_string_index_map_model()),
            ),
            (
                arb_opt(arb_vec_merge_mode_model()),
                arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
                arb_opt(arb_vec_merge_mode_model()),
                arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
            ),
            prop::collection::vec((arb_ident(), arb_component_preset_model()), 0..=2)
                .prop_map(IndexMap::from_iter),
        )
            .prop_map(
                |(
                    templates,
                    component_wasm,
                    output_wasm,
                    (build_merge_mode, build, custom_commands, clean),
                    (config, env_merge_mode, env, wasi_config_merge_mode, wasi_config),
                    (plugins_merge_mode, plugins, files_merge_mode, files),
                    presets,
                )| ComponentTemplate {
                    templates,
                    component_wasm,
                    output_wasm,
                    build_merge_mode,
                    build,
                    custom_commands,
                    clean,
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                    presets,
                },
            )
            .boxed()
    }

    fn arb_component_model() -> BoxedStrategy<Component> {
        (
            arb_token_list_model(),
            arb_opt(arb_ident()),
            arb_opt(arb_ident()),
            arb_opt(arb_ident()),
            (
                arb_opt(arb_vec_merge_mode_model()),
                arb_build_commands_model(),
                prop::collection::vec(
                    (
                        arb_ident(),
                        prop::collection::vec(arb_external_command_model(), 0..=2),
                    ),
                    0..=2,
                )
                .prop_map(IndexMap::from_iter),
                prop::collection::vec(arb_ident(), 0..=3),
            ),
            (
                arb_opt(arb_json_value()),
                arb_opt(arb_map_merge_mode_model()),
                arb_opt(arb_string_index_map_model()),
                arb_opt(arb_map_merge_mode_model()),
                arb_opt(arb_string_index_map_model()),
            ),
            (
                arb_opt(arb_vec_merge_mode_model()),
                arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
                arb_opt(arb_vec_merge_mode_model()),
                arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
            ),
            prop::collection::vec((arb_ident(), arb_component_preset_model()), 0..=2)
                .prop_map(IndexMap::from_iter),
        )
            .prop_map(
                |(
                    templates,
                    dir,
                    component_wasm,
                    output_wasm,
                    (build_merge_mode, build, custom_commands, clean),
                    (config, env_merge_mode, env, wasi_config_merge_mode, wasi_config),
                    (plugins_merge_mode, plugins, files_merge_mode, files),
                    presets,
                )| Component {
                    templates,
                    dir,
                    component_wasm,
                    output_wasm,
                    build_merge_mode,
                    build,
                    custom_commands,
                    clean,
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                    presets,
                },
            )
            .boxed()
    }

    fn arb_agent_preset_model() -> BoxedStrategy<AgentPreset> {
        (
            any::<bool>(),
            arb_opt(arb_json_value()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        )
            .prop_map(
                |(
                    is_default,
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                )| AgentPreset {
                    default: is_default.then_some(Marker),
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                },
            )
            .boxed()
    }

    fn arb_agent_model() -> BoxedStrategy<Agent> {
        (
            arb_token_list_model(),
            arb_opt(arb_json_value()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
            prop::collection::vec((arb_ident(), arb_agent_preset_model()), 0..=2)
                .prop_map(IndexMap::from_iter),
        )
            .prop_map(
                |(
                    templates,
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                    presets,
                )| Agent {
                    templates,
                    config,
                    env_merge_mode,
                    env,
                    wasi_config_merge_mode,
                    wasi_config,
                    plugins_merge_mode,
                    plugins,
                    files_merge_mode,
                    files,
                    presets,
                },
            )
            .boxed()
    }

    fn arb_server_model() -> BoxedStrategy<Server> {
        prop_oneof![
            Just(Server::Builtin(BuiltinServer::Local)),
            Just(Server::Builtin(BuiltinServer::Cloud)),
            (
                arb_url_model(),
                arb_url_model(),
                any::<bool>(),
                any::<bool>(),
                arb_ident(),
            )
                .prop_map(
                    |(url, worker_url, allow_insecure, use_oauth, static_token)| {
                        let auth = if use_oauth {
                            CustomServerAuth::OAuth2 { oauth2: Marker }
                        } else {
                            CustomServerAuth::Static { static_token }
                        };

                        Server::Custom(Box::new(CustomServer {
                            url,
                            worker_url: Some(worker_url),
                            allow_insecure: Some(allow_insecure),
                            auth,
                        }))
                    }
                ),
        ]
        .boxed()
    }

    fn arb_cli_options_model() -> BoxedStrategy<CliOptions> {
        (
            arb_opt(prop_oneof![Just(Format::Text), Just(Format::Json)].boxed()),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(format, auto_confirm, redeploy_agents, reset)| CliOptions {
                    format,
                    auto_confirm: auto_confirm.then_some(Marker),
                    redeploy_agents: redeploy_agents.then_some(Marker),
                    reset: reset.then_some(Marker),
                },
            )
            .boxed()
    }

    fn arb_deployment_options_model() -> BoxedStrategy<DeploymentOptions> {
        (any::<bool>(), any::<bool>(), any::<bool>())
            .prop_map(|(compatibility_check, version_check, security_overrides)| {
                DeploymentOptions {
                    compatibility_check: Some(compatibility_check),
                    version_check: Some(version_check),
                    security_overrides: Some(security_overrides),
                }
            })
            .boxed()
    }

    fn arb_environment_model() -> BoxedStrategy<Environment> {
        (
            any::<bool>(),
            arb_opt(arb_ident()),
            arb_opt(arb_server_model()),
            arb_token_list_model(),
            arb_opt(arb_cli_options_model()),
            arb_opt(arb_deployment_options_model()),
        )
            .prop_map(
                |(is_default, account, server, component_presets, cli, deployment)| Environment {
                    default: is_default.then_some(Marker),
                    account,
                    server,
                    component_presets,
                    cli,
                    deployment,
                },
            )
            .boxed()
    }

    fn arb_http_api_deployment_model() -> BoxedStrategy<HttpApiDeployment> {
        (
            arb_dns_label(),
            arb_opt(
                (arb_dns_label(), arb_ident())
                    .prop_map(|(host, path)| format!("https://{host}.example.com/{path}"))
                    .boxed(),
            ),
            prop::collection::vec(
                (
                    arb_ident().prop_map(AgentTypeName),
                    (
                        arb_opt(arb_ident().prop_map(SecuritySchemeName).boxed()),
                        arb_opt(arb_ident()),
                    )
                        .prop_map(
                            |(security_scheme, test_session_header_name)| {
                                HttpApiDeploymentAgentOptions {
                                    security_scheme,
                                    test_session_header_name,
                                }
                            },
                        ),
                ),
                0..=3,
            )
            .prop_map(IndexMap::from_iter),
        )
            .prop_map(|(domain, webhook_url, agents)| HttpApiDeployment {
                domain: Domain(format!("{domain}.example.com")),
                webhook_url,
                agents,
            })
            .boxed()
    }

    fn arb_http_api_model() -> BoxedStrategy<HttpApi> {
        prop::collection::vec(
            (
                arb_ident().prop_map(EnvironmentName),
                prop::collection::vec(arb_http_api_deployment_model(), 0..=2),
            ),
            0..=3,
        )
        .prop_map(|deployments| HttpApi {
            deployments: IndexMap::from_iter(deployments),
        })
        .boxed()
    }

    fn arb_mcp_deployment_model() -> BoxedStrategy<McpDeployment> {
        (
            arb_dns_label(),
            prop::collection::vec(
                (
                    arb_ident().prop_map(AgentTypeName),
                    arb_opt(arb_ident())
                        .prop_map(|security_scheme| McpDeploymentAgentOptions { security_scheme }),
                ),
                0..=3,
            )
            .prop_map(IndexMap::from_iter),
        )
            .prop_map(|(domain, agents)| McpDeployment {
                domain: Domain(format!("{domain}.example.com")),
                agents,
            })
            .boxed()
    }

    fn arb_mcp_model() -> BoxedStrategy<Mcp> {
        prop::collection::vec(
            (
                arb_ident().prop_map(EnvironmentName),
                prop::collection::vec(arb_mcp_deployment_model(), 0..=2),
            ),
            0..=3,
        )
        .prop_map(|deployments| Mcp {
            deployments: IndexMap::from_iter(deployments),
        })
        .boxed()
    }

    fn arb_bridge_sdk_language_targets() -> BoxedStrategy<BridgeSdkLanguageTargets> {
        (arb_token_list_model(), arb_opt(arb_ident()))
            .prop_map(|(agents, output_dir)| BridgeSdkLanguageTargets { agents, output_dir })
            .boxed()
    }

    fn arb_bridge_sdks_model() -> BoxedStrategy<BridgeSdks> {
        (
            arb_opt(arb_bridge_sdk_language_targets()),
            arb_opt(arb_bridge_sdk_language_targets()),
        )
            .prop_map(|(ts, rust)| BridgeSdks { ts, rust })
            .boxed()
    }

    fn arb_secret_defaults_model()
    -> BoxedStrategy<IndexMap<EnvironmentName, Vec<EnvironmentAgentSecret>>> {
        prop::collection::vec(
            (
                arb_ident().prop_map(EnvironmentName),
                prop::collection::vec(
                    (prop::collection::vec(arb_ident(), 1..=3), arb_json_value())
                        .prop_map(|(path, value)| EnvironmentAgentSecret { path, value }),
                    0..=2,
                ),
            ),
            0..=3,
        )
        .prop_map(IndexMap::from_iter)
        .boxed()
    }

    fn arb_resource_limit_model() -> BoxedStrategy<ResourceLimit> {
        prop_oneof![
            (1u64..=100u64, 1u64..=300u64).prop_map(|(value, max)| {
                ResourceLimit::Rate(ResourceRateLimit {
                    value,
                    period: TimePeriod::Second,
                    max,
                })
            }),
            (1u64..=100u64)
                .prop_map(|value| ResourceLimit::Capacity(ResourceCapacityLimit { value })),
            (1u64..=100u64)
                .prop_map(|value| ResourceLimit::Concurrency(ResourceConcurrencyLimit { value })),
        ]
        .boxed()
    }

    fn arb_resource_defaults_model()
    -> BoxedStrategy<IndexMap<EnvironmentName, Vec<ResourceDefinitionCreation>>> {
        prop::collection::vec(
            (
                arb_ident().prop_map(EnvironmentName),
                prop::collection::vec(
                    (
                        arb_ident(),
                        arb_resource_limit_model(),
                        any::<bool>(),
                        arb_ident(),
                        arb_ident(),
                    )
                        .prop_map(|(name, limit, reject, unit, units)| {
                            ResourceDefinitionCreation {
                                name: ResourceName(name),
                                limit,
                                enforcement_action: if reject {
                                    EnforcementAction::Reject
                                } else {
                                    EnforcementAction::Throttle
                                },
                                unit,
                                units,
                            }
                        }),
                    0..=2,
                ),
            ),
            0..=3,
        )
        .prop_map(IndexMap::from_iter)
        .boxed()
    }

    fn arb_api_predicate_value_model() -> BoxedStrategy<ApiPredicateValue> {
        prop_oneof![
            arb_ident().prop_map(|value| ApiPredicateValue::Text(ApiTextValue { value })),
            any::<i64>().prop_map(|value| ApiPredicateValue::Integer(ApiIntegerValue { value })),
            any::<bool>().prop_map(|value| ApiPredicateValue::Boolean(ApiBooleanValue { value })),
        ]
        .boxed()
    }

    fn arb_api_predicate_model() -> BoxedStrategy<ApiPredicate> {
        let leaf = prop_oneof![
            Just(ApiPredicate::True(ApiPredicateTrue {})),
            Just(ApiPredicate::False(ApiPredicateFalse {})),
            arb_ident().prop_map(|property| {
                ApiPredicate::PropExists(ApiPropertyExistence { property })
            }),
            (arb_ident(), arb_api_predicate_value_model()).prop_map(|(property, value)| {
                ApiPredicate::PropEq(ApiPropertyComparison { property, value })
            }),
            (arb_ident(), arb_api_predicate_value_model()).prop_map(|(property, value)| {
                ApiPredicate::PropNeq(ApiPropertyComparison { property, value })
            }),
            (
                arb_ident(),
                prop::collection::vec(arb_api_predicate_value_model(), 1..=3),
            )
                .prop_map(|(property, values)| {
                    ApiPredicate::PropIn(ApiPropertySetCheck { property, values })
                }),
            (arb_ident(), arb_ident()).prop_map(|(property, pattern)| {
                ApiPredicate::PropMatches(ApiPropertyPattern { property, pattern })
            }),
            (arb_ident(), arb_ident()).prop_map(|(property, prefix)| {
                ApiPredicate::PropStartsWith(ApiPropertyPrefix { property, prefix })
            }),
            (arb_ident(), arb_ident()).prop_map(|(property, substring)| {
                ApiPredicate::PropContains(ApiPropertySubstring {
                    property,
                    substring,
                })
            }),
        ];

        leaf.prop_recursive(3, 48, 2, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                    ApiPredicate::And(ApiPredicatePair {
                        left: Box::new(left),
                        right: Box::new(right),
                    })
                }),
                (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                    ApiPredicate::Or(ApiPredicatePair {
                        left: Box::new(left),
                        right: Box::new(right),
                    })
                }),
                inner.prop_map(|predicate| {
                    ApiPredicate::Not(ApiPredicateNot {
                        predicate: Box::new(predicate),
                    })
                }),
            ]
        })
        .boxed()
    }

    fn arb_api_retry_policy_model() -> BoxedStrategy<ApiRetryPolicy> {
        let leaf = prop_oneof![
            Just(ApiRetryPolicy::Immediate(ApiImmediatePolicy {})),
            Just(ApiRetryPolicy::Never(ApiNeverPolicy {})),
            (0u64..=10000u64)
                .prop_map(|delay_ms| { ApiRetryPolicy::Periodic(ApiPeriodicPolicy { delay_ms }) }),
            (
                (0u64..=10000u64),
                (1u8..=20u8).prop_map(|n| n as f64 / 10.0)
            )
                .prop_map(|(base_delay_ms, factor)| {
                    ApiRetryPolicy::Exponential(ApiExponentialPolicy {
                        base_delay_ms,
                        factor,
                    })
                },),
            ((0u64..=10000u64), (0u64..=10000u64)).prop_map(|(first_ms, second_ms)| {
                ApiRetryPolicy::Fibonacci(ApiFibonacciPolicy {
                    first_ms,
                    second_ms,
                })
            }),
        ];

        leaf.prop_recursive(3, 48, 2, |inner| {
            prop_oneof![
                ((0u32..=50u32), inner.clone()).prop_map(|(max_retries, inner)| {
                    ApiRetryPolicy::CountBox(ApiCountBoxPolicy {
                        max_retries,
                        inner: Box::new(inner),
                    })
                }),
                ((0u64..=100000u64), inner.clone()).prop_map(|(limit_ms, inner)| {
                    ApiRetryPolicy::TimeBox(ApiTimeBoxPolicy {
                        limit_ms,
                        inner: Box::new(inner),
                    })
                }),
                ((0u64..=100000u64), (0u64..=100000u64), inner.clone()).prop_map(
                    |(min_delay_ms, max_delay_ms, inner)| {
                        ApiRetryPolicy::Clamp(ApiClampPolicy {
                            min_delay_ms,
                            max_delay_ms,
                            inner: Box::new(inner),
                        })
                    },
                ),
                ((0u64..=100000u64), inner.clone()).prop_map(|(delay_ms, inner)| {
                    ApiRetryPolicy::AddDelay(ApiAddDelayPolicy {
                        delay_ms,
                        inner: Box::new(inner),
                    })
                }),
                ((1u8..=20u8).prop_map(|n| n as f64 / 10.0), inner.clone()).prop_map(
                    |(factor, inner)| {
                        ApiRetryPolicy::Jitter(ApiJitterPolicy {
                            factor,
                            inner: Box::new(inner),
                        })
                    },
                ),
                (arb_api_predicate_model(), inner.clone()).prop_map(|(predicate, inner)| {
                    ApiRetryPolicy::FilteredOn(ApiFilteredOnPolicy {
                        predicate,
                        inner: Box::new(inner),
                    })
                }),
                (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                    ApiRetryPolicy::AndThen(ApiRetryPolicyPair {
                        first: Box::new(first),
                        second: Box::new(second),
                    })
                }),
                (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                    ApiRetryPolicy::Union(ApiRetryPolicyPair {
                        first: Box::new(first),
                        second: Box::new(second),
                    })
                }),
                (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                    ApiRetryPolicy::Intersect(ApiRetryPolicyPair {
                        first: Box::new(first),
                        second: Box::new(second),
                    })
                }),
            ]
        })
        .boxed()
    }

    fn arb_retry_policy_defaults_model()
    -> BoxedStrategy<IndexMap<EnvironmentName, Vec<EnvironmentRetryPolicyDefault>>> {
        prop::collection::vec(
            (
                arb_ident().prop_map(EnvironmentName),
                prop::collection::vec(
                    (
                        arb_ident(),
                        0u32..=100u32,
                        arb_api_predicate_model(),
                        arb_api_retry_policy_model(),
                    )
                        .prop_map(|(name, priority, predicate, policy)| {
                            EnvironmentRetryPolicyDefault {
                                name,
                                priority,
                                predicate: predicate.into(),
                                policy: policy.into(),
                            }
                        }),
                    0..=2,
                ),
            ),
            0..=3,
        )
        .prop_map(IndexMap::from_iter)
        .boxed()
    }

    fn arb_application_model_v3() -> BoxedStrategy<Application> {
        (
            arb_opt(arb_semver()),
            arb_opt(arb_ident()),
            prop::collection::vec(arb_ident(), 0..=3),
            (
                prop::collection::vec((arb_ident(), arb_component_template_model()), 0..=3)
                    .prop_map(IndexMap::from_iter),
                prop::collection::vec((arb_ident(), arb_component_model()), 0..=3)
                    .prop_map(IndexMap::from_iter),
                prop::collection::vec(
                    (arb_ident().prop_map(AgentTypeName), arb_agent_model()),
                    0..=3,
                )
                .prop_map(IndexMap::from_iter),
                prop::collection::vec(
                    (
                        arb_ident(),
                        prop::collection::vec(arb_external_command_model(), 0..=2),
                    ),
                    0..=3,
                )
                .prop_map(IndexMap::from_iter),
                prop::collection::vec(arb_ident(), 0..=3),
            ),
            (
                arb_opt(arb_http_api_model()),
                arb_opt(arb_mcp_model()),
                prop::collection::vec((arb_ident(), arb_environment_model()), 0..=3)
                    .prop_map(IndexMap::from_iter),
                arb_opt(arb_bridge_sdks_model()),
                arb_secret_defaults_model(),
                arb_retry_policy_defaults_model(),
                arb_resource_defaults_model(),
            ),
        )
            .prop_map(
                |(
                    manifest_version,
                    app,
                    includes,
                    (component_templates, components, agents, custom_commands, clean),
                    (
                        http_api,
                        mcp,
                        environments,
                        bridge,
                        secret_defaults,
                        retry_policy_defaults,
                        resource_defaults,
                    ),
                )| Application {
                    manifest_version,
                    app,
                    includes,
                    component_templates,
                    components,
                    agents,
                    custom_commands,
                    clean,
                    http_api,
                    mcp,
                    environments,
                    bridge,
                    secret_defaults,
                    retry_policy_defaults,
                    resource_defaults,
                },
            )
            .boxed()
    }

    prop_compose! {
        fn arb_application_document()(app in arb_application_model_v3()) -> Application {
            app
        }
    }

    #[test]
    fn schema_is_loadable_and_validates_empty_app() {
        let app = Application {
            app: Some("app-name".to_string()),
            ..Default::default()
        };

        assert!(JSON_SCHEMA_VALIDATOR.is_valid(&serde_json::to_value(&app).unwrap()));
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 400,
            .. ProptestConfig::default()
        })]

        #[test]
        fn proptest_schema_accepts_serialized_application_documents(app in arb_application_document()) {
            let json_str = serde_json::to_string(&app).unwrap();
            let yaml_str = serde_yaml::to_string(&app).unwrap();

            let app_from_json: Application = serde_json::from_str(&json_str).unwrap();
            let app_from_yaml: Application = serde_yaml::from_str(&yaml_str).unwrap();

            let app_from_json_value = serde_json::to_value(&app_from_json).unwrap();
            let app_from_yaml_value = serde_json::to_value(&app_from_yaml).unwrap();
            let app_original_value = serde_json::to_value(&app).unwrap();

            prop_assert_eq!(app_from_json_value.clone(), app_from_yaml_value);
            prop_assert_eq!(app_from_json_value, app_original_value);

            let json_value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            let json_evaluation = JSON_SCHEMA_VALIDATOR.evaluate(&json_value);
            prop_assert!(
                json_evaluation.flag().valid,
                "Schema validation failed for generated app JSON payload: {:?}",
                json_evaluation
                    .iter_errors()
                    .map(|e| format!("{} :: {}", e.instance_location, e.error))
                    .collect::<Vec<_>>()
            );

            let yaml_value: serde_json::Value = serde_yaml::from_str(&yaml_str).unwrap();
            let yaml_evaluation = JSON_SCHEMA_VALIDATOR.evaluate(&yaml_value);
            prop_assert!(
                yaml_evaluation.flag().valid,
                "Schema validation failed for generated app YAML payload: {:?}",
                yaml_evaluation
                    .iter_errors()
                    .map(|e| format!("{} :: {}", e.instance_location, e.error))
                    .collect::<Vec<_>>()
            );
        }
    }
}
