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

use crate::agent_id_display::SourceLanguage;
use crate::command::shared_args::StreamArgs;
use crate::model::component::ComponentNameMatchKind;
use crate::model::environment::{
    EnvironmentReference, ResolvedEnvironmentIdentity, ResolvedEnvironmentIdentitySource,
};
use crate::model::masking::{Masked, MaskingConfig, mask_agent_config_entries, mask_sensitive_map};
use clap::ValueEnum;
use clap_verbosity_flag::Verbosity;
use colored::control::SHOULD_COLORIZE;
use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, LegacyParsedAgentId};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::worker::{AgentConfigEntryDto, UpdateRecord};
use golem_common::model::{AgentId, AgentResourceDescription, AgentStatus, Timestamp};
use serde_derive::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
// TODO: move things to model/agent

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RawAgentId(pub String);

impl From<&str> for RawAgentId {
    fn from(name: &str) -> Self {
        RawAgentId(name.to_string())
    }
}

impl From<String> for RawAgentId {
    fn from(name: String) -> Self {
        RawAgentId(name)
    }
}

impl Display for RawAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum AgentUpdateMode {
    #[value(alias = "auto")]
    Automatic,
    Manual,
}

impl Display for AgentUpdateMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentUpdateMode::Automatic => {
                write!(f, "auto")
            }
            AgentUpdateMode::Manual => {
                write!(f, "manual")
            }
        }
    }
}

impl FromStr for AgentUpdateMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(AgentUpdateMode::Automatic),
            "manual" => Ok(AgentUpdateMode::Manual),
            _ => Err(format!(
                "Unknown agent update mode: {s}. Expected one of \"auto\", \"manual\""
            )),
        }
    }
}

/// Mode selector for the `agent list` CLI command.
///
/// The default `Durable` excludes ephemeral agents from the listing so that
/// short-lived ephemeral agents from previous runs do not clutter the default
/// output. Use `Ephemeral` to list only ephemeral agents, or `All` to list
/// agents in both modes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum AgentListMode {
    Durable,
    Ephemeral,
    All,
}

impl Display for AgentListMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentListMode::Durable => write!(f, "durable"),
            AgentListMode::Ephemeral => write!(f, "ephemeral"),
            AgentListMode::All => write!(f, "all"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetadataView {
    pub component_name: ComponentName,
    pub agent_name: RawAgentId,
    pub created_by: AccountId,
    pub environment_id: EnvironmentId,
    pub env: HashMap<String, String>,
    pub default_env: HashMap<String, String>,
    pub config: Vec<AgentConfigEntryDto>,
    pub default_config: Vec<AgentConfigEntryDto>,
    pub status: AgentStatus,
    pub component_revision: ComponentRevision,
    pub retry_count: u32,

    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub exported_resource_instances: HashMap<String, AgentResourceDescription>,
    #[serde(skip)]
    pub source_language: SourceLanguage,
    #[serde(skip)]
    pub secret_config_paths: BTreeSet<String>,
}

impl From<AgentMetadata> for AgentMetadataView {
    fn from(value: AgentMetadata) -> Self {
        AgentMetadataView {
            component_name: value.component_name,
            agent_name: value.agent_id.agent_id.into(),
            created_by: value.created_by,
            environment_id: value.environment_id,
            env: value.env,
            default_env: HashMap::new(),
            config: value.config,
            default_config: Vec::new(),
            status: value.status,
            component_revision: value.component_revision,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates,
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            exported_resource_instances: value.exported_resource_instances,
            source_language: SourceLanguage::default(),
            secret_config_paths: BTreeSet::new(),
        }
    }
}

impl AgentMetadataView {
    pub fn with_defaults(mut self, defaults: AgentTypeProvisionConfig) -> Self {
        self.default_env = defaults.env.into_iter().collect();
        self.default_config = defaults.config.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_source_language(mut self, source_language: SourceLanguage) -> Self {
        self.source_language = source_language;
        self
    }

    pub fn with_secret_config_paths(mut self, secret_config_paths: BTreeSet<String>) -> Self {
        self.secret_config_paths = secret_config_paths;
        self
    }
}

impl Masked for AgentMetadataView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        self.env = mask_sensitive_map(config, &self.env);
        self.default_env = mask_sensitive_map(config, &self.default_env);
        self.config = mask_agent_config_entries(config, &self.config, &self.secret_config_paths);
        self.default_config =
            mask_agent_config_entries(config, &self.default_config, &self.secret_config_paths);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMetadata {
    pub agent_id: AgentId,
    pub component_name: ComponentName,
    pub environment_id: EnvironmentId,
    pub created_by: AccountId,
    pub env: HashMap<String, String>,
    pub config: Vec<AgentConfigEntryDto>,
    pub status: AgentStatus,
    pub component_revision: ComponentRevision,
    pub retry_count: u32,
    pub pending_invocation_count: u64,
    pub updates: Vec<UpdateRecord>,
    pub created_at: Timestamp,
    pub last_error: Option<String>,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub exported_resource_instances: HashMap<String, AgentResourceDescription>,
}

impl AgentMetadata {
    pub fn from(
        component_name: ComponentName,
        value: golem_client::model::AgentMetadataDto,
    ) -> Self {
        AgentMetadata {
            agent_id: value.agent_id,
            component_name,
            created_by: value.created_by,
            environment_id: value.environment_id,
            env: value.env,
            config: value.config.into_iter().map(Into::into).collect(),
            status: value.status,
            component_revision: value.component_revision,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates,
            created_at: value.created_at,
            last_error: value.last_error,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            exported_resource_instances: HashMap::from_iter(
                value
                    .exported_resource_instances
                    .into_iter()
                    .map(|desc| (desc.key.to_string(), desc.description)),
            ),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsMetadataResponseView {
    pub agents: Vec<AgentMetadataView>,
    pub cursors: BTreeMap<String, String>,
}

impl Masked for AgentsMetadataResponseView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        self.agents = self
            .agents
            .into_iter()
            .map(|agent| agent.masked(config))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(self)
    }
}

pub trait HasVerbosity {
    fn verbosity(&self) -> Verbosity;
}

#[derive(Debug, Clone)]
pub struct AgentLogStreamOptions {
    pub colors: bool,
    pub show_timestamp: bool,
    pub show_level: bool,
    /// Only show entries coming from the agent, no output about invocation markers and stream status
    pub logs_only: bool,
}

impl From<StreamArgs> for AgentLogStreamOptions {
    fn from(args: StreamArgs) -> Self {
        AgentLogStreamOptions {
            colors: SHOULD_COLORIZE.should_colorize(),
            show_timestamp: !args.stream_no_timestamp,
            show_level: !args.stream_no_log_level,
            logs_only: args.logs_only,
        }
    }
}

pub struct AgentNameMatch {
    pub environment: ResolvedEnvironmentIdentity,
    pub component_name_match_kind: ComponentNameMatchKind,
    pub component_name: ComponentName,
    pub agent_type_name: AgentTypeName,
    pub agent_name: RawAgentId,
    pub source_language: SourceLanguage,
    pub parsed_agent_id: Option<LegacyParsedAgentId>,
}

impl AgentNameMatch {
    pub fn environment_reference(&self) -> Option<&EnvironmentReference> {
        match &self.environment.source {
            ResolvedEnvironmentIdentitySource::Reference(reference) => Some(reference),
            ResolvedEnvironmentIdentitySource::DefaultFromManifest => None,
        }
    }

    /// Updates the canonical agent_name and the parsed form together. Use this
    /// after re-canonicalizing or normalizing the agent id so that downstream
    /// display code can use the language-specific renderer.
    pub fn with_canonical_and_parsed(
        mut self,
        agent_name: RawAgentId,
        parsed_agent_id: Option<LegacyParsedAgentId>,
    ) -> Self {
        self.agent_name = agent_name;
        self.parsed_agent_id = parsed_agent_id;
        self
    }
}
