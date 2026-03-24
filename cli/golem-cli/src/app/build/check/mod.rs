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

mod requirements;

use crate::app::build::check::requirements::{
    tool_requirements_for_language, ToolRequirement, ToolRequirementCheck, VersionRange,
};
use crate::app::context::{validated_to_anyhow, BuildContext};
use crate::log::{log_action, LogColorize, LogIndent};
use crate::model::GuestLanguage;
use crate::validation::ValidationBuilder;
use anyhow::{anyhow, bail};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;
use version_compare::{Cmp, Version};

pub async fn check_app(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    let selected_languages = selected_component_languages(ctx);

    if selected_languages.is_empty() {
        return Ok(());
    }

    log_action("Checking", "build requirements");
    let _indent = LogIndent::new();

    let mut requirements = BTreeMap::<&'static str, ToolRequirement>::new();
    for language in selected_languages {
        for requirement in tool_requirements_for_language(language) {
            requirements.insert(requirement.key, *requirement);
        }
    }

    let app_root_dir = ctx.application().app_root_dir();
    let mut validation = ValidationBuilder::new();

    for requirement in requirements.into_values() {
        validation.with_context(
            vec![("tool", requirement.name.to_string())],
            |validation| {
                if let Err(error) = check_tool_requirement(app_root_dir, requirement) {
                    validation.add_error(error.to_string());
                }
            },
        );
    }

    validated_to_anyhow(
        "Build requirements check failed",
        validation.build(()),
        None,
    )
}

fn selected_component_languages(ctx: &BuildContext<'_>) -> BTreeSet<GuestLanguage> {
    ctx.application_context()
        .selected_component_names()
        .iter()
        .filter_map(|component_name| {
            ctx.application()
                .component(component_name)
                .guess_language()
        })
        .collect()
}

fn check_tool_requirement(project_dir: &Path, requirement: ToolRequirement) -> anyhow::Result<()> {
    match requirement.check {
        ToolRequirementCheck::CommandVersion { command, args } => {
            check_command_version(project_dir, requirement, command, args)
        }
        ToolRequirementCheck::RustTargetInstalled { target } => {
            check_rust_target(project_dir, requirement, target)
        }
    }
}

fn check_command_version(
    project_dir: &Path,
    requirement: ToolRequirement,
    command: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    let output = Command::new(command)
        .current_dir(project_dir)
        .args(args)
        .output()
        .map_err(|err| {
            anyhow!(
                "{} ({}) is not available: {}\nHint: {}",
                requirement.name.log_color_error_highlight(),
                command,
                err,
                requirement.install_hint
            )
        })?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "command failed without output".to_string()
        };

        bail!(
            "{} ({}) failed to run: {}\nHint: {}",
            requirement.name.log_color_error_highlight(),
            command,
            detail,
            requirement.install_hint
        );
    }

    if let Some(range) = requirement.version_range {
        let output_text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let version = extract_version(output_text.as_str()).ok_or_else(|| {
            anyhow!(
                "{} ({}) version could not be detected from output\nHint: {}",
                requirement.name.log_color_error_highlight(),
                command,
                requirement.install_hint
            )
        })?;

        verify_version_range(requirement.name, &version, range).map_err(|err| {
            anyhow!(
                "{} ({}) {}\nHint: {}",
                requirement.name.log_color_error_highlight(),
                command,
                err,
                requirement.install_hint
            )
        })?;
    }

    Ok(())
}

fn check_rust_target(
    project_dir: &Path,
    requirement: ToolRequirement,
    target: &str,
) -> anyhow::Result<()> {
    let output = Command::new("rustup")
        .current_dir(project_dir)
        .args(["target", "list", "--installed"])
        .output()
        .map_err(|err| {
            anyhow!(
                "{} is not available: {}\nHint: {}",
                "rustup".log_color_error_highlight(),
                err,
                requirement.install_hint
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            "rustup target list --installed failed".to_string()
        } else {
            stderr
        };
        bail!(
            "{} check failed: {}\nHint: {}",
            requirement.name.log_color_error_highlight(),
            detail,
            requirement.install_hint
        );
    }

    let installed_targets = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim())
        .collect::<HashSet<_>>();

    if installed_targets.contains(target) {
        Ok(())
    } else {
        bail!(
            "{} is missing ({})\nHint: {}",
            requirement.name.log_color_error_highlight(),
            target,
            requirement.install_hint
        )
    }
}

fn extract_version(output: &str) -> Option<String> {
    let regex = Regex::new(r"([0-9]+\.[0-9]+(?:\.[0-9]+)?)").ok()?;
    regex
        .captures(output)
        .and_then(|captures| captures.get(1))
        .map(|matched| matched.as_str().to_string())
}

fn verify_version_range(name: &str, actual: &str, range: VersionRange) -> anyhow::Result<()> {
    let actual = Version::from(actual)
        .ok_or_else(|| anyhow!("detected version '{actual}' for {name} is not parseable"))?;

    if let Some(min_inclusive) = range.min_inclusive {
        let min = Version::from(min_inclusive)
            .ok_or_else(|| anyhow!("minimum version '{min_inclusive}' is not parseable"))?;

        if matches!(actual.compare(&min), Cmp::Lt) {
            bail!(
                "is too old: detected {}, expected {}",
                actual,
                version_range_to_text(range)
            );
        }
    }

    if let Some(max_exclusive) = range.max_exclusive {
        let max = Version::from(max_exclusive)
            .ok_or_else(|| anyhow!("maximum version '{max_exclusive}' is not parseable"))?;

        if !matches!(actual.compare(&max), Cmp::Lt) {
            bail!(
                "is too new: detected {}, expected {}",
                actual,
                version_range_to_text(range)
            );
        }
    }

    Ok(())
}

fn version_range_to_text(range: VersionRange) -> String {
    match (range.min_inclusive, range.max_exclusive) {
        (Some(min), Some(max)) => format!(">= {min}, < {max}"),
        (Some(min), None) => format!(">= {min}"),
        (None, Some(max)) => format!("< {max}"),
        (None, None) => "any version".to_string(),
    }
}
