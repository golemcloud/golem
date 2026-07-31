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

pub mod action_result;
pub mod extraction;
pub mod files;
pub mod oplog;
pub mod stream;

use crate::agent_id_display::SourceLanguage;
use crate::command::shared_args::StreamArgs;
use crate::log::{LogColorize, logln};
use crate::model::cli_output::StructuredOutput;
use crate::model::component::{ComponentNameMatchKind, render_agent_constructor};
use crate::model::environment::{
    EnvironmentReference, ResolvedEnvironmentIdentity, ResolvedEnvironmentIdentitySource,
};
use crate::model::masking::{Masked, MaskingConfig, mask_agent_config_entries, mask_sensitive_map};
use crate::model::text::fmt::*;
use chrono::DateTime;
use clap::ValueEnum;
use colored::Colorize;
use colored::control::SHOULD_COLORIZE;
use comfy_table::Color as ComfyColor;
use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, DeployedRegisteredAgentType, ParsedAgentId};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::worker::{AgentConfigEntryDto, UpdateRecord};
use golem_common::model::{AgentId, AgentResourceDescription, AgentStatus, Timestamp};
use itertools::Itertools;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::{Display, Formatter, Write};
use std::str::FromStr;

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
    pub agent_id: RawAgentId,
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
            agent_id: value.agent_id.agent_id.into(),
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
    pub fn with_defaults(mut self, defaults: Option<AgentTypeProvisionConfig>) -> Self {
        if let Some(defaults) = defaults {
            self.default_env = defaults.env.into_iter().collect();
            self.default_config = defaults.config.into_iter().map(Into::into).collect();
        }
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

pub struct AgentIdMatch {
    pub environment: ResolvedEnvironmentIdentity,
    pub component_name_match_kind: ComponentNameMatchKind,
    pub component_name: ComponentName,
    pub agent_type_name: AgentTypeName,
    pub agent_id: RawAgentId,
    pub source_language: SourceLanguage,
    pub parsed_agent_id: Option<ParsedAgentId>,
}

impl AgentIdMatch {
    pub fn environment_reference(&self) -> Option<&EnvironmentReference> {
        match &self.environment.source {
            ResolvedEnvironmentIdentitySource::Reference(reference) => Some(reference),
            ResolvedEnvironmentIdentitySource::DefaultFromManifest => None,
        }
    }

    /// Updates the canonical agent_id and the parsed form together. Use this
    /// after re-canonicalizing or normalizing the agent id so that downstream
    /// display code can use the language-specific renderer.
    pub fn with_canonical_and_parsed(
        mut self,
        agent_id: RawAgentId,
        parsed_agent_id: Option<ParsedAgentId>,
    ) -> Self {
        self.agent_id = agent_id;
        self.parsed_agent_id = parsed_agent_id;
        self
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTypeView {
    pub agent_type: String,
    pub constructor: String,
    pub description: String,
}

impl AgentTypeView {
    pub fn new(value: &DeployedRegisteredAgentType, wrapper_naming: bool) -> Self {
        Self {
            agent_type: value.agent_type.type_name.to_string(),
            constructor: render_agent_constructor(&value.agent_type, wrapper_naming, false),
            description: value.agent_type.description.clone(),
        }
    }
}

impl MessageWithFields for AgentTypeView {
    fn message(&self) -> String {
        format!(
            "Got deployed agent type: {} ",
            format_message_highlight(&self.agent_type)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields.field("Agent type", &self.agent_type);
        fields.field("Constructor", &self.constructor);
        fields.field("Description", &self.description);

        fields.build()
    }
}

impl Masked for AgentTypeView {}

impl StructuredOutput for AgentTypeView {
    const KIND: &'static str = "agent-type.get";
}

impl From<&DeployedRegisteredAgentType> for AgentTypeView {
    fn from(value: &DeployedRegisteredAgentType) -> Self {
        AgentTypeView::new(value, true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTypeListView {
    pub agent_types: Vec<DeployedRegisteredAgentType>,
}

impl StructuredOutput for AgentTypeListView {
    const KIND: &'static str = "agent-type.list";
}

impl TextOutput for AgentTypeListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("Agent Type").fixed(),
            Column::new("Constructor"),
            Column::new("Description"),
        ]);
        for agent_type in &self.agent_types {
            let view = AgentTypeView::new(agent_type, true);
            table.add_row(vec![view.agent_type, view.constructor, view.description]);
        }
        log_table(table);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCreateView {
    pub component_name: ComponentName,
    pub agent_id: RawAgentId,
}

impl Masked for AgentCreateView {}

impl MessageWithFields for AgentCreateView {
    fn message(&self) -> String {
        format!(
            "Created new agent {}",
            format_message_highlight(&self.agent_id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Component name", &self.component_name, format_id)
            .fmt_field("Agent ID", &self.agent_id, |agent_id| {
                format_agent_id_in(
                    &agent_id.0,
                    colored::control::SHOULD_COLORIZE.should_colorize(),
                    field_value_width::<Self>(),
                )
            });

        fields.build()
    }
}

impl StructuredOutput for AgentCreateView {
    const KIND: &'static str = "agent.new";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentGetView {
    pub metadata: AgentMetadataView,
    pub precise: bool,
}

impl AgentGetView {
    pub fn from_metadata(metadata: AgentMetadataView, precise: bool) -> Self {
        Self { metadata, precise }
    }
}

impl Masked for AgentGetView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        self.metadata = self.metadata.masked(config)?;
        Ok(self)
    }
}

fn format_untyped_config(config: &[AgentConfigEntryDto]) -> String {
    config
        .iter()
        .map(|entry| {
            format!(
                "{}={}",
                entry.path.join(".").log_color_highlight(),
                entry.value.0
            )
        })
        .join("\n")
}

fn to_sorted_btree_map(map: &HashMap<String, String>) -> BTreeMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

impl MessageWithFields for AgentGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for agent {}",
            format_message_highlight(&self.metadata.agent_id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        let mut update_history = String::new();
        for update in &self.metadata.updates {
            match update {
                UpdateRecord::PendingUpdate(update) => {
                    let _ = writeln!(
                        update_history,
                        "{}",
                        format!(
                            "{}: Pending update to {}",
                            update.timestamp, update.target_revision
                        )
                        .bright_black()
                    );
                }
                UpdateRecord::SuccessfulUpdate(update) => {
                    let _ = writeln!(
                        update_history,
                        "{}",
                        format!(
                            "{}: Successful update to {}",
                            update.timestamp, update.target_revision
                        )
                        .green()
                        .bold()
                    );
                }
                UpdateRecord::FailedUpdate(update) => {
                    let _ = writeln!(
                        update_history,
                        "{}",
                        format!(
                            "{}: Failed update to {}{}",
                            update.timestamp,
                            update.target_revision,
                            update
                                .details
                                .as_ref()
                                .map(|details| format!(": {details}"))
                                .unwrap_or_default()
                        )
                        .yellow()
                    );
                }
            }
        }

        fields
            .fmt_field("Component name", &self.metadata.component_name, format_id)
            .fmt_field(
                "Component revision",
                &self.metadata.component_revision,
                format_id,
            )
            .fmt_field("Agent ID", &self.metadata.agent_id, |agent_id| {
                format_agent_id_in(
                    &agent_id.0,
                    colored::control::SHOULD_COLORIZE.should_colorize(),
                    field_value_width::<Self>(),
                )
            })
            .field("Created at", &self.metadata.created_at)
            .fmt_field(
                "Component size",
                &self.metadata.component_size,
                format_binary_size,
            )
            .fmt_field(
                "Total linear memory size",
                &self.metadata.total_linear_memory_size,
                format_binary_size,
            )
            .fmt_field_optional(
                "Environment variables - defaults",
                &self.metadata.default_env,
                !self.metadata.default_env.is_empty(),
                |env| format_env(&to_sorted_btree_map(env)),
            )
            .fmt_field_optional(
                "Environment variables - overrides",
                &self.metadata.env,
                !self.metadata.env.is_empty(),
                |env| format_env(&to_sorted_btree_map(env)),
            )
            .fmt_field_optional(
                "Config - defaults",
                &self.metadata.default_config,
                !self.metadata.default_config.is_empty(),
                |config| format_untyped_config(config),
            )
            .fmt_field_optional(
                "Config - overrides",
                &self.metadata.config,
                !self.metadata.config.is_empty(),
                |config| format_untyped_config(config),
            )
            .fmt_field_optional("Status", &self.metadata.status, self.precise, format_status)
            .fmt_field_optional(
                "Retry count",
                &self.metadata.retry_count,
                self.precise,
                format_retry_count,
            )
            .fmt_field_optional(
                "Pending invocation count",
                &self.metadata.pending_invocation_count,
                self.metadata.pending_invocation_count > 0,
                |n| n.to_string(),
            )
            .fmt_field_optional(
                "Last error",
                &self.metadata.last_error,
                self.metadata.last_error.is_some() && self.precise,
                |err| format_stack(err.as_ref().unwrap()),
            )
            .fmt_field_optional(
                "WARNING",
                "The presented agent metadata may not be up-to-date",
                !self.precise,
                format_warn,
            );

        fields.build()
    }
}

impl StructuredOutput for AgentGetView {
    const KIND: &'static str = "agent.get";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl StructuredOutput for AgentsMetadataResponseView {
    const KIND: &'static str = "agent.list";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl TextOutput for AgentsMetadataResponseView {
    fn log(&self) {
        let colorize = colored::control::SHOULD_COLORIZE.should_colorize();
        let term_width = terminal_width();
        logln(Self::format_table_wide(
            &self.agents,
            term_width,
            colorize,
            false,
        ));

        if !self.cursors.is_empty() {
            logln("");
        }
        for (component_name, cursor) in &self.cursors {
            logln(format!(
                "Cursor for more results for component {}: {}",
                component_name.log_color_highlight(),
                cursor.log_color_highlight()
            ));
        }
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        self.masked(config)?.log();
        Ok(())
    }
}

/// Agent-list component-name column: capped at `MAX` so it cannot eat the agent
/// id budget, squeezable to `MIN` when ids need the room.
const MAX_COMPONENT_NAME_WIDTH: usize = 28;
const MIN_COMPONENT_NAME_WIDTH: usize = 12;

/// Below this the agent id column is left unformatted for the table to wrap.
const MIN_AGENT_NAME_WIDTH: usize = 24;

impl AgentsMetadataResponseView {
    fn status_color(status: &AgentStatus, colorize: bool) -> ComfyColor {
        if colorize {
            match status {
                AgentStatus::Running => ComfyColor::Green,
                AgentStatus::Idle => ComfyColor::Cyan,
                AgentStatus::Suspended => ComfyColor::Yellow,
                AgentStatus::Interrupted => ComfyColor::Red,
                AgentStatus::Retrying => ComfyColor::Yellow,
                AgentStatus::Failed => ComfyColor::Red,
                AgentStatus::Exited => ComfyColor::White,
            }
        } else {
            ComfyColor::Reset
        }
    }

    fn format_table_wide(
        agents: &[AgentMetadataView],
        term_width: u16,
        colorize: bool,
        full_width: bool,
    ) -> String {
        // Agent ids are self-formatted (broken at their own structure), so the
        // column width must be known before the cells are built;
        // `self_formatting_table` budgets it. `Range` marks the component name as
        // the squeezable column.
        let headers = vec![
            Column::new("Component name")
                .width_range(MIN_COMPONENT_NAME_WIDTH, MAX_COMPONENT_NAME_WIDTH),
            Column::new("Agent ID"),
            Column::new("Revision").content_right(),
            Column::new("Status").content_right(),
            Column::new("Pending").content_right(),
            Column::new("Created at").content(),
        ];

        let format_agent_id = |raw: &str, width: Option<usize>| match width {
            Some(width) => format_agent_id_in(raw, colorize, width),
            None => raw.to_string(),
        };

        let rows = agents
            .iter()
            .map(|agent| {
                vec![
                    TableCell::new(agent.component_name.to_string()),
                    TableCell::new(agent.agent_id.0.clone()),
                    TableCell::new(agent.component_revision.to_string()).right(),
                    TableCell::new(agent.status.to_string())
                        .right()
                        .color(Self::status_color(&agent.status, colorize)),
                    TableCell::new(agent.pending_invocation_count.to_string()).right(),
                    TableCell::new(agent.created_at.to_string()),
                ]
            })
            .collect();

        self_formatting_table(SelfFormattingTableSpec {
            preset: TablePreset::FullCondensed,
            term_width,
            full_width,
            headers,
            flex: FlexColumn {
                index: 1,
                min_width: MIN_AGENT_NAME_WIDTH,
                format: &format_agent_id,
            },
            rows,
        })
        .to_string()
    }
}

impl TruncatableTextOutput for AgentsMetadataResponseView {
    fn render_truncated(&self, max_lines: usize, colorize: bool) -> String {
        let cursor_lines = if self.cursors.is_empty() {
            0
        } else {
            1 + self.cursors.len()
        };
        let available_for_table = max_lines.saturating_sub(cursor_lines);

        let term_width = terminal_width();
        let table_str = Self::format_table_wide(&self.agents, term_width, colorize, true);

        let mut out = truncate_rendered(table_str, available_for_table);

        if !self.cursors.is_empty() {
            out.push('\n');
            for (component_name, cursor) in &self.cursors {
                out.push('\n');
                out.push_str(&format!(
                    "Cursor for more results for component {}: {}",
                    component_name.log_color_highlight(),
                    cursor.log_color_highlight()
                ));
            }
        }

        out
    }

    fn render_truncated_masked(
        &self,
        max_lines: usize,
        colorize: bool,
        config: MaskingConfig,
    ) -> anyhow::Result<String> {
        Ok(self
            .clone()
            .masked(config)?
            .render_truncated(max_lines, colorize))
    }
}

/// Formats an agent id to a caller-supplied width (see `format_agent_id_for_terminal`).
fn format_agent_id_in(agent_id: &str, colorize: bool, width: usize) -> String {
    crate::agent_id_display::format_agent_id_for_terminal(agent_id, colorize, Some(width))
}

// Helper function to convert Unix timestamp to human-readable format
pub fn format_timestamp(timestamp: u64) -> String {
    if let Some(datetime) = DateTime::from_timestamp(timestamp as i64, 0) {
        datetime.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        format!("{timestamp}") // Fallback to raw timestamp if conversion fails
    }
}

pub fn format_agent_id_match(agent_id_match: &AgentIdMatch) -> String {
    let rendered_agent_id = crate::agent_id_display::render_agent_id_or_raw(
        agent_id_match.parsed_agent_id.as_ref(),
        &agent_id_match.source_language,
        &agent_id_match.agent_id.0,
    );

    format!(
        "{}{}/{}",
        match &agent_id_match.environment_reference() {
            Some(environment_reference) => {
                match environment_reference {
                    EnvironmentReference::Environment { environment_name } => {
                        format!("{}/", environment_name.0.blue().bold())
                    }
                    EnvironmentReference::ApplicationEnvironment {
                        application_name,
                        environment_name,
                    } => {
                        format!(
                            "{}/{}/",
                            application_name.0.blue().bold(),
                            environment_name.0.blue().bold()
                        )
                    }
                    EnvironmentReference::AccountApplicationEnvironment {
                        account_email,
                        application_name,
                        environment_name,
                    } => {
                        format!(
                            "{}/{}/{}/",
                            account_email.blue().bold(),
                            application_name.0.blue().bold(),
                            environment_name.0.blue().bold()
                        )
                    }
                }
            }
            None => "".to_string(),
        },
        agent_id_match.component_name.0.blue().bold(),
        rendered_agent_id.green().bold(),
    )
}

fn format_status(status: &AgentStatus) -> String {
    let status_name = status.to_string();
    match status {
        AgentStatus::Running => status_name.green(),
        AgentStatus::Idle => status_name.cyan(),
        AgentStatus::Suspended => status_name.yellow(),
        AgentStatus::Interrupted => status_name.red(),
        AgentStatus::Retrying => status_name.yellow(),
        AgentStatus::Failed => status_name.bright_red(),
        AgentStatus::Exited => status_name.white(),
    }
    .to_string()
}

fn format_retry_count(retry_count: &u32) -> String {
    if *retry_count == 0 {
        retry_count.to_string()
    } else {
        format_warn(&retry_count.to_string())
    }
}
