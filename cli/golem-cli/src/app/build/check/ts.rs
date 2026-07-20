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
    TsConfigSettingRequirement, effect_tsconfig_requirements, typescript_tsconfig_requirements,
};
use crate::app::build::check::{
    DependencyFixStep, DependencyMatcherSemantics, DependencySpecCompliance,
    ExpectedDependencyKind, evaluate_dependency_spec_compliance, expected_dependency_value,
};
use crate::app::context::BuildContext;
use crate::app::edit;
use crate::app::edit::json::collect_object_entries;
use crate::app::edit::tsconfig_json::RequiredSetting;
use crate::fs;
use crate::model::GuestLanguage;
use crate::sdk_overrides::SdkOverrides;
use crate::versions;
use std::collections::{BTreeMap, BTreeSet};

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
    selected_languages: &BTreeSet<GuestLanguage>,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Option<DependencyFixStep>> {
    let package_json_path = ctx.application().app_root_dir().join("package.json");
    let package_json_str_contents = fs::read_to_string(&package_json_path)?;
    let requirements = selected_sdk_requirements(overrides, selected_languages)?;
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
                )?,
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

    for component_name in ctx.application_context().selected_component_names() {
        let component = ctx.application().component(component_name);
        let (requirements, update_source) = match component.guess_language() {
            Some(GuestLanguage::TypeScript) => (
                typescript_tsconfig_requirements(),
                r#"{
  "compilerOptions": {
    "moduleResolution": "bundler",
    "experimentalDecorators": true,
    "emitDecoratorMetadata": true
  }
}"#,
            ),
            Some(GuestLanguage::Effect) => (
                effect_tsconfig_requirements(),
                r#"{
  "compilerOptions": {
    "moduleResolution": "bundler"
  }
}"#,
            ),
            _ => continue,
        };
        let requirements = requirements
            .iter()
            .map(to_tsconfig_required_setting)
            .collect::<Vec<_>>();

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

fn selected_sdk_requirements(
    overrides: &SdkOverrides,
    selected_languages: &BTreeSet<GuestLanguage>,
) -> anyhow::Result<Vec<PackageJsonDependencyRequirement>> {
    let mut requirements = BTreeMap::<&'static str, PackageJsonDependencyRequirement>::new();

    if selected_languages.contains(&GuestLanguage::TypeScript) {
        for requirement in typescript_sdk_requirements(overrides)? {
            requirements.insert(requirement.name, requirement);
        }
    }
    if selected_languages.contains(&GuestLanguage::Effect) {
        for requirement in effect_sdk_requirements(overrides)? {
            requirements.insert(requirement.name, requirement);
        }
    }

    Ok(requirements.into_values().collect())
}

fn typescript_sdk_requirements(
    overrides: &SdkOverrides,
) -> anyhow::Result<Vec<PackageJsonDependencyRequirement>> {
    let make_expected = |package_name: &str| -> anyhow::Result<ExpectedDependencyKind> {
        if overrides.ts_packages_path.is_some() {
            Ok(ExpectedDependencyKind::ExactPath(
                overrides.ts_package_dep(package_name)?,
            ))
        } else {
            Ok(ExpectedDependencyKind::SemanticCompatibleVersion {
                base_version: overrides.ts_package_dep(package_name)?,
                use_version_hint: false,
            })
        }
    };

    let mut requirements = vec![
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
    ];
    requirements.extend(common_typescript_toolchain_requirements());
    Ok(requirements)
}

fn effect_sdk_requirements(
    overrides: &SdkOverrides,
) -> anyhow::Result<Vec<PackageJsonDependencyRequirement>> {
    let effect_golem_expected = if overrides.effect_golem_path.is_some() {
        ExpectedDependencyKind::ExactPath(overrides.effect_golem_dep()?)
    } else {
        ExpectedDependencyKind::ExactValue(overrides.effect_golem_dep()?)
    };

    let mut requirements = vec![
        PackageJsonDependencyRequirement {
            name: "@golemcloud/effect-golem",
            section: PackageJsonSection::Dependencies,
            expected: effect_golem_expected,
            semantics: DependencyMatcherSemantics::TypeScript,
        },
        PackageJsonDependencyRequirement {
            name: "effect",
            section: PackageJsonSection::Dependencies,
            expected: ExpectedDependencyKind::ExactValue(versions::effect_dep::EFFECT.to_string()),
            semantics: DependencyMatcherSemantics::TypeScript,
        },
    ];
    requirements.extend(common_typescript_toolchain_requirements());
    Ok(requirements)
}

fn common_typescript_toolchain_requirements() -> Vec<PackageJsonDependencyRequirement> {
    [
        (
            "@rollup/plugin-node-resolve",
            versions::ts_dep::ROLLUP_PLUGIN_NODE_RESOLVE,
        ),
        (
            "@rollup/plugin-typescript",
            versions::ts_dep::ROLLUP_PLUGIN_TYPESCRIPT,
        ),
        (
            "@rollup/plugin-commonjs",
            versions::ts_dep::ROLLUP_PLUGIN_COMMONJS,
        ),
        ("@rollup/plugin-json", versions::ts_dep::ROLLUP_PLUGIN_JSON),
        ("@types/node", versions::ts_dep::TYPES_NODE),
        ("rollup", versions::ts_dep::ROLLUP),
        ("tslib", versions::ts_dep::TSLIB),
        ("typescript", versions::ts_dep::TYPESCRIPT),
    ]
    .into_iter()
    .map(|(name, version)| PackageJsonDependencyRequirement {
        name,
        section: PackageJsonSection::DevDependencies,
        expected: ExpectedDependencyKind::SemanticCompatibleVersion {
            base_version: dep_base_version(version),
            use_version_hint: true,
        },
        semantics: DependencyMatcherSemantics::TypeScript,
    })
    .collect()
}

fn dep_base_version(spec: &str) -> String {
    crate::app::build::check::extract_version(spec).unwrap_or_else(|| spec.to_string())
}

#[cfg(test)]
mod test {
    use super::{
        PackageJsonDependencyRequirement, PackageJsonSection, effect_sdk_requirements,
        selected_sdk_requirements, typescript_sdk_requirements,
    };
    use crate::app::template::TEMPLATES_DIR;
    use crate::model::GuestLanguage;
    use crate::sdk_overrides::sdk_overrides;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn ts_template_and_check_requirements_match() {
        let overrides = sdk_overrides().unwrap();
        assert_template_and_check_requirements_match(
            "ts/common/package.json",
            typescript_sdk_requirements(overrides).unwrap(),
        );
    }

    #[test]
    fn effect_template_and_check_requirements_match() {
        let overrides = sdk_overrides().unwrap();
        assert_template_and_check_requirements_match(
            "effect/common/package.json",
            effect_sdk_requirements(overrides).unwrap(),
        );
    }

    #[test]
    fn mixed_typescript_and_effect_requirements_include_both_sdk_profiles() {
        let languages = BTreeSet::from([GuestLanguage::TypeScript, GuestLanguage::Effect]);
        let requirement_names = selected_sdk_requirements(sdk_overrides().unwrap(), &languages)
            .unwrap()
            .into_iter()
            .map(|requirement| requirement.name)
            .collect::<BTreeSet<_>>();

        assert!(requirement_names.contains("@golemcloud/golem-ts-sdk"));
        assert!(requirement_names.contains("@golemcloud/golem-ts-typegen"));
        assert!(requirement_names.contains("@golemcloud/effect-golem"));
        assert!(requirement_names.contains("effect"));
        assert_eq!(requirement_names.len(), 13);
    }

    fn assert_template_and_check_requirements_match(
        template_path: &str,
        requirements: Vec<PackageJsonDependencyRequirement>,
    ) {
        let template_source = TEMPLATES_DIR
            .get_file(template_path)
            .unwrap()
            .contents_utf8()
            .unwrap()
            .to_string();
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

        let mut check_deps = BTreeSet::new();
        let mut check_dev_deps = BTreeSet::new();

        for requirement in requirements {
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
