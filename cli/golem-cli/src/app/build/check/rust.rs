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
use crate::fs;
use crate::model::GuestLanguage;
use crate::sdk_overrides::{RustDependency, SdkOverrides};
use crate::versions;
use std::path::{Path, PathBuf};

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
                evaluate_cargo_dependency_compliance(found, requirement, cargo_toml_path.parent())?;

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
        CargoDependencyMatcher, CargoDependencyRequirement, build_cargo_update_spec,
        evaluate_cargo_dependency_compliance, rust_dependency_requirements,
    };
    use crate::app::template::TEMPLATES_DIR;
    use crate::app::edit::cargo_toml::DependencySpec;
    use crate::sdk_overrides::sdk_overrides;
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
}
