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
use clap::ValueEnum;
use clap_verbosity_flag::Verbosity;
use colored::control::SHOULD_COLORIZE;
use golem_common::model::account::AccountId;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::worker::{UpdateRecord, AgentConfigEntryDto as AgentConfigEntryDto};
use golem_common::model::{AgentId, AgentResourceDescription, AgentStatus, Timestamp};
use serde_derive::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetadataView {
    pub component_name: ComponentName,
    pub agent_name: RawAgentId,
    pub created_by: AccountId,
    pub environment_id: EnvironmentId,
    pub env: HashMap<String, String>,
    pub wasi_config: BTreeMap<String, String>,
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
}

impl From<AgentMetadata> for AgentMetadataView {
    fn from(value: AgentMetadata) -> Self {
        AgentMetadataView {
            component_name: value.component_name,
            agent_name: value.agent_id.agent_id.into(),
            created_by: value.created_by,
            environment_id: value.environment_id,
            env: value.env,
            wasi_config: value.wasi_config,
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
        }
    }
}

impl AgentMetadataView {
    pub fn with_source_language(mut self, source_language: SourceLanguage) -> Self {
        self.source_language = source_language;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMetadata {
    pub agent_id: AgentId,
    pub component_name: ComponentName,
    pub environment_id: EnvironmentId,
    pub created_by: AccountId,
    pub env: HashMap<String, String>,
    pub wasi_config: BTreeMap<String, String>,
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
            // TODO: atl: rename server-side field to `wasi_config`.
            wasi_config: value.config_vars,
            // TODO: atl: rename server-side field to `config`.
            config: value.agent_config.into_iter().map(Into::into).collect(),
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
pub struct AgentsMetadataResponseView {
    pub agents: Vec<AgentMetadataView>,
    pub cursors: BTreeMap<String, String>,
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
    pub agent_name: RawAgentId,
    pub source_language: SourceLanguage,
}

impl AgentNameMatch {
    pub fn environment_reference(&self) -> Option<&EnvironmentReference> {
        match &self.environment.source {
            ResolvedEnvironmentIdentitySource::Reference(reference) => Some(reference),
            ResolvedEnvironmentIdentitySource::DefaultFromManifest => None,
        }
    }
}
