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
    tool_requirements_for_language, typescript_tsconfig_requirements, ToolRequirement,
    ToolRequirementCheck, TsConfigSettingRequirement, VersionRange,
};
use crate::app::context::{validated_to_anyhow, BuildContext};
use crate::app::edit;
use crate::app::edit::cargo_toml::{DependencySpec, DependencyTable};
use crate::app::edit::json::collect_object_entries;
use crate::app::edit::tsconfig_json::RequiredSetting;
use crate::fs;
use crate::log::LogColorize;
use crate::model::GuestLanguage;
use crate::sdk_overrides::{sdk_overrides, RustDependency};
use crate::sdk_versions;
use crate::validation::ValidationBuilder;
use anyhow::{anyhow, bail};
use regex::Regex;
use semver::{Version as SemVerVersion, VersionReq};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use version_compare::{Cmp, Version};

#[derive(Clone, Copy, Debug)]
enum PackageJsonSection {
    Dependencies,
    DevDependencies,
}

#[derive(Clone, Copy, Debug)]
enum DependencyMatcherSemantics {
    Rust,
    TypeScript,
}

#[derive(Clone, Debug)]
enum ExpectedDependencyKind {
    ExactPath(String),
    ExactLiteral(String),
    SemanticCompatibleVersion { base_version: String },
}

#[derive(Clone, Debug)]
struct PackageJsonDependencyRequirement {
    name: &'static str,
    section: PackageJsonSection,
    expected: ExpectedDependencyKind,
    semantics: DependencyMatcherSemantics,
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
        let package_step = plan_package_json_fix_step(ctx, overrides, &mut plan.warnings)?;
        if let Some(step) = package_step {
            plan.steps.push(step);
        }

        let tsconfig_steps = plan_tsconfig_fix_steps(ctx)?;
        plan.steps.extend(tsconfig_steps);
    }

    if selected_languages.contains(&GuestLanguage::Rust) {
        let rust_steps = plan_rust_cargo_fix_steps(ctx, overrides, &mut plan.warnings)?;
        plan.steps.extend(rust_steps);
    }

    Ok(plan)
}

fn plan_package_json_fix_step(
    ctx: &BuildContext<'_>,
    overrides: &crate::sdk_overrides::SdkOverrides,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<DependencyFixStep>> {
    let package_json_path = ctx.application().app_root_dir().join("package.json");
    let package_json_str_contents = fs::read_to_string(&package_json_path)?;
    let requirements = typescript_sdk_requirements(overrides);
    let names = requirements.iter().map(|r| r.name).collect::<Vec<_>>();

    let dependencies = collect_object_entries(&package_json_str_contents, "dependencies", &names)?;
    let dev_dependencies =
        collect_object_entries(&package_json_str_contents, "devDependencies", &names)?;

    let mut dependency_updates = Vec::<(String, String)>::new();
    let mut dev_dependency_updates = Vec::<(String, String)>::new();

    for requirement in requirements {
        let expected_value = expected_dependency_value(&requirement.expected);
        let section_versions = match requirement.section {
            PackageJsonSection::Dependencies => &dependencies,
            PackageJsonSection::DevDependencies => &dev_dependencies,
        };

        let compliance = match section_versions
            .get(requirement.name)
            .and_then(|v| v.as_ref())
        {
            Some(raw) => match parse_json_string_literal(raw) {
                Some(found_text) => evaluate_dependency_spec_compliance(
                    found_text.as_str(),
                    &requirement.expected,
                    requirement.semantics,
                ),
                None => DependencySpecCompliance::SkipWarn(format!(
                    "Skipped dependency check for complex package spec '{}'",
                    raw
                )),
            },
            None => DependencySpecCompliance::NeedsUpdate,
        };

        match compliance {
            DependencySpecCompliance::Compatible => {
                // NOP
            }
            DependencySpecCompliance::NeedsUpdate => {
                let update = (requirement.name.to_string(), expected_value);
                match requirement.section {
                    PackageJsonSection::Dependencies => dependency_updates.push(update),
                    PackageJsonSection::DevDependencies => dev_dependency_updates.push(update),
                }
            }
            DependencySpecCompliance::SkipWarn(message) => {
                warnings.push(format!("{} ({})", message, package_json_path.display()));
            }
        }
    }

    if dependency_updates.is_empty() && dev_dependency_updates.is_empty() {
        return Ok(None);
    }

    let new = edit::package_json::merge_dependencies(
        package_json_str_contents.as_str(),
        &dependency_updates,
        &dev_dependency_updates,
    )?;

    Ok(Some(DependencyFixStep {
        path: package_json_path,
        current: package_json_str_contents,
        new,
    }))
}

fn plan_tsconfig_fix_steps(ctx: &BuildContext<'_>) -> anyhow::Result<Vec<DependencyFixStep>> {
    let mut steps = Vec::new();
    let requirements = typescript_tsconfig_requirements()
        .iter()
        .map(to_tsconfig_required_setting)
        .collect::<Vec<_>>();
    let update_source = r#"{
  "compilerOptions": {
    "moduleResolution": "bundler",
    "experimentalDecorators": true,
    "emitDecoratorMetadata": true
  }
}"#;

    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        if component.guess_language() != Some(GuestLanguage::TypeScript) {
            continue;
        }

        let tsconfig_path = component.component_dir().join("tsconfig.json");
        let source = fs::read_to_string(&tsconfig_path)?;
        let missing = edit::tsconfig_json::check_required_settings(source.as_str(), &requirements)?;

        if !missing.is_empty() {
            let new = edit::tsconfig_json::merge_with_newer(source.as_str(), update_source)?;

            steps.push(DependencyFixStep {
                path: tsconfig_path,
                current: source,
                new,
            });
        }
    }

    Ok(steps)
}

fn plan_rust_cargo_fix_steps(
    ctx: &BuildContext<'_>,
    overrides: &crate::sdk_overrides::SdkOverrides,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Vec<DependencyFixStep>> {
    let golem_rust_expected = match overrides.golem_rust_dependency() {
        RustDependency::Path(path) => {
            ExpectedDependencyKind::ExactPath(path.to_string_lossy().to_string())
        }
        RustDependency::Version(version) => ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version: version,
        },
    };

    let mut requirements = vec![
        CargoDependencyRequirement {
            name: "log",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::LOG.to_string(),
                features: vec!["kv".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
        },
        CargoDependencyRequirement {
            name: "serde",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::SERDE.to_string(),
                features: vec!["derive".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
        },
        CargoDependencyRequirement {
            name: "serde_json",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::SERDE_JSON.to_string(),
                features: Vec::new(),
            },
            matcher: CargoDependencyMatcher::Exact,
        },
        CargoDependencyRequirement {
            name: "wstd",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::WSTD.to_string(),
                features: vec!["default".to_string(), "json".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
        },
    ];

    let golem_rust_expected_spec = match &golem_rust_expected {
        ExpectedDependencyKind::ExactPath(path) => DependencySpec::Path {
            path: path.clone(),
            features: vec!["export_golem_agentic".to_string()],
        },
        ExpectedDependencyKind::SemanticCompatibleVersion { base_version } => {
            DependencySpec::Version {
                version: base_version.clone(),
                features: vec!["export_golem_agentic".to_string()],
            }
        }
        ExpectedDependencyKind::ExactLiteral(value) => DependencySpec::Version {
            version: value.clone(),
            features: vec!["export_golem_agentic".to_string()],
        },
    };
    requirements.push(CargoDependencyRequirement {
        name: "golem-rust",
        expected_spec: golem_rust_expected_spec,
        matcher: CargoDependencyMatcher::Kind {
            expected: golem_rust_expected,
            semantics: DependencyMatcherSemantics::Rust,
        },
    });

    let spec_names = requirements
        .iter()
        .map(|requirement| requirement.name)
        .collect::<Vec<_>>();

    let mut steps = Vec::new();
    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        if component.guess_language() != Some(GuestLanguage::Rust) {
            continue;
        }

        let cargo_toml_path = component.component_dir().join("Cargo.toml");
        let original = fs::read_to_string(&cargo_toml_path)?;
        let mut working = original.clone();
        let specs = edit::cargo_toml::collect_dependency_specs(&original, &spec_names)?;

        for requirement in &requirements {
            let found = specs.get(requirement.name).and_then(|spec| spec.as_ref());
            let compliance = evaluate_cargo_dependency_compliance(found, requirement);

            match compliance {
                DependencySpecCompliance::Compatible => {}
                DependencySpecCompliance::SkipWarn(message) => {
                    warnings.push(format!("{} ({})", message, cargo_toml_path.display()));
                }
                DependencySpecCompliance::NeedsUpdate => {
                    working = edit::cargo_toml::upsert_dependency_auto(
                        &working,
                        requirement.name,
                        &requirement.expected_spec,
                        DependencyTable::Dependencies,
                    )?;
                }
            }
        }

        if working != original {
            steps.push(DependencyFixStep {
                path: cargo_toml_path,
                current: original,
                new: working,
            });
        }
    }

    Ok(steps)
}

enum DependencySpecCompliance {
    Compatible,
    NeedsUpdate,
    SkipWarn(String),
}

enum CargoDependencyMatcher {
    Exact,
    Kind {
        expected: ExpectedDependencyKind,
        semantics: DependencyMatcherSemantics,
    },
}

struct CargoDependencyRequirement {
    name: &'static str,
    expected_spec: DependencySpec,
    matcher: CargoDependencyMatcher,
}

fn evaluate_dependency_spec_compliance(
    found_text: &str,
    expected: &ExpectedDependencyKind,
    semantics: DependencyMatcherSemantics,
) -> DependencySpecCompliance {
    match expected {
        ExpectedDependencyKind::ExactPath(expected_path) => {
            if normalize_path_str(found_text) == normalize_path_str(expected_path) {
                DependencySpecCompliance::Compatible
            } else {
                DependencySpecCompliance::NeedsUpdate
            }
        }
        ExpectedDependencyKind::ExactLiteral(expected_literal) => {
            if found_text == expected_literal {
                DependencySpecCompliance::Compatible
            } else {
                DependencySpecCompliance::NeedsUpdate
            }
        }
        ExpectedDependencyKind::SemanticCompatibleVersion { base_version } => {
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

fn evaluate_cargo_dependency_compliance(
    found: Option<&DependencySpec>,
    requirement: &CargoDependencyRequirement,
) -> DependencySpecCompliance {
    let Some(found) = found else {
        return DependencySpecCompliance::NeedsUpdate;
    };

    if !has_required_features(found, &requirement.expected_spec) {
        return DependencySpecCompliance::NeedsUpdate;
    }

    match (&requirement.matcher, found) {
        (_, DependencySpec::Unsupported(raw)) => DependencySpecCompliance::SkipWarn(format!(
            "Skipped dependency check for complex Cargo spec '{}'",
            raw
        )),
        (CargoDependencyMatcher::Exact, _) => {
            if found == &requirement.expected_spec {
                DependencySpecCompliance::Compatible
            } else {
                DependencySpecCompliance::NeedsUpdate
            }
        }
        (
            CargoDependencyMatcher::Kind {
                expected,
                semantics,
            },
            DependencySpec::Version { version, .. },
        ) => evaluate_dependency_spec_compliance(version, expected, *semantics),
        (
            CargoDependencyMatcher::Kind {
                expected,
                semantics,
            },
            DependencySpec::Path { path, .. },
        ) => evaluate_dependency_spec_compliance(path, expected, *semantics),
    }
}

fn has_required_features(found: &DependencySpec, expected: &DependencySpec) -> bool {
    let found_features = match found {
        DependencySpec::Version { features, .. } => features,
        DependencySpec::Path { features, .. } => features,
        DependencySpec::Unsupported(_) => return true,
    };

    let expected_features = match expected {
        DependencySpec::Version { features, .. } => features,
        DependencySpec::Path { features, .. } => features,
        DependencySpec::Unsupported(_) => return true,
    };

    expected_features
        .iter()
        .all(|feature| found_features.contains(feature))
}

fn expected_dependency_value(expected: &ExpectedDependencyKind) -> String {
    match expected {
        ExpectedDependencyKind::ExactPath(path) => path.clone(),
        ExpectedDependencyKind::ExactLiteral(value) => value.clone(),
        ExpectedDependencyKind::SemanticCompatibleVersion { base_version } => base_version.clone(),
    }
}

fn to_tsconfig_required_setting(requirement: &TsConfigSettingRequirement) -> RequiredSetting {
    RequiredSetting {
        path: requirement.path.iter().map(|s| (*s).to_string()).collect(),
        expected_literal: requirement.expected_literal.map(|value| value.to_string()),
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
    let normalized: PathBuf = fs::normalize_path_lexically(Path::new(path));
    normalized.to_string_lossy().to_string()
}

fn parse_json_string_literal(raw: &str) -> Option<String> {
    serde_json::from_str::<String>(raw).ok()
}

fn typescript_sdk_requirements(
    overrides: &crate::sdk_overrides::SdkOverrides,
) -> Vec<PackageJsonDependencyRequirement> {
    let make_expected = |package_name: &str| {
        if overrides.ts_packages_path.is_some() {
            ExpectedDependencyKind::ExactPath(overrides.ts_package_dep(package_name))
        } else {
            ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: overrides.ts_package_dep(package_name),
            }
        }
    };

    vec![
        PackageJsonDependencyRequirement {
            name: "@golemcloud/golem-ts-sdk",
            section: PackageJsonSection::Dependencies,
            expected: make_expected("golem-ts-sdk"),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@golemcloud/golem-ts-typegen",
            section: PackageJsonSection::DevDependencies,
            expected: make_expected("golem-ts-typegen"),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-alias",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::ROLLUP_PLUGIN_ALIAS.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-node-resolve",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::ROLLUP_PLUGIN_NODE_RESOLVE.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-typescript",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::ROLLUP_PLUGIN_TYPESCRIPT.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-commonjs",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::ROLLUP_PLUGIN_COMMONJS.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-json",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::ROLLUP_PLUGIN_JSON.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@types/node",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::TYPES_NODE.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "rollup",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::ROLLUP.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "tslib",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(sdk_versions::ts_dep::TSLIB.to_string()),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "typescript",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::ExactLiteral(
                sdk_versions::ts_dep::TYPESCRIPT.to_string(),
            ),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
    ]
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

#[cfg(test)]
mod test {
    use super::{verify_semantic_version_compatibility, DependencyMatcherSemantics};
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
        assert!(verify_semantic_version_compatibility(
            "2.0.0",
            "3.0.0",
            DependencyMatcherSemantics::Rust,
        )
        .is_err());
    }

    #[test]
    fn dependency_version_check_rejects_semantic_incompatible_requirement() {
        assert!(verify_semantic_version_compatibility(
            "2.0.0",
            "~1.9.0",
            DependencyMatcherSemantics::Rust,
        )
        .is_err());
    }

    #[test]
    fn dependency_version_check_applies_typescript_bare_exact_semantics() {
        assert!(verify_semantic_version_compatibility(
            "2.0.0",
            "2.0.1",
            DependencyMatcherSemantics::TypeScript,
        )
        .is_err());
    }

    #[test]
    fn dependency_version_check_applies_rust_bare_caret_semantics() {
        verify_semantic_version_compatibility("2.0.1", "2.0.0", DependencyMatcherSemantics::Rust)
            .unwrap();
    }
}
