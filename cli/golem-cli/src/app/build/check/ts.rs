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
    TsConfigSettingRequirement, typescript_tsconfig_requirements,
};
use crate::app::build::check::{
    DependencyFixStep, DependencyMatcherSemantics, DependencyPresence, DependencySpecCompliance,
    ExpectedDependencyKind, evaluate_dependency_spec_compliance, expected_dependency_value,
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
    presence: DependencyPresence,
}

impl PackageJsonDependencyRequirement {
    fn dependency(
        name: &'static str,
        expected: ExpectedDependencyKind,
        presence: DependencyPresence,
    ) -> Self {
        Self {
            name,
            section: PackageJsonSection::Dependencies,
            expected,
            presence,
        }
    }

    fn dev_dependency(
        name: &'static str,
        expected: ExpectedDependencyKind,
        presence: DependencyPresence,
    ) -> Self {
        Self {
            name,
            section: PackageJsonSection::DevDependencies,
            expected,
            presence,
        }
    }
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
                    DependencyMatcherSemantics::TypeScript,
                )?,
                None => DependencySpecCompliance::SkipWarn(format!(
                    "Skipped dependency check for complex package spec '{}'",
                    raw
                )),
            },
            None if requirement.presence == DependencyPresence::Optional => {
                DependencySpecCompliance::Compatible
            }
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
        let Some(language) = component.guess_language() else {
            continue;
        };
        // Each TS-family SDK has its own required tsconfig settings; non-TS → empty.
        let tsconfig_requirements = typescript_tsconfig_requirements(language);
        if tsconfig_requirements.is_empty() {
            continue;
        }
        let requirements = tsconfig_requirements
            .iter()
            .map(to_tsconfig_required_setting)
            .collect::<Vec<_>>();

        let tsconfig_path = component.component_dir().join("tsconfig.json");
        let source = fs::read_to_string(&tsconfig_path)?;
        let missing = edit::tsconfig_json::check_required_settings(source.as_str(), &requirements)?;

        if !missing.is_empty() {
            // Derive the fix patch from exactly what's missing, then deep-merge it in.
            let patch = edit::tsconfig_json::build_settings_patch(&missing)?;
            let new = edit::tsconfig_json::merge_with_newer(source.as_str(), &patch)?;

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

fn typescript_sdk_requirements(
    overrides: &SdkOverrides,
) -> anyhow::Result<Vec<PackageJsonDependencyRequirement>> {
    use DependencyPresence::{Optional, Required};
    use PackageJsonDependencyRequirement as Req;
    use versions::ts_dep;

    // Expected spec for an `@golemcloud` SDK package: the local path when overridden,
    // otherwise a semver-compatible match on the published version.
    let sdk_package = |name: &str| -> anyhow::Result<ExpectedDependencyKind> {
        let dep = overrides.ts_package_dep(name)?;
        Ok(if overrides.ts_packages_path.is_some() {
            ExpectedDependencyKind::ExactPath(dep)
        } else {
            ExpectedDependencyKind::compatible(dep)
        })
    };
    let pinned = ExpectedDependencyKind::version_hint;

    Ok(vec![
        // Required: the SDK + the shared rollup/tsc build toolchain.
        Req::dependency(
            "@golemcloud/golem-ts-sdk",
            sdk_package("golem-ts-sdk")?,
            Required,
        ),
        Req::dev_dependency(
            "@rollup/plugin-alias",
            pinned(ts_dep::ROLLUP_PLUGIN_ALIAS),
            Required,
        ),
        Req::dev_dependency(
            "@rollup/plugin-node-resolve",
            pinned(ts_dep::ROLLUP_PLUGIN_NODE_RESOLVE),
            Required,
        ),
        Req::dev_dependency(
            "@rollup/plugin-typescript",
            pinned(ts_dep::ROLLUP_PLUGIN_TYPESCRIPT),
            Required,
        ),
        Req::dev_dependency(
            "@rollup/plugin-commonjs",
            pinned(ts_dep::ROLLUP_PLUGIN_COMMONJS),
            Required,
        ),
        Req::dev_dependency(
            "@rollup/plugin-json",
            pinned(ts_dep::ROLLUP_PLUGIN_JSON),
            Required,
        ),
        Req::dev_dependency("@types/node", pinned(ts_dep::TYPES_NODE), Required),
        Req::dev_dependency("rollup", pinned(ts_dep::ROLLUP), Required),
        Req::dev_dependency("tslib", pinned(ts_dep::TSLIB), Required),
        Req::dev_dependency("typescript", pinned(ts_dep::TYPESCRIPT), Required),
        // Optional (only checked if present): the known Standard Schema libraries apply
        // only where used.
        Req::dependency("zod", pinned(ts_dep::ZOD), Optional),
        Req::dependency("valibot", pinned(ts_dep::VALIBOT), Optional),
        Req::dependency("arktype", pinned(ts_dep::ARKTYPE), Optional),
    ])
}

#[cfg(test)]
mod test {
    use super::{PackageJsonSection, typescript_sdk_requirements};
    use crate::app::template::TEMPLATES_DIR;
    use crate::sdk_overrides::sdk_overrides;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn ts_template_and_check_requirements_match() {
        let template_source = TEMPLATES_DIR
            .get_file("ts/common/package.json")
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

        let overrides = sdk_overrides().unwrap();
        let mut required_deps = BTreeSet::new();
        let mut required_dev_deps = BTreeSet::new();
        let mut known_deps = BTreeSet::new();
        let mut known_dev_deps = BTreeSet::new();

        for requirement in typescript_sdk_requirements(overrides).unwrap() {
            let (known, required) = match requirement.section {
                PackageJsonSection::Dependencies => (&mut known_deps, &mut required_deps),
                PackageJsonSection::DevDependencies => {
                    (&mut known_dev_deps, &mut required_dev_deps)
                }
            };
            known.insert(requirement.name.to_string());
            if requirement.presence == crate::app::build::check::DependencyPresence::Required {
                required.insert(requirement.name.to_string());
            }
        }

        // Every mandatory dependency must be present in the `ts` template.
        assert!(
            required_deps.is_subset(&template_deps),
            "ts template is missing required dependencies: {:?}",
            required_deps.difference(&template_deps).collect::<Vec<_>>()
        );
        assert!(
            required_dev_deps.is_subset(&template_dev_deps),
            "ts template is missing required devDependencies: {:?}",
            required_dev_deps
                .difference(&template_dev_deps)
                .collect::<Vec<_>>()
        );

        // Every dependency the template ships must be known to the build check,
        // so no unmanaged dependency drifts into the template unnoticed.
        assert!(
            template_deps.is_subset(&known_deps),
            "ts template has dependencies unknown to the build check: {:?}",
            template_deps.difference(&known_deps).collect::<Vec<_>>()
        );
        assert!(
            template_dev_deps.is_subset(&known_dev_deps),
            "ts template has devDependencies unknown to the build check: {:?}",
            template_dev_deps
                .difference(&known_dev_deps)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn ts_tsconfig_requires_bundler_resolution_without_decorators() {
        use crate::app::build::check::requirements::typescript_tsconfig_requirements;
        use crate::model::GuestLanguage;

        let setting_keys = |language| -> Vec<&'static str> {
            typescript_tsconfig_requirements(language)
                .iter()
                .map(|requirement| *requirement.path.last().unwrap())
                .collect()
        };

        // The TS SDK only needs bundler resolution — no decorator options.
        let ts = setting_keys(GuestLanguage::TypeScript);
        assert!(ts.contains(&"moduleResolution"));
        assert!(!ts.contains(&"experimentalDecorators"));
        assert!(!ts.contains(&"emitDecoratorMetadata"));
    }
}
