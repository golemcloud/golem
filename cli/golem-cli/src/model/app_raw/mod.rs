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

use crate::bridge_gen::BridgeMode;
use crate::log::LogColorize;
use crate::model::cascade::property::map::MapMergeMode;
use crate::model::cascade::property::vec::VecMergeMode;
use crate::model::format::Format;
use crate::model::language::GuestLanguage;
use crate::{APP_MANIFEST_JSON_SCHEMA, fs};
use anyhow::{Context, anyhow};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath, ComponentName};
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::quota::{EnforcementAction, ResourceLimit, ResourceName};
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::model::tool::{ToolFilesystemAccess, ToolName};
use golem_common::model::tool_middleware::{ToolMiddlewareMergeMode, ToolMiddlewareName};
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
    #[serde(default, skip_serializing_if = "ToolDeclarations::is_empty")]
    pub tools: ToolDeclarations,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub custom_commands: IndexMap<String, Vec<ExternalCommand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_api: Option<HttpApi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<Mcp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_server: Option<LocalServer>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub environments: IndexMap<String, Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<AppVersionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge: Option<BridgeSdks>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub secret_defaults: IndexMap<EnvironmentName, JsonObject>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub retry_policy_defaults: IndexMap<EnvironmentName, IndexMap<String, EnvironmentRetryPolicy>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub resource_defaults: IndexMap<EnvironmentName, IndexMap<ResourceName, ResourceDefinition>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub tool_releases: IndexMap<EnvironmentName, IndexMap<String, PublishTool>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub tool_middleware_releases: IndexMap<EnvironmentName, IndexMap<String, PublishTool>>,
}

pub type JsonObject = serde_json::Map<String, serde_json::Value>;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(transparent)]
pub struct ToolDeclarations(IndexMap<String, serde_json::Value>);

impl ToolDeclarations {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_entries(self) -> impl Iterator<Item = (String, serde_json::Value)> {
        self.0.into_iter()
    }

    pub fn into_tools_and_middleware(
        mut self,
    ) -> (
        IndexMap<String, serde_json::Value>,
        Option<serde_json::Value>,
    ) {
        let middleware = self.0.shift_remove("middleware");
        (self.0, middleware)
    }
}

impl<'de> Deserialize<'de> for ToolDeclarations {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let declarations = IndexMap::<String, serde_json::Value>::deserialize(deserializer)?;
        if let Some(serde_json::Value::Object(value)) = declarations.get("middleware")
            && [
                "component",
                "release",
                "templates",
                "config",
                "envMergeMode",
                "env",
                "pluginsMergeMode",
                "plugins",
                "filesMergeMode",
                "files",
                "presets",
            ]
            .iter()
            .any(|field| value.contains_key(*field))
        {
            return Err(serde::de::Error::custom(
                "tool name `middleware` is reserved for tool middleware declarations",
            ));
        }
        Ok(Self(declarations))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<RegistrySubject>,
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ToolPreset>,
}

impl ToolDeclaration {
    pub fn tool_layer_properties(&self) -> ToolLayerProperties {
        ToolLayerProperties {
            config: self.config.clone(),
            env_merge_mode: self.env_merge_mode,
            env: self.env.clone(),
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins.clone(),
            files_merge_mode: self.files_merge_mode,
            files: self.files.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(untagged)]
pub enum RegistrySubject {
    ById(RegistrySubjectById),
    ByCoordinates(RegistrySubjectByCoordinates),
}

impl<'de> Deserialize<'de> for RegistrySubject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let fields = value
            .as_object()
            .ok_or_else(|| {
                serde::de::Error::custom(
                    "tool release reference must be an object containing either `releaseId`, or `account`, `name`, and `version`",
                )
            })?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();

        let has_release_id = fields.contains("releaseId");
        let has_coordinates = ["account", "name", "version"]
            .iter()
            .any(|field| fields.contains(*field));

        match (has_release_id, has_coordinates) {
            (true, false) => serde_json::from_value::<RegistrySubjectById>(value)
                .map(Self::ById)
                .map_err(|error| {
                    serde::de::Error::custom(format!(
                        "invalid release ID reference; expected only `releaseId`: {error}"
                    ))
                }),
            (false, true) => serde_json::from_value::<RegistrySubjectByCoordinates>(value)
                .map(Self::ByCoordinates)
                .map_err(|error| {
                    serde::de::Error::custom(format!(
                        "invalid release coordinate reference; expected `account`, `name`, and `version`: {error}"
                    ))
                }),
            (true, true) => Err(serde::de::Error::custom(
                "tool release reference cannot combine `releaseId` with `account`, `name`, or `version`",
            )),
            (false, false) => Err(serde::de::Error::custom(format!(
                "tool release reference must contain either `releaseId`, or `account`, `name`, and `version`; found fields: {}",
                fields.into_iter().collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

impl RegistrySubject {
    pub fn to_release_reference(
        &self,
    ) -> Result<golem_common::model::tool_release::ToolReleaseReference, String> {
        use golem_common::model::account::AccountEmail;
        use golem_common::model::tool::ToolName;
        use golem_common::model::tool_release::{
            ToolReleaseByCoordinates, ToolReleaseById, ToolReleaseReference,
        };

        match self {
            Self::ById(reference) => Ok(ToolReleaseReference::ById(ToolReleaseById {
                release_id: reference.release_id,
            })),
            Self::ByCoordinates(reference) => Ok(ToolReleaseReference::ByCoordinates(
                ToolReleaseByCoordinates {
                    account: AccountEmail::new(reference.account.clone()),
                    name: ToolName::try_from(reference.name.as_str())?,
                    version: reference.version.clone(),
                },
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrySubjectById {
    pub release_id: golem_common::model::tool_release::ToolReleaseId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolMiddlewareRegistrySubjectById {
    pub release_id: golem_common::model::tool_middleware_release::ToolMiddlewareReleaseId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(untagged)]
pub enum ToolMiddlewareRegistrySubject {
    ById(ToolMiddlewareRegistrySubjectById),
    ByCoordinates(RegistrySubjectByCoordinates),
}

impl<'de> Deserialize<'de> for ToolMiddlewareRegistrySubject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Subject {
            ById(ToolMiddlewareRegistrySubjectById),
            ByCoordinates(RegistrySubjectByCoordinates),
        }
        match Subject::deserialize(deserializer)? {
            Subject::ById(value) => Ok(Self::ById(value)),
            Subject::ByCoordinates(value) => Ok(Self::ByCoordinates(value)),
        }
    }
}

impl ToolMiddlewareRegistrySubject {
    pub fn to_release_reference(
        &self,
    ) -> Result<golem_common::model::tool_middleware_release::ToolMiddlewareReleaseReference, String>
    {
        use golem_common::model::account::AccountEmail;
        use golem_common::model::tool_middleware_release::{
            ToolMiddlewareReleaseByCoordinates, ToolMiddlewareReleaseById,
            ToolMiddlewareReleaseReference,
        };
        match self {
            Self::ById(value) => Ok(ToolMiddlewareReleaseReference::ById(
                ToolMiddlewareReleaseById {
                    release_id: value.release_id,
                },
            )),
            Self::ByCoordinates(value) => Ok(ToolMiddlewareReleaseReference::ByCoordinates(
                ToolMiddlewareReleaseByCoordinates {
                    account: AccountEmail::new(value.account.clone()),
                    name: ToolMiddlewareName::try_from(value.name.as_str())?,
                    version: value.version.clone(),
                },
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolMiddlewareDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ToolMiddlewareRegistrySubject>,
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ToolPreset>,
}

impl ToolMiddlewareDeclaration {
    pub fn tool_layer_properties(&self) -> ToolLayerProperties {
        ToolLayerProperties {
            config: self.config.clone(),
            env_merge_mode: self.env_merge_mode,
            env: self.env.clone(),
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins.clone(),
            files_merge_mode: self.files_merge_mode,
            files: self.files.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrySubjectByCoordinates {
    pub account: String,
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPreset {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Marker>,
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

impl ToolPreset {
    pub fn into_tool_layer_properties(self) -> ToolLayerProperties {
        ToolLayerProperties {
            config: self.config,
            env_merge_mode: self.env_merge_mode,
            env: self.env,
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins,
            files_merge_mode: self.files_merge_mode,
            files: self.files,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolLayerProperties {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretKeyMergeMode {
    #[default]
    Intersect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ManifestSecretKeyScope {
    All(String),
    Keys(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ManifestConfigKeyScope {
    All(String),
    Keys(Vec<String>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawManifestKeyScope {
    All(String),
    Keys(Vec<String>),
}

fn deserialize_manifest_key_scope<'de, D>(
    deserializer: D,
    kind: &str,
) -> Result<RawManifestKeyScope, D::Error>
where
    D: Deserializer<'de>,
{
    let scope = RawManifestKeyScope::deserialize(deserializer)?;
    match &scope {
        RawManifestKeyScope::All(value) if value == "*" => {}
        RawManifestKeyScope::All(value) => {
            return Err(serde::de::Error::custom(format!(
                "expected '*' or a list of {kind} paths, found '{value}'"
            )));
        }
        RawManifestKeyScope::Keys(paths) => {
            for path in paths {
                if path == "*" {
                    return Err(serde::de::Error::custom(format!(
                        "'*' must be used as the whole {kind} scope, not as a list entry"
                    )));
                }
                crate::args::parse_agent_config_path(path).map_err(|error| {
                    serde::de::Error::custom(format!("invalid {kind} path '{path}': {error}"))
                })?;
            }
        }
    }
    Ok(scope)
}

impl<'de> Deserialize<'de> for ManifestSecretKeyScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_manifest_key_scope(deserializer, "secret").map(|scope| match scope {
            RawManifestKeyScope::All(value) => Self::All(value),
            RawManifestKeyScope::Keys(paths) => Self::Keys(paths),
        })
    }
}

impl<'de> Deserialize<'de> for ManifestConfigKeyScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_manifest_key_scope(deserializer, "config").map(|scope| match scope {
            RawManifestKeyScope::All(value) => Self::All(value),
            RawManifestKeyScope::Keys(paths) => Self::Keys(paths),
        })
    }
}

fn deserialize_tool_bindings<'de, D>(
    deserializer: D,
) -> Result<Option<IndexMap<String, ToolBinding>>, D::Error>
where
    D: Deserializer<'de>,
{
    let bindings = Option::<IndexMap<String, ToolBinding>>::deserialize(deserializer)?;
    if let Some(bindings) = &bindings {
        for name in bindings.keys() {
            ToolName::try_from(name.as_str()).map_err(serde::de::Error::custom)?;
        }
    }
    Ok(bindings)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<IndexMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_keys_readable_merge_mode: Option<SecretKeyMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_keys_readable: Option<ManifestConfigKeyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_keys_readable_merge_mode: Option<SecretKeyMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_keys_readable: Option<ManifestSecretKeyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_keys_revealable_merge_mode: Option<SecretKeyMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_keys_revealable: Option<ManifestSecretKeyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_access: Option<ToolFilesystemAccess>,
    /// `None` distinguishes omission from an explicitly authored empty chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub middleware: Option<Vec<ToolMiddlewareInstallation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub middleware_merge_mode: Option<ToolMiddlewareMergeMode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolMiddlewareInstallation {
    Shortcut(String),
    Structured(ToolMiddlewareInstallationStruct),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolMiddlewareInstallationStruct {
    pub name: ToolMiddlewareName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default = "empty_normalized_json")]
    pub parameters: NormalizedJsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default)]
    pub filesystem_access: ToolFilesystemAccess,
}

fn empty_normalized_json() -> NormalizedJsonValue {
    NormalizedJsonValue::new(serde_json::json!({}))
}

impl ToolMiddlewareInstallation {
    pub fn into_common(
        self,
    ) -> Result<golem_common::model::tool_middleware::ToolMiddlewareInstallation, String> {
        use golem_common::model::account::AccountEmail;
        let value = match self {
            Self::Structured(value) => value,
            Self::Shortcut(shortcut) => {
                let (name, version) = shortcut
                    .rsplit_once('@')
                    .map_or((shortcut.as_str(), None), |(name, version)| {
                        (name, Some(version.to_string()))
                    });
                ToolMiddlewareInstallationStruct {
                    name: ToolMiddlewareName::try_from(name)?,
                    version,
                    parameters: empty_normalized_json(),
                    account: None,
                    filesystem_access: ToolFilesystemAccess::Unset,
                }
            }
        };
        Ok(
            golem_common::model::tool_middleware::ToolMiddlewareInstallation {
                name: value.name,
                version: value.version,
                parameters: value.parameters,
                account: value.account.map(AccountEmail::new),
                filesystem_access: value.filesystem_access,
            },
        )
    }
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentDependencies {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<ComponentDependencyReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ComponentDependencyReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComponentDependencyReference {
    Shortcut(String),
    Structured(ComponentDependencyReferenceStruct),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentDependencyReferenceStruct {
    pub component: String,
    pub name: String,
}

impl ComponentDependencies {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.tools.is_empty()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentTemplate {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub templates: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_wasm: Option<String>,
    #[serde(default, skip_serializing_if = "ComponentDependencies::is_empty")]
    pub dependencies: ComponentDependencies,
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
    pub initial_card: Option<ManifestInitialCard>,
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentPreset>,
}

impl ComponentTemplate {
    pub fn component_layer_properties(&self) -> ComponentLayerProperties {
        ComponentLayerProperties {
            component_wasm: self.component_wasm.clone(),
            output_wasm: self.output_wasm.clone(),
            dependencies: self.dependencies.clone(),
            build_merge_mode: self.build_merge_mode,
            build: self.build.clone(),
            custom_commands: self.custom_commands.clone(),
            clean: self.clean.clone(),
            agent_properties: AgentLayerProperties {
                config: self.config.clone(),
                initial_card: self.initial_card.clone(),
                env_merge_mode: self.env_merge_mode,
                env: self.env.clone(),
                plugins_merge_mode: self.plugins_merge_mode,
                plugins: self.plugins.clone(),
                files_merge_mode: self.files_merge_mode,
                files: self.files.clone(),
                tools_merge_mode: None,
                tools: None,
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
    #[serde(default, skip_serializing_if = "ComponentDependencies::is_empty")]
    pub dependencies: ComponentDependencies,
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
    pub initial_card: Option<ManifestInitialCard>,
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
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, ComponentPreset>,
}

impl Component {
    pub fn component_layer_properties(&self) -> ComponentLayerProperties {
        ComponentLayerProperties {
            component_wasm: self.component_wasm.clone(),
            output_wasm: self.output_wasm.clone(),
            dependencies: self.dependencies.clone(),
            build_merge_mode: self.build_merge_mode,
            build: self.build.clone(),
            custom_commands: self.custom_commands.clone(),
            clean: self.clean.clone(),
            agent_properties: AgentLayerProperties {
                config: self.config.clone(),
                initial_card: self.initial_card.clone(),
                env_merge_mode: self.env_merge_mode,
                env: self.env.clone(),
                plugins_merge_mode: self.plugins_merge_mode,
                plugins: self.plugins.clone(),
                files_merge_mode: self.files_merge_mode,
                files: self.files.clone(),
                tools_merge_mode: None,
                tools: None,
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
    #[serde(default, skip_serializing_if = "ComponentDependencies::is_empty")]
    pub dependencies: ComponentDependencies,
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
    pub initial_card: Option<ManifestInitialCard>,
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

impl ComponentPreset {
    pub fn into_component_layer_properties(self) -> ComponentLayerProperties {
        ComponentLayerProperties {
            component_wasm: self.component_wasm,
            output_wasm: self.output_wasm,
            dependencies: self.dependencies,
            build_merge_mode: self.build_merge_mode,
            build: self.build,
            custom_commands: self.custom_commands,
            clean: self.clean,
            agent_properties: AgentLayerProperties {
                config: self.config,
                initial_card: self.initial_card,
                env_merge_mode: self.env_merge_mode,
                env: self.env,
                plugins_merge_mode: self.plugins_merge_mode,
                plugins: self.plugins,
                files_merge_mode: self.files_merge_mode,
                files: self.files,
                tools_merge_mode: None,
                tools: None,
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
    pub initial_card: Option<ManifestInitialCard>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_tool_bindings")]
    pub tools: Option<IndexMap<String, ToolBinding>>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub presets: IndexMap<String, AgentPreset>,
}

impl Agent {
    pub fn agent_layer_properties(&self) -> AgentLayerProperties {
        AgentLayerProperties {
            config: self.config.clone(),
            initial_card: self.initial_card.clone(),
            env_merge_mode: self.env_merge_mode,
            env: self.env.clone(),
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins.clone(),
            files_merge_mode: self.files_merge_mode,
            files: self.files.clone(),
            tools_merge_mode: self.tools_merge_mode,
            tools: self.tools.clone(),
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
    pub initial_card: Option<ManifestInitialCard>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_tool_bindings")]
    pub tools: Option<IndexMap<String, ToolBinding>>,
}

impl AgentPreset {
    pub fn into_agent_layer_properties(self) -> AgentLayerProperties {
        AgentLayerProperties {
            config: self.config,
            initial_card: self.initial_card,
            env_merge_mode: self.env_merge_mode,
            env: self.env,
            plugins_merge_mode: self.plugins_merge_mode,
            plugins: self.plugins,
            files_merge_mode: self.files_merge_mode,
            files: self.files,
            tools_merge_mode: self.tools_merge_mode,
            tools: self.tools,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentRetryPolicy {
    pub priority: u32,
    pub predicate: golem_common::model::retry_policy::Predicate,
    pub policy: golem_common::model::retry_policy::RetryPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDefinition {
    pub limit: ResourceLimit,
    pub enforcement_action: EnforcementAction,
    pub unit: String,
    pub units: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<DeploymentDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<DeploymentSubdomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi_endpoint: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<DeploymentDomain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<DeploymentSubdomain>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<AgentTypeName, McpDeploymentAgentOptions>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpDeploymentAgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeploymentDomain(pub String);

impl From<Domain> for DeploymentDomain {
    fn from(value: Domain) -> Self {
        Self(value.0)
    }
}

impl DeploymentDomain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeploymentSubdomain(pub String);

impl DeploymentSubdomain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalServer {
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "golem_common::config::byte_size::optional"
    )]
    pub system_memory_override: Option<std::num::NonZeroU64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub router_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub router_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub custom_request_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mcp_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ports_file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_filesystem_root: Option<PathBuf>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<AppVersionSourceOverride>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tools: Option<EnvironmentTools>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentTools {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub middleware: Vec<ToolMiddlewareInstallation>,
    #[serde(flatten)]
    pub bindings: IndexMap<String, ToolBinding>,
}

impl<'de> Deserialize<'de> for EnvironmentTools {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = IndexMap::<String, serde_json::Value>::deserialize(deserializer)?;
        let mut middleware = Vec::new();
        let mut bindings = IndexMap::new();
        for (name, value) in value {
            if name == "middleware" {
                middleware = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            } else {
                let binding: ToolBinding =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                if binding.middleware_merge_mode.is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "middlewareMergeMode is only valid on agent tool bindings, not environment tool binding `{name}`"
                    )));
                }
                bindings.insert(name, binding);
            }
        }
        Ok(Self {
            middleware,
            bindings,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishTool {}

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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compatibility_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_compatibility_mode:
        Option<golem_common::schema::tool::compatibility::ToolCompatibilityMode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub security_overrides: Option<bool>,
}

impl DeploymentOptions {
    pub fn new_local() -> Self {
        Self {
            compatibility_check: Some(false),
            tool_compatibility_mode: None,
            version_check: Some(false),
            security_overrides: Some(true),
        }
    }

    pub fn new_cloud() -> Self {
        Self {
            compatibility_check: None,
            tool_compatibility_mode: None,
            version_check: None,
            security_overrides: None,
        }
    }

    pub fn with_defaults_from(mut self, other: Self) -> Self {
        if self.compatibility_check.is_none() {
            self.compatibility_check = other.compatibility_check;
        }
        if self.tool_compatibility_mode.is_none() {
            self.tool_compatibility_mode = other.tool_compatibility_mode;
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

    pub fn tool_compatibility_mode(
        &self,
    ) -> golem_common::schema::tool::compatibility::ToolCompatibilityMode {
        self.tool_compatibility_mode.unwrap_or_default()
    }

    pub fn version_check(&self) -> bool {
        self.version_check.unwrap_or(false)
    }

    pub fn security_overrides(&self) -> bool {
        self.security_overrides.unwrap_or(false)
    }

    pub fn to_diffable(&self) -> diff::Environment {
        diff::Environment {
            compatibility_check: self.compatibility_check(),
            tool_compatibility_mode: self.tool_compatibility_mode(),
            version_check: self.version_check(),
            security_overrides: self.security_overrides(),
        }
    }
}

/// How the logical version attached to a deployment is computed. Orthogonal to
/// [`DeploymentOptions::version_check`], which governs uniqueness of that string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AppVersionSource {
    Static(String),
    Git { git: GitVersionSource },
    Env { env: String },
}

/// Hash mode (`hashOnly`) or tag mode (`tagPattern`); mutually exclusive by shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GitVersionSource {
    Hash(GitHashVersionSource),
    Tag(GitTagVersionSource),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHashVersionSource {
    pub hash_only: Marker,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allow_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub static_fallback: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitTagVersionSource {
    pub tag_pattern: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commit_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hash_fallback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allow_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub static_fallback: Option<String>,
}

/// Partial per-environment override, layered over the application-wide `version`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AppVersionSourceOverride {
    Static(String),
    Git { git: GitVersionSourceOverride },
    Env { env: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GitVersionSourceOverride {
    Hash(GitHashVersionSource),
    Tag(GitTagVersionSourceOverride),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitTagVersionSourceOverride {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commit_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hash_fallback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allow_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub static_fallback: Option<String>,
}

const TAG_PATTERN_REQUIRED: &str = "`tagPattern` is required for git tag mode; set it on the environment override or the application-wide `version`";

impl AppVersionSourceOverride {
    /// Resolve this override against the optional application-wide root.
    pub fn resolve_over(self, root: Option<AppVersionSource>) -> Result<AppVersionSource, String> {
        match root {
            Some(root) => self.merge_over(root),
            None => self.into_source(),
        }
    }

    fn merge_over(self, root: AppVersionSource) -> Result<AppVersionSource, String> {
        match (self, root) {
            (AppVersionSourceOverride::Git { git: over }, AppVersionSource::Git { git: root }) => {
                Ok(AppVersionSource::Git {
                    git: over.merge_over(root)?,
                })
            }
            // The override selects a different top-level source: it replaces.
            (over, _) => over.into_source(),
        }
    }

    fn into_source(self) -> Result<AppVersionSource, String> {
        match self {
            AppVersionSourceOverride::Git { git } => Ok(AppVersionSource::Git {
                git: git.into_source()?,
            }),
            AppVersionSourceOverride::Static(value) => Ok(AppVersionSource::Static(value)),
            AppVersionSourceOverride::Env { env } => Ok(AppVersionSource::Env { env }),
        }
    }
}

impl GitVersionSourceOverride {
    fn merge_over(self, root: GitVersionSource) -> Result<GitVersionSource, String> {
        match (self, root) {
            (GitVersionSourceOverride::Tag(over), GitVersionSource::Tag(root)) => {
                Ok(GitVersionSource::Tag(GitTagVersionSource {
                    tag_pattern: over.tag_pattern.unwrap_or(root.tag_pattern),
                    commit_info: over.commit_info.or(root.commit_info),
                    hash_fallback: over.hash_fallback.or(root.hash_fallback),
                    allow_dirty: over.allow_dirty.or(root.allow_dirty),
                    static_fallback: over.static_fallback.or(root.static_fallback),
                }))
            }
            (GitVersionSourceOverride::Hash(over), GitVersionSource::Hash(root)) => {
                Ok(GitVersionSource::Hash(GitHashVersionSource {
                    hash_only: Marker,
                    allow_dirty: over.allow_dirty.or(root.allow_dirty),
                    static_fallback: over.static_fallback.or(root.static_fallback),
                }))
            }
            // Mode switch: the override must stand on its own.
            (over, _) => over.into_source(),
        }
    }

    fn into_source(self) -> Result<GitVersionSource, String> {
        match self {
            GitVersionSourceOverride::Hash(hash) => Ok(GitVersionSource::Hash(hash)),
            GitVersionSourceOverride::Tag(tag) => match tag.tag_pattern {
                Some(tag_pattern) => Ok(GitVersionSource::Tag(GitTagVersionSource {
                    tag_pattern,
                    commit_info: tag.commit_info,
                    hash_fallback: tag.hash_fallback,
                    allow_dirty: tag.allow_dirty,
                    static_fallback: tag.static_fallback,
                })),
                None => Err(TAG_PATTERN_REQUIRED.to_string()),
            },
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestInitialCard {
    #[serde(default)]
    pub lower_bound: ManifestInitialCardBound,
    #[serde(default)]
    pub upper_bound: ManifestInitialCardBound,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestInitialCardBound {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positive: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub negative: Vec<String>,
}

// Common component-level fields merged from templates/components/presets.
// This helper intentionally stays outside serde parsing: using flatten here would weaken
// strict unknown-field checks on manifest-facing structs that use deny_unknown_fields.
#[derive(Clone, Debug)]
pub struct ComponentLayerProperties {
    pub component_wasm: Option<String>,
    pub output_wasm: Option<String>,
    pub dependencies: ComponentDependencies,
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
    pub initial_card: Option<ManifestInitialCard>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_merge_mode: Option<MapMergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<IndexMap<String, ToolBinding>>,
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
    pub effect: Option<BridgeSdkLanguageTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<BridgeSdkLanguageTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scala: Option<BridgeSdkLanguageTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moonbit: Option<BridgeSdkLanguageTargets>,
}

impl BridgeSdks {
    pub fn for_language(&self, language: GuestLanguage) -> Option<&BridgeSdkLanguageTargets> {
        match language {
            GuestLanguage::Rust => self.rust.as_ref(),
            GuestLanguage::TypeScript => self.ts.as_ref(),
            GuestLanguage::Effect => self.effect.as_ref(),
            GuestLanguage::Scala => self.scala.as_ref(),
            GuestLanguage::MoonBit => self.moonbit.as_ref(),
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
            targets.and_then(|targets| {
                (targets
                    .external
                    .as_ref()
                    .is_some_and(|external| !external.agents.is_empty())
                    || targets
                        .internal
                        .as_ref()
                        .is_some_and(|guest| !guest.agents.is_empty() || !guest.tools.is_empty()))
                .then_some((lang, targets))
            })
        })
    }

    pub fn for_all_used_modes(
        &self,
    ) -> Vec<(GuestLanguage, BridgeMode, BridgeSdkInternalTargetsRef<'_>)> {
        let mut result = Vec::new();
        for (language, targets) in self.for_all_languages() {
            if let Some(targets) = targets {
                if let Some(external) = &targets.external
                    && !external.agents.is_empty()
                {
                    result.push((
                        language,
                        BridgeMode::External,
                        BridgeSdkInternalTargetsRef {
                            agents: &external.agents,
                            tools: None,
                            output_dir: external.output_dir.as_ref(),
                        },
                    ));
                }
                if let Some(guest) = &targets.internal
                    && (!guest.agents.is_empty() || !guest.tools.is_empty())
                {
                    result.push((
                        language,
                        BridgeMode::Guest,
                        BridgeSdkInternalTargetsRef {
                            agents: &guest.agents,
                            tools: Some(&guest.tools),
                            output_dir: guest.output_dir.as_ref(),
                        },
                    ));
                }
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdkLanguageTargets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<BridgeSdkExternalTargets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal: Option<BridgeSdkInternalTargets>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdkExternalTargets {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub agents: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeSdkInternalTargets {
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub agents: LenientTokenList,
    #[serde(default, skip_serializing_if = "LenientTokenList::is_empty")]
    pub tools: LenientTokenList,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct BridgeSdkInternalTargetsRef<'a> {
    pub agents: &'a LenientTokenList,
    pub tools: Option<&'a LenientTokenList>,
    pub output_dir: Option<&'a String>,
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
mod tests;
