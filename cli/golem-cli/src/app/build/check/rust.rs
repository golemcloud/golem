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

use crate::app::build::check::{
    DependencyFixStep, DependencyMatcherSemantics, DependencySpecCompliance,
    ExpectedDependencyKind, evaluate_dependency_spec_compliance,
};
use crate::app::context::BuildContext;
use crate::app::edit;
use crate::app::edit::cargo_toml::{DependencySpec, DependencyTable};
use crate::bridge_gen::{BridgeMode, bridge_client_directory_name_for_mode};
use crate::fs;
use crate::model::GuestLanguage;
use crate::sdk_overrides::{RustDependency, SdkOverrides};
use crate::versions;
use golem_common::model::agent::AgentTypeName;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(super) fn plan_rust_cargo_fix_steps(
    ctx: &BuildContext<'_>,
    overrides: &SdkOverrides,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Vec<DependencyFixStep>> {
    let cargo_workspace_manifest = cargo_workspace_manifest(ctx)?;
    let requirements = rust_dependency_requirements(overrides);

    let spec_names = requirements
        .iter()
        .map(|requirement| requirement.name)
        .collect::<Vec<_>>();

    let guest_targets = explicit_rust_guest_bridge_targets(ctx);

    let mut steps = Vec::new();
    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        let is_rust = component.guess_language() == Some(GuestLanguage::Rust);

        let cargo_toml_path = component.component_dir().join("Cargo.toml");

        // Non-Rust components only receive guest bridge dependency fixes, and only when they
        // already have a Cargo.toml (they may consume a generated Rust guest client crate).
        if !is_rust && (guest_targets.is_empty() || !cargo_toml_path.exists()) {
            continue;
        }

        let original = fs::read_to_string(&cargo_toml_path)?;
        let mut working = original.clone();

        if is_rust {
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

                let compliance = evaluate_cargo_dependency_compliance(
                    found,
                    requirement,
                    cargo_toml_path.parent(),
                )?;

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
                            build_cargo_update_spec(found, requirement, cargo_toml_path.parent())?;
                        working = edit::cargo_toml::upsert_dependency_auto(
                            &working,
                            requirement.name,
                            &update_spec,
                            DependencyTable::Dependencies,
                        )?;
                    }
                }
            }
        }

        if !guest_targets.is_empty() {
            apply_guest_bridge_dependency_fixes(
                &mut working,
                GuestDependencyScope::Package,
                component.component_dir(),
                &cargo_toml_path,
                &guest_targets,
                warnings,
            )?;
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
        // A root Cargo.toml can be both a package manifest and the workspace manifest, so the
        // component loop above may already have produced a fix step for this same path. Continue
        // from that step's edited content and merge into it, otherwise the two steps would be
        // applied in sequence and the later one would overwrite the earlier one's changes.
        let existing_step_index = steps
            .iter()
            .position(|step| step.path == workspace_cargo_toml_path);
        let mut working = match existing_step_index {
            Some(index) => steps[index].new.clone(),
            None => workspace_source.clone(),
        };
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
            )?;

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
                    )?;
                    working = edit::cargo_toml::upsert_dependency_in_workspace_dependencies(
                        &working,
                        requirement.name,
                        &update_spec,
                    )?;
                }
            }
        }

        if !guest_targets.is_empty() {
            let workspace_root = workspace_cargo_toml_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            apply_guest_bridge_dependency_fixes(
                &mut working,
                GuestDependencyScope::Workspace,
                workspace_root,
                &workspace_cargo_toml_path,
                &guest_targets,
                warnings,
            )?;
        }

        match existing_step_index {
            Some(index) => {
                steps[index].new = working;
            }
            None => {
                if working != workspace_source {
                    steps.push(DependencyFixStep {
                        path: workspace_cargo_toml_path,
                        current: workspace_source,
                        new: working,
                    });
                }
            }
        }
    }

    Ok(steps)
}

/// A guest bridge client crate that an explicit `bridge.rust.guest.agents` entry generates, plus
/// the directory it is generated into.
struct RustGuestBridgeTarget {
    crate_name: String,
    output_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuestDependencyScope {
    Package,
    Workspace,
}

/// Collects the explicit Rust guest bridge targets whose generated crate name and output directory
/// are known before any component is built.
///
/// Only direct agent-type matchers are returned: `*` and application component names are skipped
/// because their concrete agent-type names (and therefore crate names) are only known after the
/// producing components are built and their agent metadata is extracted.
fn explicit_rust_guest_bridge_targets(ctx: &BuildContext<'_>) -> Vec<RustGuestBridgeTarget> {
    let Some(rust_targets) = ctx
        .application()
        .bridge_sdks()
        .for_language(GuestLanguage::Rust)
    else {
        return Vec::new();
    };
    let Some(guest) = &rust_targets.guest else {
        return Vec::new();
    };

    let component_names = ctx
        .application()
        .component_names()
        .map(|component_name| component_name.as_str().to_string())
        .collect::<BTreeSet<_>>();

    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for matcher in guest.agents.clone().into_set() {
        if matcher == "*" || component_names.contains(&matcher) {
            continue;
        }
        if !seen.insert(matcher.clone()) {
            continue;
        }

        let agent_type_name = AgentTypeName(matcher);
        targets.push(RustGuestBridgeTarget {
            crate_name: bridge_client_directory_name_for_mode(&agent_type_name, BridgeMode::Guest),
            output_dir: ctx.application().bridge_sdk_dir(
                &agent_type_name,
                GuestLanguage::Rust,
                BridgeMode::Guest,
            ),
        });
    }
    targets
}

/// Normalizes existing Cargo dependencies that reference a generated Rust guest client crate so
/// that their `path` points at the canonical generated output directory.
///
/// This only touches dependencies that already exist (by crate identity): it never adds a guest
/// client dependency to a component that does not already reference it, because the manifest does
/// not encode which component consumes which guest bridge.
fn apply_guest_bridge_dependency_fixes(
    working: &mut String,
    scope: GuestDependencyScope,
    base_dir: &Path,
    manifest_path: &Path,
    guest_targets: &[RustGuestBridgeTarget],
    warnings: &mut Vec<String>,
) -> anyhow::Result<()> {
    let crate_names = guest_targets
        .iter()
        .map(|target| target.crate_name.clone())
        .collect::<BTreeSet<_>>();

    let matches = match scope {
        GuestDependencyScope::Package => {
            edit::cargo_toml::find_package_dependencies_by_crate_name(working, &crate_names)?
        }
        GuestDependencyScope::Workspace => {
            edit::cargo_toml::find_workspace_dependencies_by_crate_name(working, &crate_names)?
        }
    };

    let base_dir_abs = fs::absolute_lexical_path(base_dir)?;

    for matched in matches {
        let Some(target) = guest_targets
            .iter()
            .find(|target| target.crate_name == matched.crate_name)
        else {
            continue;
        };
        let expected_abs = fs::absolute_lexical_path(&target.output_dir)?;

        match &matched.spec {
            DependencySpec::Path { path, .. } => {
                let found_abs =
                    fs::absolute_lexical_path_from_base_dir(Path::new(path), &base_dir_abs);
                if found_abs == expected_abs {
                    continue;
                }
                let new_path = relative_path_unix(&base_dir_abs, &expected_abs)?;
                *working = edit::cargo_toml::set_dependency_path(
                    working,
                    matched.location,
                    &matched.key,
                    &new_path,
                )?;
            }
            DependencySpec::Version { .. } => {
                let new_path = relative_path_unix(&base_dir_abs, &expected_abs)?;
                *working = edit::cargo_toml::set_dependency_path(
                    working,
                    matched.location,
                    &matched.key,
                    &new_path,
                )?;
            }
            DependencySpec::Unsupported(raw) => {
                warnings.push(format!(
                    "Skipped guest bridge dependency fix for {} with complex Cargo spec '{}' ({})",
                    matched.crate_name,
                    raw,
                    manifest_path.display()
                ));
            }
        }
    }

    Ok(())
}

/// Computes a Unix-style relative path from `from_dir_abs` to `to_abs`. Both inputs must be
/// absolute, lexically normalized paths.
fn relative_path_unix(from_dir_abs: &Path, to_abs: &Path) -> anyhow::Result<String> {
    let mut target_components = to_abs.components().peekable();
    let mut base_components = from_dir_abs.components().peekable();

    while let (Some(target), Some(base)) = (target_components.peek(), base_components.peek()) {
        if target == base {
            target_components.next();
            base_components.next();
        } else {
            break;
        }
    }

    let mut relative = PathBuf::new();
    for component in base_components {
        if matches!(component, Component::Normal(_) | Component::ParentDir) {
            relative.push("..");
        }
    }
    for component in target_components {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }

    fs::path_to_unix_str(&relative)
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

fn rust_dependency_requirements(overrides: &SdkOverrides) -> Vec<CargoDependencyRequirement> {
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
                version: versions::rust_dep::LOG.to_string(),
                features: vec!["kv".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        },
        CargoDependencyRequirement {
            name: "serde",
            expected_spec: DependencySpec::Version {
                version: versions::rust_dep::SERDE.to_string(),
                features: vec!["derive".to_string()],
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        },
        CargoDependencyRequirement {
            name: "serde_json",
            expected_spec: DependencySpec::Version {
                version: versions::rust_dep::SERDE_JSON.to_string(),
                features: Vec::new(),
            },
            matcher: CargoDependencyMatcher::Exact,
            required: false,
        },
        CargoDependencyRequirement {
            name: "wstd",
            expected_spec: DependencySpec::Version {
                version: versions::rust_dep::WSTD.to_string(),
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
) -> anyhow::Result<DependencySpec> {
    let base_spec = match (&requirement.matcher, found) {
        (
            CargoDependencyMatcher::Kind {
                expected,
                semantics,
            },
            Some(DependencySpec::Version { version, features }),
        ) if evaluate_dependency_spec_compliance(version, expected, *semantics)?
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
            || evaluate_dependency_spec_compliance(path, expected, *semantics)?
                == DependencySpecCompliance::Compatible =>
        {
            DependencySpec::Path {
                path: path.clone(),
                features: features.clone(),
            }
        }
        _ => requirement.expected_spec.clone(),
    };

    Ok(merge_dependency_features(
        &base_spec,
        found,
        Some(&requirement.expected_spec),
    ))
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

fn evaluate_cargo_dependency_compliance(
    found: Option<&DependencySpec>,
    requirement: &CargoDependencyRequirement,
    base_dir: Option<&Path>,
) -> anyhow::Result<DependencySpecCompliance> {
    let Some(found) = found else {
        return Ok(if requirement.required {
            DependencySpecCompliance::NeedsUpdate
        } else {
            DependencySpecCompliance::Compatible
        });
    };

    if !has_required_features(found, &requirement.expected_spec) {
        return Ok(DependencySpecCompliance::NeedsUpdate);
    }

    Ok(match (&requirement.matcher, found) {
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
        ) => evaluate_dependency_spec_compliance(version, expected, *semantics)?,
        (
            CargoDependencyMatcher::Kind {
                expected,
                semantics,
            },
            DependencySpec::Path { path, .. },
        ) => {
            if let ExpectedDependencyKind::ExactPath(expected_path) = expected
                && path_matches_expected(path, expected_path, base_dir)
            {
                return Ok(DependencySpecCompliance::Compatible);
            }

            evaluate_dependency_spec_compliance(path, expected, *semantics)?
        }
    })
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

#[cfg(test)]
mod test {
    use super::{
        CargoDependencyMatcher, CargoDependencyRequirement, GuestDependencyScope,
        RustGuestBridgeTarget, apply_guest_bridge_dependency_fixes, build_cargo_update_spec,
        evaluate_cargo_dependency_compliance, plan_rust_cargo_fix_steps, relative_path_unix,
        rust_dependency_requirements,
    };
    use crate::app::context::{ApplicationContext, BuildContext};
    use crate::app::edit::cargo_toml::DependencySpec;
    use crate::app::template::TEMPLATES_DIR;
    use crate::model::app::{
        Application, ApplicationComponentSelectMode, ApplicationConfig, ApplicationPreload,
        ApplicationSourceMode, BuildConfig, ComponentPresetSelector, LoadedRawApps,
    };
    use crate::model::app_raw;
    use crate::sdk_overrides::sdk_overrides;
    use golem_common::model::environment::EnvironmentName;
    use pretty_assertions::assert_eq;
    use std::collections::{BTreeMap, BTreeSet};
    use test_r::test;
    use toml_edit::DocumentMut;

    #[test]
    fn cargo_update_preserves_existing_features_when_adding_required_ones() {
        let requirement = CargoDependencyRequirement {
            name: "golem-rust",
            expected_spec: DependencySpec::Path {
                path: "/tmp/sdks/rust/golem-rust".to_string(),
                features: vec!["export_golem_agentic".to_string()],
            },
            matcher: CargoDependencyMatcher::Kind {
                expected: crate::app::build::check::ExpectedDependencyKind::ExactPath(
                    "/tmp/sdks/rust/golem-rust".to_string(),
                ),
                semantics: crate::app::build::check::DependencyMatcherSemantics::Rust,
            },
            required: true,
        };

        let found = DependencySpec::Path {
            path: "/tmp/sdks/rust/golem-rust".to_string(),
            features: vec!["extra-feature".to_string()],
        };

        let updated = build_cargo_update_spec(Some(&found), &requirement, None).unwrap();

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
                expected: crate::app::build::check::ExpectedDependencyKind::ExactPath(
                    "/repo/sdks/rust/golem-rust".to_string(),
                ),
                semantics: crate::app::build::check::DependencyMatcherSemantics::Rust,
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
        )
        .unwrap();

        assert_eq!(
            compliance,
            crate::app::build::check::DependencySpecCompliance::Compatible
        );
    }

    #[test]
    fn rust_template_and_check_requirements_match() {
        let template_source = TEMPLATES_DIR
            .get_file("rust/component/component-dir/Cargo.toml._")
            .unwrap()
            .contents_utf8()
            .unwrap()
            .to_string()
            .replace("GOLEM_RUST_VERSION_OR_PATH", "version = \"0.0.0\"");

        let doc: DocumentMut = template_source.parse().unwrap();
        let deps = doc["dependencies"].as_table_like().unwrap();

        let template_names = deps
            .iter()
            .map(|(k, _): (&str, _)| k.to_string())
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
                .and_then(|table: &dyn toml_edit::TableLike| table.get("features"))
                .and_then(|features: &toml_edit::Item| features.as_array())
                .map(|array: &toml_edit::Array| {
                    array
                        .iter()
                        .filter_map(|v: &toml_edit::Value| v.as_str())
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
        let template_source = TEMPLATES_DIR
            .get_file("rust/common-on-demand/golem.yaml")
            .unwrap()
            .contents_utf8()
            .unwrap()
            .to_string();

        assert!(template_source.contains("{{ cargoTarget }}/wasm32-wasip2/debug"));
        assert!(template_source.contains("{{ cargoTarget }}/wasm32-wasip2/release"));
        assert!(!template_source.contains("CARGO_TARGET_DIR"));

        // The whole cargo target directory must never be a `clean` target: with a
        // redirected/shared CARGO_TARGET_DIR it can be the directory that holds the
        // golem-cli binary itself (and every other component's build), so cleaning it
        // would delete unrelated artifacts. The per-component wasm files are already
        // cleaned via `componentWasm`, `outputWasm`, and the build `targets`.
        assert!(!template_source.contains("- \"{{ cargoTarget }}\""));
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

        let compliance = evaluate_cargo_dependency_compliance(None, &requirement, None).unwrap();
        assert_eq!(
            compliance,
            crate::app::build::check::DependencySpecCompliance::Compatible
        );
    }

    fn guest_target(output_dir: &std::path::Path) -> RustGuestBridgeTarget {
        RustGuestBridgeTarget {
            crate_name: "bar-agent-guest-client".to_string(),
            output_dir: output_dir.to_path_buf(),
        }
    }

    fn run_guest_fix(
        source: &str,
        scope: GuestDependencyScope,
        base_dir: &std::path::Path,
        targets: &[RustGuestBridgeTarget],
    ) -> (String, Vec<String>) {
        let mut working = source.to_string();
        let mut warnings = Vec::new();
        apply_guest_bridge_dependency_fixes(
            &mut working,
            scope,
            base_dir,
            &base_dir.join("Cargo.toml"),
            targets,
            &mut warnings,
        )
        .unwrap();
        (working, warnings)
    }

    #[test]
    async fn plan_rust_cargo_fix_steps_merges_guest_updates_for_root_package_workspace_manifest() {
        let root = tempfile::tempdir().unwrap();
        let golem_yaml_path = root.path().join("golem.yaml");

        std::fs::write(
            root.path().join("Cargo.toml"),
            indoc::indoc! {r#"
                [package]
                name = "consumer"
                version = "0.1.0"
                edition = "2021"

                [workspace]
                members = ["."]
                resolver = "2"

                [dependencies]
                bar-agent-guest-client = { path = "wrong-package/bar-agent-guest-client" }

                [workspace.dependencies]
                bar = { package = "bar-agent-guest-client", path = "wrong-workspace/bar-agent-guest-client" }
            "#},
        )
        .unwrap();
        std::fs::write(root.path().join("src.rs"), "").unwrap();
        std::fs::write(
            &golem_yaml_path,
            indoc::formatdoc! {r#"
                manifestVersion: {}

                app: root-package-workspace-guest-fix

                environments:
                  local:
                    server: local

                components:
                  app:consumer:
                    dir: .
                    templates: rust
                    componentWasm: consumer.wasm
                    outputWasm: consumer-final.wasm

                bridge:
                  rust:
                    guest:
                      agents: BarAgent
                      outputDir: bridge
            "#, crate::versions::sdk::MANIFEST},
        )
        .unwrap();

        let raw_apps = vec![
            app_raw::ApplicationWithSource::from_yaml_file(&golem_yaml_path)
                .expect("raw manifest should parse"),
            app_raw::ApplicationWithSource::from_yaml_file(
                &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("templates/rust/common-on-demand/golem.yaml"),
            )
            .expect("rust template manifest should parse"),
        ];
        let (preload, warns, errors) = Application::preload_from_raw_apps(&raw_apps).into_product();
        assert!(warns.is_empty(), "\n{}", warns.join("\n\n"));
        assert!(errors.is_empty(), "\n{}", errors.join("\n\n"));
        let Some(ApplicationPreload {
            application_name,
            environments,
            local_server,
        }) = preload
        else {
            panic!("expected application preload")
        };

        let mut app_ctx = ApplicationContext::new(
            ApplicationSourceMode::Preloaded(LoadedRawApps {
                app_root_dir: root.path().to_path_buf(),
                calling_working_dir: root.path().to_path_buf(),
                raw_apps,
            }),
            ApplicationConfig {
                offline: true,
                dev_mode: false,
                should_colorize: false,
                enable_wasmtime_fs_cache: false,
            },
            application_name,
            environments,
            local_server,
            ComponentPresetSelector {
                environment: EnvironmentName("local".to_string()),
                presets: Vec::new(),
            },
            reqwest::Client::new(),
        )
        .await
        .unwrap()
        .expect("application context should load");
        app_ctx
            .select_components(&ApplicationComponentSelectMode::All)
            .unwrap();

        let build_config = BuildConfig::new();
        let mut warnings = Vec::new();
        let steps = plan_rust_cargo_fix_steps(
            &BuildContext::new(&app_ctx, &build_config),
            sdk_overrides().unwrap(),
            &mut warnings,
        )
        .unwrap();

        let cargo_toml_path = root.path().join("Cargo.toml");
        let mut applied = std::fs::read_to_string(&cargo_toml_path).unwrap();
        for step in steps.iter().filter(|step| step.path == cargo_toml_path) {
            applied = step.new.clone();
        }

        let doc: DocumentMut = applied.parse().unwrap();
        assert_eq!(
            doc["dependencies"]["bar-agent-guest-client"]
                .as_table_like()
                .unwrap()
                .get("path")
                .unwrap()
                .as_str()
                .unwrap(),
            "bridge/bar-agent-guest-client"
        );
        assert_eq!(
            doc["workspace"]["dependencies"]["bar"]
                .as_table_like()
                .unwrap()
                .get("path")
                .unwrap()
                .as_str()
                .unwrap(),
            "bridge/bar-agent-guest-client"
        );
    }

    #[test]
    fn guest_dependency_fix_updates_outdated_path() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { path = "../old/bar-agent-guest-client" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        let doc: DocumentMut = updated.parse().unwrap();
        let path = doc["dependencies"]["bar-agent-guest-client"]
            .as_table_like()
            .unwrap()
            .get("path")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(path, "../bridge/bar-agent-guest-client");
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_updates_target_specific_dependency_path() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [target.wasm32-wasip2.dependencies]
            bar-agent-guest-client = { path = "../old/bar-agent-guest-client" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        let doc: DocumentMut = updated.parse().unwrap();
        let path = doc["target"]["wasm32-wasip2"]["dependencies"]["bar-agent-guest-client"]
            .as_table_like()
            .unwrap()
            .get("path")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(path, "../bridge/bar-agent-guest-client");
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_converts_version_to_path() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = "0.0.0"
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        assert!(
            updated.contains(
                r#"bar-agent-guest-client = { path = "../bridge/bar-agent-guest-client" }"#
            )
        );
        assert!(!updated.contains("0.0.0"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_converts_inline_version_to_path_without_version_constraint() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { version = "999.0.0", features = ["extra"] }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        let doc: DocumentMut = updated.parse().unwrap();
        let item = doc["dependencies"]["bar-agent-guest-client"]
            .as_table_like()
            .unwrap();
        assert_eq!(
            item.get("path").unwrap().as_str().unwrap(),
            "../bridge/bar-agent-guest-client"
        );
        assert!(item.get("features").is_some());
        assert!(item.get("version").is_none());
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_preserves_compatible_path() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { path = "../bridge/bar-agent-guest-client" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        assert_eq!(updated, source);
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_does_not_force_add() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            serde = "1"
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        assert_eq!(updated, source);
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_preserves_package_alias() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar = { package = "bar-agent-guest-client", path = "../old", features = ["extra"] }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        let doc: DocumentMut = updated.parse().unwrap();
        let item = doc["dependencies"]["bar"].as_table_like().unwrap();
        assert_eq!(
            item.get("package").unwrap().as_str().unwrap(),
            "bar-agent-guest-client"
        );
        assert_eq!(
            item.get("path").unwrap().as_str().unwrap(),
            "../bridge/bar-agent-guest-client"
        );
        assert!(item.get("features").is_some());
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_updates_workspace_alias() {
        let root = tempfile::tempdir().unwrap();
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [workspace]
            members = ["consumer"]

            [workspace.dependencies]
            bar = { package = "bar-agent-guest-client", path = "old/bar-agent-guest-client" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Workspace,
            root.path(),
            &[guest_target(&output_dir)],
        );

        let doc: DocumentMut = updated.parse().unwrap();
        let item = doc["workspace"]["dependencies"]["bar"]
            .as_table_like()
            .unwrap();
        assert_eq!(
            item.get("package").unwrap().as_str().unwrap(),
            "bar-agent-guest-client"
        );
        assert_eq!(
            item.get("path").unwrap().as_str().unwrap(),
            "bridge/bar-agent-guest-client"
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn guest_dependency_fix_warns_on_unsupported_spec() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { git = "https://example.com/bar.git" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        assert_eq!(updated, source);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bar-agent-guest-client"));
    }

    #[test]
    fn guest_dependency_fix_warns_on_unsupported_git_spec_with_version() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { git = "https://example.com/bar.git", version = "0.0.1" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        assert_eq!(updated, source);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bar-agent-guest-client"));
    }

    #[test]
    fn guest_dependency_fix_warns_on_unsupported_git_spec_with_path() {
        let root = tempfile::tempdir().unwrap();
        let consumer_dir = root.path().join("consumer");
        let output_dir = root.path().join("bridge/bar-agent-guest-client");

        let source = indoc::indoc! {r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { git = "https://example.com/bar.git", path = "../custom/bar-agent-guest-client" }
        "#};

        let (updated, warnings) = run_guest_fix(
            source,
            GuestDependencyScope::Package,
            &consumer_dir,
            &[guest_target(&output_dir)],
        );

        assert_eq!(updated, source);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("bar-agent-guest-client"));
    }

    #[test]
    fn relative_path_unix_computes_sibling_path() {
        let from = std::path::Path::new("/a/b/consumer");
        let to = std::path::Path::new("/a/b/bridge/bar-agent-guest-client");
        assert_eq!(
            relative_path_unix(from, to).unwrap(),
            "../bridge/bar-agent-guest-client"
        );
    }
}
