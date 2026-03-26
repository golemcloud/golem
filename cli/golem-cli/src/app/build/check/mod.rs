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
use std::env;
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
    SemanticCompatibleVersion {
        base_version: String,
        use_version_hint: bool,
    },
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
    let cargo_workspace_manifest = cargo_workspace_manifest(ctx)?;
    let requirements = rust_dependency_requirements(overrides);

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
            if cargo_workspace_manifest.is_some()
                && matches!(
                    edit::cargo_toml::resolve_dependency_location(&original, requirement.name)?,
                    Some(edit::cargo_toml::DependencyLocation::WorkspaceDependencies)
                )
            {
                continue;
            }

            let compliance =
                evaluate_cargo_dependency_compliance(found, requirement, cargo_toml_path.parent());

            match compliance {
                DependencySpecCompliance::Compatible => {}
                DependencySpecCompliance::SkipWarn(message) => {
                    warnings.push(format!("{} ({})", message, cargo_toml_path.display()));
                }
                DependencySpecCompliance::NeedsUpdate => {
                    if found.is_none() && !requirement.required {
                        continue;
                    }
                    let update_spec =
                        build_cargo_update_spec(found, requirement, cargo_toml_path.parent());
                    working = edit::cargo_toml::upsert_dependency_auto(
                        &working,
                        requirement.name,
                        &update_spec,
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

    if let Some((workspace_cargo_toml_path, workspace_source)) = cargo_workspace_manifest {
        let mut working = workspace_source.clone();
        let specs = edit::cargo_toml::collect_dependency_specs(&workspace_source, &spec_names)?;

        for requirement in &requirements {
            let found = specs.get(requirement.name).and_then(|spec| spec.as_ref());
            if !matches!(
                edit::cargo_toml::resolve_dependency_location(&workspace_source, requirement.name)?,
                Some(edit::cargo_toml::DependencyLocation::WorkspaceDependencies)
            ) {
                continue;
            }

            let compliance = evaluate_cargo_dependency_compliance(
                found,
                requirement,
                workspace_cargo_toml_path.parent(),
            );

            match compliance {
                DependencySpecCompliance::Compatible => {}
                DependencySpecCompliance::SkipWarn(message) => {
                    warnings.push(format!(
                        "{} ({})",
                        message,
                        workspace_cargo_toml_path.display()
                    ));
                }
                DependencySpecCompliance::NeedsUpdate => {
                    if found.is_none() && !requirement.required {
                        continue;
                    }
                    let update_spec = build_cargo_update_spec(
                        found,
                        requirement,
                        workspace_cargo_toml_path.parent(),
                    );
                    working = edit::cargo_toml::upsert_dependency_in_workspace_dependencies(
                        &working,
                        requirement.name,
                        &update_spec,
                    )?;
                }
            }
        }

        if working != workspace_source {
            steps.push(DependencyFixStep {
                path: workspace_cargo_toml_path,
                current: workspace_source,
                new: working,
            });
        }
    }

    Ok(steps)
}

fn cargo_workspace_manifest(ctx: &BuildContext<'_>) -> anyhow::Result<Option<(PathBuf, String)>> {
    let path = ctx.application().app_root_dir().join("Cargo.toml");
    if !path.exists() {
        return Ok(None);
    }

    let source = fs::read_to_string(&path)?;
    if edit::cargo_toml::is_workspace_manifest(&source)? {
        Ok(Some((path, source)))
    } else {
        Ok(None)
    }
}

fn rust_dependency_requirements(
    overrides: &crate::sdk_overrides::SdkOverrides,
) -> Vec<CargoDependencyRequirement> {
    let golem_rust_expected = match overrides.golem_rust_dependency() {
        RustDependency::Path(path) => {
            ExpectedDependencyKind::ExactPath(path.to_string_lossy().to_string())
        }
        RustDependency::Version(version) => ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version: version,
            use_version_hint: false,
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
            required: false,
        },
        CargoDependencyRequirement {
            name: "serde",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::SERDE.to_string(),
                features: vec!["derive".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        },
        CargoDependencyRequirement {
            name: "serde_json",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::SERDE_JSON.to_string(),
                features: Vec::new(),
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        },
        CargoDependencyRequirement {
            name: "wstd",
            expected_spec: DependencySpec::Version {
                version: sdk_versions::rust_dep::WSTD.to_string(),
                features: vec!["default".to_string(), "json".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        },
    ];

    let golem_rust_expected_spec = match &golem_rust_expected {
        ExpectedDependencyKind::ExactPath(path) => DependencySpec::Path {
            path: path.clone(),
            features: vec![],
        },
        ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version,
            use_version_hint: _,
        } => DependencySpec::Version {
            version: base_version.clone(),
            features: vec![],
        },
    };
    requirements.push(CargoDependencyRequirement {
        name: "golem-rust",
        expected_spec: golem_rust_expected_spec,
        matcher: CargoDependencyMatcher::Kind {
            expected: golem_rust_expected,
            semantics: DependencyMatcherSemantics::Rust,
        },
        required: true,
    });

    requirements
}

#[derive(Debug, PartialEq, Eq)]
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
    required: bool,
}

fn build_cargo_update_spec(
    found: Option<&DependencySpec>,
    requirement: &CargoDependencyRequirement,
    base_dir: Option<&Path>,
) -> DependencySpec {
    let base_spec = match (&requirement.matcher, found) {
        (
            CargoDependencyMatcher::Kind {
                expected,
                semantics,
            },
            Some(DependencySpec::Version { version, features }),
        ) if evaluate_dependency_spec_compliance(version, expected, *semantics)
            == DependencySpecCompliance::Compatible =>
        {
            DependencySpec::Version {
                version: version.clone(),
                features: features.clone(),
            }
        }
        (
            CargoDependencyMatcher::Kind {
                expected,
                semantics,
            },
            Some(DependencySpec::Path { path, features }),
        ) if matches!(expected, ExpectedDependencyKind::ExactPath(expected_path)
            if path_matches_expected(path, expected_path, base_dir))
            || evaluate_dependency_spec_compliance(path, expected, *semantics)
                == DependencySpecCompliance::Compatible =>
        {
            DependencySpec::Path {
                path: path.clone(),
                features: features.clone(),
            }
        }
        _ => requirement.expected_spec.clone(),
    };

    merge_dependency_features(&base_spec, found, Some(&requirement.expected_spec))
}

fn merge_dependency_features(
    base: &DependencySpec,
    found: Option<&DependencySpec>,
    required: Option<&DependencySpec>,
) -> DependencySpec {
    let mut merged = base.clone();

    let expected_features = required
        .map(dependency_features)
        .unwrap_or_else(|| dependency_features(base));
    let found_features = found.map(dependency_features).unwrap_or_default();

    let mut features = Vec::new();
    for feature in expected_features.into_iter().chain(found_features) {
        if !features.iter().any(|f| f == &feature) {
            features.push(feature);
        }
    }

    match &mut merged {
        DependencySpec::Version { features: f, .. } | DependencySpec::Path { features: f, .. } => {
            *f = features;
        }
        DependencySpec::Unsupported(_) => {}
    }

    merged
}

fn dependency_features(spec: &DependencySpec) -> Vec<String> {
    match spec {
        DependencySpec::Version { features, .. } | DependencySpec::Path { features, .. } => {
            features.clone()
        }
        DependencySpec::Unsupported(_) => Vec::new(),
    }
}

fn evaluate_dependency_spec_compliance(
    found_text: &str,
    expected: &ExpectedDependencyKind,
    semantics: DependencyMatcherSemantics,
) -> DependencySpecCompliance {
    match expected {
        ExpectedDependencyKind::ExactPath(expected_path) => {
            if resolve_local_dependency_path(found_text) == resolve_local_dependency_path(expected_path)
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

fn evaluate_cargo_dependency_compliance(
    found: Option<&DependencySpec>,
    requirement: &CargoDependencyRequirement,
    base_dir: Option<&Path>,
) -> DependencySpecCompliance {
    let Some(found) = found else {
        return if requirement.required {
            DependencySpecCompliance::NeedsUpdate
        } else {
            DependencySpecCompliance::Compatible
        };
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
        ) => {
            if let ExpectedDependencyKind::ExactPath(expected_path) = expected {
                if path_matches_expected(path, expected_path, base_dir) {
                    return DependencySpecCompliance::Compatible;
                }
            }

            evaluate_dependency_spec_compliance(path, expected, *semantics)
        }
    }
}

fn path_matches_expected(found: &str, expected: &str, base_dir: Option<&Path>) -> bool {
    let resolve = |value: &str| {
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else if let Some(base_dir) = base_dir {
            base_dir.join(path)
        } else {
            path
        };

        fs::normalize_path_lexically(path.as_path())
    };

    resolve(found) == resolve(expected)
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
        ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version,
            use_version_hint: _,
        } => base_version.clone(),
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
                use_version_hint: false,
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
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::ROLLUP_PLUGIN_ALIAS),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-node-resolve",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::ROLLUP_PLUGIN_NODE_RESOLVE),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-typescript",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::ROLLUP_PLUGIN_TYPESCRIPT),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-commonjs",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::ROLLUP_PLUGIN_COMMONJS),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-json",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::ROLLUP_PLUGIN_JSON),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@types/node",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::TYPES_NODE),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "rollup",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::ROLLUP),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "tslib",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::TSLIB),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "typescript",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(sdk_versions::ts_dep::TYPESCRIPT),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
    ]
}

fn dep_base_version(spec: &str) -> String {
    extract_version(spec).unwrap_or_else(|| spec.to_string())
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
    use super::{
        build_cargo_update_spec, evaluate_cargo_dependency_compliance,
        evaluate_dependency_spec_compliance, rust_dependency_requirements,
        typescript_sdk_requirements, verify_semantic_version_compatibility, CargoDependencyMatcher,
        CargoDependencyRequirement, DependencyMatcherSemantics, DependencySpecCompliance,
        ExpectedDependencyKind, PackageJsonSection,
    };
    use crate::app::edit::cargo_toml::DependencySpec;
    use crate::sdk_overrides::sdk_overrides;
    use pretty_assertions::assert_eq;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;
    use test_r::test;
    use toml_edit::DocumentMut;

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
    fn cargo_update_preserves_existing_features_when_adding_required_ones() {
        let requirement = CargoDependencyRequirement {
            name: "golem-rust",
            expected_spec: DependencySpec::Path {
                path: "/tmp/sdks/rust/golem-rust".to_string(),
                features: vec!["export_golem_agentic".to_string()],
            },
            matcher: CargoDependencyMatcher::Kind {
                expected: ExpectedDependencyKind::ExactPath(
                    "/tmp/sdks/rust/golem-rust".to_string(),
                ),
                semantics: DependencyMatcherSemantics::Rust,
            },
            required: true,
        };

        let found = DependencySpec::Path {
            path: "/tmp/sdks/rust/golem-rust".to_string(),
            features: vec!["extra-feature".to_string()],
        };

        let updated = build_cargo_update_spec(Some(&found), &requirement, None);

        assert_eq!(
            updated,
            DependencySpec::Path {
                path: "/tmp/sdks/rust/golem-rust".to_string(),
                features: vec![
                    "export_golem_agentic".to_string(),
                    "extra-feature".to_string()
                ],
            }
        );
    }

    #[test]
    fn cargo_path_comparison_accepts_relative_path_equivalent_to_absolute_override() {
        let requirement = CargoDependencyRequirement {
            name: "golem-rust",
            expected_spec: DependencySpec::Path {
                path: "/repo/sdks/rust/golem-rust".to_string(),
                features: vec!["export_golem_agentic".to_string()],
            },
            matcher: CargoDependencyMatcher::Kind {
                expected: ExpectedDependencyKind::ExactPath(
                    "/repo/sdks/rust/golem-rust".to_string(),
                ),
                semantics: DependencyMatcherSemantics::Rust,
            },
            required: true,
        };

        let found = DependencySpec::Path {
            path: "../../sdks/rust/golem-rust".to_string(),
            features: vec!["export_golem_agentic".to_string()],
        };

        let compliance = evaluate_cargo_dependency_compliance(
            Some(&found),
            &requirement,
            Some(std::path::Path::new(
                "/repo/test-components/oplog-processor",
            )),
        );

        assert_eq!(compliance, DependencySpecCompliance::Compatible);
    }

    #[test]
    fn ts_template_and_check_requirements_match() {
        let template_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/ts/common/package.json");
        let template_source = std::fs::read_to_string(&template_path).unwrap();
        let template_json: serde_json::Value = serde_json::from_str(&template_source).unwrap();

        let template_deps = template_json["dependencies"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let template_dev_deps = template_json["devDependencies"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();

        let overrides = sdk_overrides().unwrap();
        let mut check_deps = BTreeSet::new();
        let mut check_dev_deps = BTreeSet::new();

        for requirement in typescript_sdk_requirements(overrides) {
            match requirement.section {
                PackageJsonSection::Dependencies => {
                    check_deps.insert(requirement.name.to_string());
                }
                PackageJsonSection::DevDependencies => {
                    check_dev_deps.insert(requirement.name.to_string());
                }
            }
        }

        assert_eq!(check_deps, template_deps);
        assert_eq!(check_dev_deps, template_dev_deps);
    }

    #[test]
    fn rust_template_and_check_requirements_match() {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("templates/rust/component/component-dir/Cargo.toml._");
        let template_source = std::fs::read_to_string(&template_path)
            .unwrap()
            .replace("GOLEM_RUST_VERSION_OR_PATH", "version = \"0.0.0\"");

        let doc: DocumentMut = template_source.parse().unwrap();
        let deps = doc["dependencies"].as_table_like().unwrap();

        let template_names = deps
            .iter()
            .map(|(k, _)| k.to_string())
            .collect::<BTreeSet<_>>();

        let overrides = sdk_overrides().unwrap();
        let requirements = rust_dependency_requirements(overrides);
        let check_names = requirements
            .iter()
            .map(|r| r.name.to_string())
            .collect::<BTreeSet<_>>();

        assert_eq!(check_names, template_names);

        let required_features = requirements
            .iter()
            .filter_map(|requirement| match &requirement.expected_spec {
                DependencySpec::Version { features, .. }
                | DependencySpec::Path { features, .. }
                    if !features.is_empty() =>
                {
                    Some((requirement.name.to_string(), features.clone()))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();

        for (dep_name, required) in required_features {
            let item = deps.get(dep_name.as_str()).unwrap();
            let found = item
                .as_table_like()
                .and_then(|table| table.get("features"))
                .and_then(|features| features.as_array())
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            for feature in required {
                assert!(
                    found.contains(&feature),
                    "missing feature '{}' for dependency '{}' in rust template",
                    feature,
                    dep_name
                );
            }
        }
    }

    #[test]
    fn rust_common_on_demand_template_uses_relative_target_and_no_cargo_target_dir_env() {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("templates/rust/common-on-demand/golem.yaml");
        let template_source = std::fs::read_to_string(&template_path).unwrap();

        assert!(template_source.contains("target/wasm32-wasip2/debug"));
        assert!(template_source.contains("target/wasm32-wasip2/release"));
        assert!(!template_source.contains("CARGO_TARGET_DIR"));
    }

    #[test]
    fn optional_rust_dependency_absent_is_accepted() {
        let requirement = CargoDependencyRequirement {
            name: "serde",
            expected_spec: DependencySpec::Version {
                version: "1".to_string(),
                features: vec!["derive".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        };

        let compliance = evaluate_cargo_dependency_compliance(None, &requirement, None);
        assert_eq!(compliance, DependencySpecCompliance::Compatible);
    }

    #[test]
    fn ts_path_comparison_accepts_relative_file_path_equivalent_to_absolute_override() {
        let cwd = std::env::current_dir().unwrap();
        let found = format!(
            "file:{}",
            cwd.join("sdks/ts/packages/golem-ts-sdk")
                .to_string_lossy()
                .replace("/workspace/golem-alt-00/", "/workspace/golem-alt-00/../golem-alt-00/")
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
