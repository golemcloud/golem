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

use crate::log::{LogColorize, logln};
use crate::model::cli_output::StructuredOutput;
use crate::model::deploy::{EnvironmentSetupDisplay, EnvironmentSetupPlan};
use crate::model::masking::{Masked, MaskingConfig, mask_secret_with_fingerprint};
use crate::model::text::fmt::TextOutput;
use colored::Colorize;
use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::diff::{
    AgentTypeProvisionConfigDiff, BTreeMapDiffValue, DeploymentDiff, DiffForHashOf,
};
use serde::Serialize;
use serde::ser::Serializer;
use std::path::Path;

const DIFF_COLLAPSE_THRESHOLD: usize = 12;
const DIFF_COLLAPSE_KEEP_HEAD: usize = 3;
const DIFF_COLLAPSE_KEEP_TAIL: usize = 3;
const DIFF_COLLAPSE_DOTS: usize = 3;

impl TextOutput for DeploymentDiff {
    fn log(&self) {
        logln("");
        if !self.components.is_empty() {
            logln("Component changes:".log_color_help_group().to_string());
            for (component_name, component_diff) in &self.components {
                match component_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} component {}",
                            "create".green(),
                            component_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} component {}",
                            "delete".red(),
                            component_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        DiffForHashOf::HashDiff { .. } => {
                            logln(format!(
                                "  - {} component {}",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                        }
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} component {}, changes:",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                            if diff.wasm_changed {
                                logln("    - binary");
                            }
                            if !diff.agent_type_provision_config_changes.is_empty() {
                                logln("    - provision configs");
                                for (agent_type, change) in
                                    &diff.agent_type_provision_config_changes
                                {
                                    match change {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} agent type {}",
                                                "create".green(),
                                                agent_type.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} agent type {}",
                                                "delete".red(),
                                                agent_type.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(inner) => {
                                            logln(format!(
                                                "      - {} agent type {}:",
                                                "update".yellow(),
                                                agent_type.log_color_highlight()
                                            ));
                                            if let DiffForHashOf::ValueDiff { diff } = inner {
                                                log_provision_config_diff(diff);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            logln("");
        }
        if !self.http_api_deployments.is_empty() {
            logln(
                "HTTP API deployment changes:"
                    .log_color_help_group()
                    .to_string(),
            );
            for (domain, http_api_deployment_diff) in &self.http_api_deployments {
                match http_api_deployment_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "create".green(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "delete".red(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        DiffForHashOf::HashDiff { .. } => logln(format!(
                            "  - {} HTTP API deployment {}",
                            "update".yellow(),
                            domain.log_color_highlight()
                        )),
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} HTTP API deployment {}, changes:",
                                "update".yellow(),
                                domain.log_color_highlight()
                            ));
                            if diff.webhooks_url_changed {
                                logln("    - webhooks_url");
                            }
                            if diff.openapi_endpoint_changed {
                                logln("    - openapi_endpoint");
                            }
                            if !diff.agents_changes.is_empty() {
                                logln("    - agents");
                                for (agent_name, agent_diff) in &diff.agents_changes {
                                    match agent_diff {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "create".green(),
                                                agent_name.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "delete".red(),
                                                agent_name.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(diff) => {
                                            logln(format!(
                                                "      - {} agent {}, changes:",
                                                "update".yellow(),
                                                agent_name.log_color_highlight()
                                            ));
                                            if diff.security_scheme_changed {
                                                logln("        - security_scheme");
                                            }
                                            if diff.test_session_header_changed {
                                                logln("        - test_session_header");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            logln("");
        }
        if !self.mcp_deployments.is_empty() {
            logln("MCP deployment changes:".log_color_help_group().to_string());
            for (domain, mcp_deployment_diff) in &self.mcp_deployments {
                match mcp_deployment_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} MCP deployment {}",
                            "create".green(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} MCP deployment {}",
                            "delete".red(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        DiffForHashOf::HashDiff { .. } => {
                            logln(format!(
                                "  - {} MCP deployment {}",
                                "update".yellow(),
                                domain.log_color_highlight()
                            ));
                        }
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} MCP deployment {}, changes:",
                                "update".yellow(),
                                domain.log_color_highlight()
                            ));
                            if !diff.agents_changes.is_empty() {
                                logln("    - agents");
                                for (agent_name, agent_diff) in &diff.agents_changes {
                                    match agent_diff {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "create".green(),
                                                agent_name.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} agent {}",
                                                "delete".red(),
                                                agent_name.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(diff) => {
                                            logln(format!(
                                                "      - {} agent {}, changes:",
                                                "update".yellow(),
                                                agent_name.log_color_highlight()
                                            ));
                                            if diff.security_scheme_changed {
                                                logln("        - security_scheme");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            logln("");
        }
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        let _ = config;
        self.log();
        Ok(())
    }
}

impl StructuredOutput for DeploymentDiff {
    const KIND: &'static str = "deploy.diff";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl Masked for DeploymentDiff {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        if config.show_secrets {
            return Ok(self);
        }

        mask_deployment_diff_secrets(&mut self)?;
        Ok(self)
    }
}

fn mask_deployment_diff_secrets(diff: &mut DeploymentDiff) -> anyhow::Result<()> {
    for component_change in diff.components.values_mut() {
        let BTreeMapDiffValue::Update(component_diff) = component_change else {
            continue;
        };
        let DiffForHashOf::ValueDiff {
            diff: component_diff,
        } = component_diff
        else {
            continue;
        };

        for provision_config_change in component_diff
            .agent_type_provision_config_changes
            .values_mut()
        {
            let BTreeMapDiffValue::Update(provision_config_diff) = provision_config_change else {
                continue;
            };
            let DiffForHashOf::ValueDiff {
                diff: provision_config_diff,
            } = provision_config_diff
            else {
                continue;
            };
            mask_agent_type_provision_config_diff(provision_config_diff)?;
        }
    }

    Ok(())
}

fn mask_agent_type_provision_config_diff(
    diff: &mut AgentTypeProvisionConfigDiff,
) -> anyhow::Result<()> {
    for env_change in diff.env_changes.values_mut() {
        mask_string_diff_update(env_change)?;
    }

    for config_change in diff.config_changes.values_mut() {
        mask_normalized_json_diff_update(config_change)?;
    }

    Ok(())
}

fn mask_string_diff_update(change: &mut BTreeMapDiffValue<String>) -> anyhow::Result<()> {
    if let BTreeMapDiffValue::Update(update) = change {
        *update = mask_secret_with_fingerprint(&serde_json::to_string(update)?);
    }
    Ok(())
}

fn mask_normalized_json_diff_update(
    change: &mut BTreeMapDiffValue<NormalizedJsonValue>,
) -> anyhow::Result<()> {
    if let BTreeMapDiffValue::Update(update) = change {
        *update = NormalizedJsonValue(serde_json::Value::String(mask_secret_with_fingerprint(
            &serde_json::to_string(update)?,
        )));
    }
    Ok(())
}

fn log_provision_config_diff(diff: &AgentTypeProvisionConfigDiff) {
    if !diff.env_changes.is_empty() {
        logln("        - env");
    }
    if !diff.config_changes.is_empty() {
        logln("        - agent config");
    }
    if !diff.file_changes.is_empty() {
        logln("        - files");
        for (path, file_diff) in &diff.file_changes {
            match file_diff {
                BTreeMapDiffValue::Create => logln(format!(
                    "          - {} {}",
                    "add".green(),
                    path.log_color_highlight()
                )),
                BTreeMapDiffValue::Delete => logln(format!(
                    "          - {} {}",
                    "remove".red(),
                    path.log_color_highlight()
                )),
                BTreeMapDiffValue::Update(inner) => {
                    if let DiffForHashOf::ValueDiff { diff } = inner {
                        let mut changes = vec![];
                        if diff.content_changed {
                            changes.push("content");
                        }
                        if diff.permissions_changed {
                            changes.push("permissions");
                        }
                        logln(format!(
                            "          - {} {} ({})",
                            "update".yellow(),
                            path.log_color_highlight(),
                            changes.join(", ")
                        ));
                    }
                }
            }
        }
    }
    if !diff.plugin_changes.is_empty() {
        // TODO: show plugin name/version once grant ID → name mapping is available
        logln(format!(
            "        - plugins ({} change(s))",
            diff.plugin_changes.len()
        ));
    }
}

pub fn log_unified_diff(diff: &str) {
    for line in diff.lines() {
        log_unified_diff_line(classify_diff_line(line));
    }
}

pub fn log_unified_diff_for_path(path: &Path, diff: &str) {
    if is_compact_diff_path(path) {
        log_unified_diff_compact(diff);
    } else {
        log_unified_diff(diff);
    }
}

pub struct EnvironmentSetupPlanView<'a>(pub &'a EnvironmentSetupPlan);

impl Serialize for EnvironmentSetupPlanView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // EnvironmentSetupPlan.display is built with the active MaskingConfig.
        // This view serializes that prepared display and must not be constructed
        // from display data that skipped environment setup masking.
        self.0.display.serialize(serializer)
    }
}

pub struct DeployPlanView<'a> {
    pub deployment_diff: &'a DeploymentDiff,
    pub environment_setup: Option<&'a EnvironmentSetupPlan>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeployPlanFields<'a> {
    deployment_diff: &'a DeploymentDiff,
    environment_setup: Option<&'a EnvironmentSetupDisplay>,
}

impl Serialize for DeployPlanView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deployment_diff = self
            .deployment_diff
            .clone()
            .masked(MaskingConfig::hide_secrets())
            .map_err(serde::ser::Error::custom)?;

        DeployPlanFields {
            deployment_diff: &deployment_diff,
            environment_setup: self.environment_setup.map(|setup| &setup.display),
        }
        .serialize(serializer)
    }
}

impl TextOutput for DeployPlanView<'_> {
    fn log(&self) {
        let has_deployment_changes = !self.deployment_diff.components.is_empty()
            || !self.deployment_diff.http_api_deployments.is_empty()
            || !self.deployment_diff.mcp_deployments.is_empty();

        if has_deployment_changes {
            self.deployment_diff.log();
        }

        if let Some(environment_setup) = self.environment_setup.map(EnvironmentSetupPlanView)
            && !environment_setup.0.display.is_empty()
        {
            environment_setup.log();
        }
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        let _ = config;
        self.log();
        Ok(())
    }
}

impl StructuredOutput for DeployPlanView<'_> {
    const KIND: &'static str = "deploy.plan";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let deployment_diff = self
            .deployment_diff
            .clone()
            .masked(config)
            .map_err(serde::ser::Error::custom)?;

        DeployPlanFields {
            deployment_diff: &deployment_diff,
            environment_setup: self.environment_setup.map(|setup| &setup.display),
        }
        .serialize(serializer)
    }
}

impl TextOutput for EnvironmentSetupPlanView<'_> {
    fn log(&self) {
        let setup = self.0;

        if !setup.display.to_be_applied.is_empty() {
            logln(
                "Environment setup to apply:"
                    .log_color_help_group()
                    .to_string(),
            );
            if !setup.display.to_be_applied.secret_values.is_empty() {
                for key in setup.display.to_be_applied.secret_values.keys() {
                    logln(format!(
                        "  - create secret value {}",
                        key.log_color_highlight()
                    ));
                }
            }
            if !setup.display.to_be_applied.retry_policies.is_empty() {
                for key in setup.display.to_be_applied.retry_policies.keys() {
                    logln(format!(
                        "  - create retry policy {}",
                        key.log_color_highlight()
                    ));
                }
            }
            if !setup.display.to_be_applied.resources.is_empty() {
                for key in setup.display.to_be_applied.resources.keys() {
                    logln(format!("  - create resource {}", key.log_color_highlight()));
                }
            }
        }

        if !setup.display.skipped_already_exists.is_empty() {
            if !setup.display.to_be_applied.is_empty() {
                logln("");
            }
            logln(
                "Environment setup skipped because it already exists:"
                    .log_color_help_group()
                    .to_string(),
            );
            if !setup
                .display
                .skipped_already_exists
                .secret_values
                .is_empty()
            {
                for key in &setup.display.skipped_already_exists.secret_values {
                    logln(format!("  - secret value {}", key.log_color_highlight()));
                }
            }
            if !setup
                .display
                .skipped_already_exists
                .retry_policies
                .is_empty()
            {
                for key in &setup.display.skipped_already_exists.retry_policies {
                    logln(format!("  - retry policy {}", key.log_color_highlight()));
                }
            }
            if !setup.display.skipped_already_exists.resources.is_empty() {
                for key in &setup.display.skipped_already_exists.resources {
                    logln(format!("  - resource {}", key.log_color_highlight()));
                }
            }
        }
    }
}

impl StructuredOutput for EnvironmentSetupPlanView<'_> {
    const KIND: &'static str = "deploy.environment-setup-plan";
}

impl EnvironmentSetupPlanView<'_> {
    pub fn has_entries_to_apply(&self) -> bool {
        !self.0.display.to_be_applied.is_empty()
    }
}

fn is_compact_diff_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn log_unified_diff_compact(diff: &str) {
    let lines: Vec<DiffLine<'_>> = diff.lines().map(classify_diff_line).collect();
    let runs = regroup_diff_lines(&lines);

    for run in runs {
        render_diff_run(run);
    }
}

fn regroup_diff_lines<'a>(lines: &'a [DiffLine<'a>]) -> Vec<DiffRun<'a>> {
    let mut runs = Vec::new();

    for line in lines {
        match line {
            DiffLine::Added(_) => push_change_line(&mut runs, ChangeKind::Added, *line),
            DiffLine::Removed(_) => push_change_line(&mut runs, ChangeKind::Removed, *line),
            _ => push_other_line(&mut runs, *line),
        }
    }

    runs
}

fn push_change_line<'a>(runs: &mut Vec<DiffRun<'a>>, kind: ChangeKind, line: DiffLine<'a>) {
    match runs.last_mut() {
        Some(DiffRun::Change {
            kind: existing_kind,
            lines,
        }) if *existing_kind == kind => lines.push(line),
        _ => runs.push(DiffRun::Change {
            kind,
            lines: vec![line],
        }),
    }
}

fn push_other_line<'a>(runs: &mut Vec<DiffRun<'a>>, line: DiffLine<'a>) {
    match runs.last_mut() {
        Some(DiffRun::Other(lines)) => lines.push(line),
        _ => runs.push(DiffRun::Other(vec![line])),
    }
}

fn render_diff_run(run: DiffRun<'_>) {
    match run {
        DiffRun::Change { lines, .. } if lines.len() > DIFF_COLLAPSE_THRESHOLD => {
            let head_keep = DIFF_COLLAPSE_KEEP_HEAD.min(lines.len());
            let tail_keep = DIFF_COLLAPSE_KEEP_TAIL.min(lines.len() - head_keep);

            for line in lines.iter().take(head_keep) {
                log_unified_diff_line(*line);
            }

            for _ in 0..DIFF_COLLAPSE_DOTS {
                logln(".".dimmed().to_string());
            }

            for line in lines.iter().skip(lines.len() - tail_keep).take(tail_keep) {
                log_unified_diff_line(*line);
            }
        }
        DiffRun::Change { lines, .. } | DiffRun::Other(lines) => {
            for line in lines {
                log_unified_diff_line(line);
            }
        }
    }
}

fn log_unified_diff_line(line: DiffLine<'_>) {
    match line {
        DiffLine::Added(raw) => logln(raw.green().bold().to_string()),
        DiffLine::Removed(raw) => logln(raw.red().bold().to_string()),
        DiffLine::Hunk(raw) => logln(raw.bold().to_string()),
        DiffLine::Other(raw) => logln(raw),
    }
}

fn classify_diff_line(line: &str) -> DiffLine<'_> {
    if line.starts_with('+') && !line.starts_with("+++") {
        DiffLine::Added(line)
    } else if line.starts_with('-') && !line.starts_with("---") {
        DiffLine::Removed(line)
    } else if line.starts_with("@@") {
        DiffLine::Hunk(line)
    } else {
        DiffLine::Other(line)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ChangeKind {
    Added,
    Removed,
}

#[derive(Clone, Copy)]
enum DiffLine<'a> {
    Added(&'a str),
    Removed(&'a str),
    Hunk(&'a str),
    Other(&'a str),
}

enum DiffRun<'a> {
    Change {
        kind: ChangeKind,
        lines: Vec<DiffLine<'a>>,
    },
    Other(Vec<DiffLine<'a>>),
}
