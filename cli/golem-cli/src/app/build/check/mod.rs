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

mod agents;
mod requirements;
mod rust;
mod ts;

use crate::app::build::check::requirements::{
    ToolRequirement, ToolRequirementCheck, VersionRange, tool_requirements_for_language,
};
use crate::app::context::{BuildContext, validated_to_anyhow};
use crate::fs;
use crate::log::LogColorize;
use crate::model::GuestLanguage;
use crate::sdk_overrides::sdk_overrides;
use crate::validation::ValidationBuilder;
use anyhow::{anyhow, bail};
use regex::Regex;
use semver::{Version as SemVerVersion, VersionReq};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use version_compare::{Cmp, Version};

#[derive(Clone, Copy, Debug)]
enum DependencyMatcherSemantics {
    Rust,
    TypeScript,
}

#[derive(Clone, Debug)]
enum ExpectedDependencyKind {
    ExactPath(String),
    SemanticCompatibleVersion {
        base_version: String,
        use_version_hint: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum DependencySpecCompliance {
    Compatible,
    NeedsUpdate,
    SkipWarn(String),
}

#[derive(Clone, Debug)]
pub struct DependencyFixStep {
    pub path: PathBuf,
    pub current: String,
    pub new: String,
}

#[derive(Clone, Debug, Default)]
pub struct DependencyFixPlan {
    pub steps: Vec<DependencyFixStep>,
    pub warnings: Vec<String>,
}

impl DependencyFixPlan {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

pub async fn check_build_tool_requirements(ctx: &BuildContext<'_>) -> anyhow::Result<()> {
    let selected_languages = selected_component_languages(ctx);

    if selected_languages.is_empty() {
        return Ok(());
    }

    let mut requirements = BTreeMap::<&'static str, ToolRequirement>::new();
    for language in &selected_languages {
        for requirement in tool_requirements_for_language(*language) {
            requirements.insert(requirement.key, *requirement);
        }
    }

    let app_root_dir = ctx.application().app_root_dir();
    let mut validation = ValidationBuilder::new();

    for requirement in requirements.into_values() {
        validation.with_context(vec![("tool", requirement.name.to_string())], |validation| {
            if let Err(error) = check_tool_requirement(app_root_dir, requirement) {
                validation.add_error(error.to_string());
            }
        });
    }

    validated_to_anyhow(
        "Build tool requirements check failed",
        validation.build(()),
        None,
    )
}

pub fn plan_dependency_fixes(ctx: &BuildContext<'_>) -> anyhow::Result<DependencyFixPlan> {
    let selected_languages = selected_component_languages(ctx);
    if selected_languages.is_empty() {
        return Ok(DependencyFixPlan::default());
    }

    let overrides = sdk_overrides()?;

    let mut plan = DependencyFixPlan::default();

    if selected_languages.contains(&GuestLanguage::TypeScript) {
        let package_step = ts::plan_package_json_fix_step(ctx, overrides, &mut plan.warnings)?;
        if let Some(step) = package_step {
            plan.steps.push(step);
        }

        let tsconfig_steps = ts::plan_tsconfig_fix_steps(ctx)?;
        plan.steps.extend(tsconfig_steps);
    }

    if selected_languages.contains(&GuestLanguage::Rust) {
        let rust_steps = rust::plan_rust_cargo_fix_steps(ctx, overrides, &mut plan.warnings)?;
        plan.steps.extend(rust_steps);
    }

    if let Some(step) = agents::plan_agents_md_fix_step(ctx, &selected_languages)? {
        plan.steps.push(step);
    }

    Ok(plan)
}

fn selected_component_languages(ctx: &BuildContext<'_>) -> BTreeSet<GuestLanguage> {
    ctx.application_context()
        .selected_component_names()
        .iter()
        .filter_map(|component_name| ctx.application().component(component_name).guess_language())
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
                "{} is not available: {}\nHint: {}",
                requirement.name.log_color_error_highlight(),
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

    let installed_targets_output = String::from_utf8_lossy(&output.stdout);
    let installed_targets = installed_targets_output
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

fn evaluate_dependency_spec_compliance(
    found_text: &str,
    expected: &ExpectedDependencyKind,
    semantics: DependencyMatcherSemantics,
) -> DependencySpecCompliance {
    match expected {
        ExpectedDependencyKind::ExactPath(expected_path) => {
            if resolve_local_dependency_path(found_text)
                == resolve_local_dependency_path(expected_path)
            {
                DependencySpecCompliance::Compatible
            } else {
                DependencySpecCompliance::NeedsUpdate
            }
        }
        ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version,
            use_version_hint,
        } => {
            if *use_version_hint {
                let Some(actual_version) = extract_version(found_text) else {
                    return DependencySpecCompliance::SkipWarn(format!(
                        "Skipped dependency check for complex version spec '{}'",
                        found_text
                    ));
                };

                if verify_semantic_version_compatible_with_base(base_version, &actual_version)
                    .is_ok()
                {
                    return DependencySpecCompliance::Compatible;
                }

                return DependencySpecCompliance::NeedsUpdate;
            }

            if !looks_semver_compatible_spec(found_text) {
                return DependencySpecCompliance::SkipWarn(format!(
                    "Skipped dependency check for complex version spec '{}'",
                    found_text
                ));
            }

            if verify_semantic_version_compatibility(base_version, found_text, semantics).is_ok() {
                DependencySpecCompliance::Compatible
            } else {
                DependencySpecCompliance::NeedsUpdate
            }
        }
    }
}

fn verify_semantic_version_compatible_with_base(
    base_version: &str,
    actual_version: &str,
) -> anyhow::Result<()> {
    let expected_version = SemVerVersion::parse(base_version).map_err(|err| {
        anyhow!(
            "expected version '{}' has invalid semver format: {}",
            base_version,
            err
        )
    })?;
    let actual_version = SemVerVersion::parse(actual_version).map_err(|err| {
        anyhow!(
            "detected version '{}' has invalid semver format: {}",
            actual_version,
            err
        )
    })?;

    let compatible_with_expected =
        VersionReq::parse(&format!("^{}", expected_version)).map_err(|err| {
            anyhow!(
                "failed to build semantic compatibility requirement from '{}': {}",
                expected_version,
                err
            )
        })?;

    if compatible_with_expected.matches(&actual_version) {
        Ok(())
    } else {
        bail!(
            "semantic version incompatibility: detected {}, expected compatible with ^{}",
            actual_version,
            expected_version
        )
    }
}

fn verify_semantic_version_compatibility(
    base_version: &str,
    actual_spec: &str,
    semantics: DependencyMatcherSemantics,
) -> anyhow::Result<()> {
    let expected_version = SemVerVersion::parse(base_version).map_err(|err| {
        anyhow!(
            "expected version '{}' has invalid semver format: {}",
            base_version,
            err
        )
    })?;

    let actual_req = parse_semver_requirement(actual_spec, semantics)?;

    if actual_req.matches(&expected_version) {
        Ok(())
    } else {
        bail!(
            "semantic version incompatibility: requirement '{}' does not match expected base version {}",
            actual_spec,
            expected_version
        )
    }
}

fn parse_semver_requirement(
    spec: &str,
    semantics: DependencyMatcherSemantics,
) -> anyhow::Result<VersionReq> {
    if SemVerVersion::parse(spec).is_ok() {
        let normalized = match semantics {
            DependencyMatcherSemantics::Rust => format!("^{spec}"),
            DependencyMatcherSemantics::TypeScript => format!("={spec}"),
        };

        return VersionReq::parse(normalized.as_str()).map_err(|err| {
            anyhow!(
                "invalid semantic version requirement '{}': {}",
                normalized,
                err
            )
        });
    }

    VersionReq::parse(spec)
        .map_err(|err| anyhow!("invalid semantic version requirement '{}': {}", spec, err))
}

fn looks_semver_compatible_spec(spec: &str) -> bool {
    SemVerVersion::parse(spec).is_ok() || VersionReq::parse(spec).is_ok()
}

fn normalize_path_str(path: &str) -> String {
    let path = path
        .strip_prefix("file://")
        .or_else(|| path.strip_prefix("file:"))
        .unwrap_or(path);
    let normalized: PathBuf = fs::normalize_path_lexically(Path::new(path));
    normalized.to_string_lossy().to_string()
}

fn resolve_local_dependency_path(path: &str) -> String {
    let normalized = normalize_path_str(path);
    let path = PathBuf::from(&normalized);
    if path.is_absolute() {
        return normalized;
    }

    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize_path_str(cwd.join(path).to_string_lossy().as_ref())
}

fn expected_dependency_value(expected: &ExpectedDependencyKind) -> String {
    match expected {
        ExpectedDependencyKind::ExactPath(path) => path.clone(),
        ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version,
            use_version_hint: _,
        } => base_version.clone(),
    }
}

#[cfg(test)]
mod test {
    use super::{
        DependencyMatcherSemantics, DependencySpecCompliance, ExpectedDependencyKind,
        evaluate_dependency_spec_compliance, verify_semantic_version_compatibility,
    };
    use pretty_assertions::assert_eq;
    use test_r::test;

    #[test]
    fn dependency_version_check_accepts_semantic_compatible_version() {
        verify_semantic_version_compatibility("2.3.4", "2.0.1", DependencyMatcherSemantics::Rust)
            .unwrap();
    }

    #[test]
    fn dependency_version_check_accepts_semantic_compatible_requirement() {
        verify_semantic_version_compatibility("2.3.1", "^2.0.0", DependencyMatcherSemantics::Rust)
            .unwrap();
    }

    #[test]
    fn dependency_version_check_rejects_semantic_incompatible_version() {
        assert!(
            verify_semantic_version_compatibility(
                "2.0.0",
                "3.0.0",
                DependencyMatcherSemantics::Rust,
            )
            .is_err()
        );
    }

    #[test]
    fn dependency_version_check_rejects_semantic_incompatible_requirement() {
        assert!(
            verify_semantic_version_compatibility(
                "2.0.0",
                "~1.9.0",
                DependencyMatcherSemantics::Rust,
            )
            .is_err()
        );
    }

    #[test]
    fn dependency_version_check_applies_typescript_bare_exact_semantics() {
        assert!(
            verify_semantic_version_compatibility(
                "2.0.0",
                "2.0.1",
                DependencyMatcherSemantics::TypeScript,
            )
            .is_err()
        );
    }

    #[test]
    fn dependency_version_check_applies_rust_bare_caret_semantics() {
        verify_semantic_version_compatibility("2.0.1", "2.0.0", DependencyMatcherSemantics::Rust)
            .unwrap();
    }

    #[test]
    fn ts_non_sdk_semantic_check_accepts_newer_compatible_range_by_version_hint() {
        let compliance = evaluate_dependency_spec_compliance(
            "^4.60.1",
            &ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: "4.50.1".to_string(),
                use_version_hint: true,
            },
            DependencyMatcherSemantics::TypeScript,
        );

        assert_eq!(compliance, DependencySpecCompliance::Compatible);
    }

    #[test]
    fn ts_path_comparison_accepts_relative_file_path_equivalent_to_absolute_override() {
        let cwd = std::env::current_dir().unwrap();
        let found = format!(
            "file:{}",
            cwd.join("sdks/ts/packages/golem-ts-sdk")
                .to_string_lossy()
                .replace(
                    "/workspace/golem-alt-00/",
                    "/workspace/golem-alt-00/../golem-alt-00/"
                )
        );
        let expected = format!(
            "file:{}",
            cwd.join("sdks/ts/packages/golem-ts-sdk").to_string_lossy()
        );

        let compliance = evaluate_dependency_spec_compliance(
            &found,
            &ExpectedDependencyKind::ExactPath(expected),
            DependencyMatcherSemantics::TypeScript,
        );

        assert_eq!(compliance, DependencySpecCompliance::Compatible);
    }
}
