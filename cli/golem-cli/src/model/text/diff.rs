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
use crate::model::text::fmt::TextView;
use colored::Colorize;
use golem_common::model::diff::{
    AgentTypeProvisionConfigDiff, BTreeMapDiffValue, DeploymentDiff, DiffForHashOf,
};
use std::path::Path;

const DIFF_COLLAPSE_THRESHOLD: usize = 12;
const DIFF_COLLAPSE_KEEP_HEAD: usize = 3;
const DIFF_COLLAPSE_KEEP_TAIL: usize = 3;
const DIFF_COLLAPSE_DOTS: usize = 3;

impl TextView for DeploymentDiff {
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
                                                "      - {} file {}",
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
                    BTreeMapDiffValue::Update(_diff) => {
                        logln(format!(
                            "  - {} MCP deployment {}",
                            "update".yellow(),
                            domain.log_color_highlight()
                        ));
                    }
                }
            }
            logln("");
        }
    }
}

fn log_provision_config_diff(diff: &AgentTypeProvisionConfigDiff) {
    if !diff.env_changes.is_empty() {
        logln("        - env");
    }
    if !diff.wasi_config_changes.is_empty() {
        logln("        - wasi config");
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
    if path.file_name().and_then(|name| name.to_str()) == Some("AGENTS.md") {
        log_unified_diff_compact(diff);
    } else {
        log_unified_diff(diff);
    }
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
