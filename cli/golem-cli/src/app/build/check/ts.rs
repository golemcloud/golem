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

use crate::app::build::check::requirements::{
    typescript_tsconfig_requirements, TsConfigSettingRequirement,
};
use crate::app::build::check::{
    evaluate_dependency_spec_compliance, expected_dependency_value, DependencyFixStep,
    DependencyMatcherSemantics, DependencySpecCompliance, ExpectedDependencyKind,
};
use crate::app::context::BuildContext;
use crate::app::edit;
use crate::app::edit::json::collect_object_entries;
use crate::app::edit::tsconfig_json::RequiredSetting;
use crate::fs;
use crate::sdk_overrides::SdkOverrides;
use crate::versions;

#[derive(Clone, Copy, Debug)]
enum PackageJsonSection {
    Dependencies,
    DevDependencies,
}

#[derive(Clone, Debug)]
struct PackageJsonDependencyRequirement {
    name: &'static str,
    section: PackageJsonSection,
    expected: ExpectedDependencyKind,
    semantics: DependencyMatcherSemantics,
}

pub(super) fn plan_package_json_fix_step(
    ctx: &BuildContext<'_>,
    overrides: &SdkOverrides,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<DependencyFixStep>> {
    let package_json_path = ctx.application().app_root_dir().join("package.json");
    let package_json_str_contents = fs::read_to_string(&package_json_path)?;
    let requirements = typescript_sdk_requirements(overrides)?;
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
            DependencySpecCompliance::Compatible => {}
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

pub(super) fn plan_tsconfig_fix_steps(
    ctx: &BuildContext<'_>,
) -> anyhow::Result<Vec<DependencyFixStep>> {
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
        if component.guess_language() != Some(crate::model::GuestLanguage::TypeScript) {
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

fn to_tsconfig_required_setting(requirement: &TsConfigSettingRequirement) -> RequiredSetting {
    RequiredSetting {
        path: requirement.path.iter().map(|s| (*s).to_string()).collect(),
        expected_literal: requirement.expected_literal.map(|value| value.to_string()),
    }
}

fn parse_json_string_literal(raw: &str) -> Option<String> {
    serde_json::from_str::<String>(raw).ok()
}

fn typescript_sdk_requirements(overrides: &SdkOverrides) -> anyhow::Result<Vec<PackageJsonDependencyRequirement>> {
    let make_expected = |package_name: &str| -> anyhow::Result<ExpectedDependencyKind> {
        if overrides.ts_packages_path.is_some() {
            Ok(ExpectedDependencyKind::ExactPath(overrides.ts_package_dep(package_name)?))
        } else {
            Ok(ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: overrides.ts_package_dep(package_name)?,
                use_version_hint: false,
            })
        }
    };

    Ok(vec![
        PackageJsonDependencyRequirement {
            name: "@golemcloud/golem-ts-sdk",
            section: PackageJsonSection::Dependencies,
            expected: make_expected("golem-ts-sdk")?,
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@golemcloud/golem-ts-typegen",
            section: PackageJsonSection::DevDependencies,
            expected: make_expected("golem-ts-typegen")?,
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-alias",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::ROLLUP_PLUGIN_ALIAS),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-node-resolve",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::ROLLUP_PLUGIN_NODE_RESOLVE),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-typescript",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::ROLLUP_PLUGIN_TYPESCRIPT),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-commonjs",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::ROLLUP_PLUGIN_COMMONJS),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@rollup/plugin-json",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::ROLLUP_PLUGIN_JSON),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "@types/node",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::TYPES_NODE),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "rollup",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::ROLLUP),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "tslib",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::TSLIB),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "typescript",
            section: PackageJsonSection::DevDependencies,
            expected: ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: dep_base_version(versions::ts_dep::TYPESCRIPT),
                use_version_hint: true,
            },
            semantics: DependencyMatcherSemantics::TypeScript,
        },
    ])
}

fn dep_base_version(spec: &str) -> String {
    crate::app::build::check::extract_version(spec).unwrap_or_else(|| spec.to_string())
}

#[cfg(test)]
mod test {
    use super::{typescript_sdk_requirements, PackageJsonSection};
    use crate::sdk_overrides::sdk_overrides;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use test_r::test;

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

        for requirement in typescript_sdk_requirements(overrides).unwrap() {
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
}
