// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::fs;
use crate::model::app_raw::{RustBridgeDependency, RustBridgeDependencyDetails};
use anyhow::{Context, bail};
use regex::Regex;
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::Path as SynPath;
use toml_edit::{DocumentMut, Item, Value};

const RESERVED_DEPENDENCIES: &[&str] = &[
    "chrono",
    "golem-client",
    "golem-common",
    "golem-rust",
    "reqwest",
    "reqwest-middleware",
    "serde",
    "serde_json",
    "uuid",
];

#[derive(Clone, Debug, Default)]
pub struct RustBridgeGeneratorConfig {
    derive_rules: Vec<RustDeriveRule>,
    dependencies: BTreeMap<String, RustDependency>,
}

#[derive(Clone, Debug)]
struct RustDeriveRule {
    pattern: String,
    regex: Regex,
    derives: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RustDependency {
    pub version: Option<String>,
    pub path: Option<PathBuf>,
    pub git: Option<String>,
    pub package: Option<String>,
    pub features: Vec<String>,
    pub default_features: Option<bool>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MarkerConfig<'a> {
    derive_rules: Vec<MarkerDeriveRule<'a>>,
    dependencies: &'a BTreeMap<String, RustDependency>,
}

#[derive(Serialize)]
struct MarkerDeriveRule<'a> {
    pattern: &'a str,
    derives: Vec<&'a str>,
}

impl Serialize for RustBridgeGeneratorConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        MarkerConfig {
            derive_rules: self
                .derive_rules
                .iter()
                .map(|rule| MarkerDeriveRule {
                    pattern: &rule.pattern,
                    derives: rule.derives.iter().map(String::as_str).collect(),
                })
                .collect(),
            dependencies: &self.dependencies,
        }
        .serialize(serializer)
    }
}

impl RustBridgeGeneratorConfig {
    pub fn from_manifest(
        derive_rules: &[String],
        dependencies: BTreeMap<String, RustBridgeDependency>,
        source_dir: &Path,
    ) -> anyhow::Result<Self> {
        Self::normalize(derive_rules, dependencies, source_dir)
    }

    pub fn from_cli(
        derive_rules: &[String],
        dependency_assignments: &[String],
        cwd: &Path,
    ) -> anyhow::Result<Self> {
        let dependencies = dependency_assignments
            .iter()
            .map(|assignment| parse_dependency_assignment(assignment))
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        Self::normalize(derive_rules, dependencies, cwd)
    }

    fn normalize(
        derive_rules: &[String],
        dependencies: BTreeMap<String, RustBridgeDependency>,
        base_dir: &Path,
    ) -> anyhow::Result<Self> {
        let derive_rules = derive_rules
            .iter()
            .map(|rule| parse_derive_rule(rule))
            .collect::<anyhow::Result<_>>()?;
        let dependencies = dependencies
            .into_iter()
            .map(|(name, dependency)| {
                validate_dependency_name(&name)?;
                Ok((name, normalize_dependency(dependency, base_dir)?))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self {
            derive_rules,
            dependencies,
        })
    }

    pub(crate) fn derives_for(
        &self,
        type_name: &str,
        builtins: &[&str],
        allow_debug: bool,
        allow_clone: bool,
    ) -> Vec<SynPath> {
        let mut seen = builtins
            .iter()
            .map(|value| DeriveKey::Builtin((*value).to_string()))
            .collect::<BTreeSet<_>>();
        self.derive_rules
            .iter()
            .filter(|rule| rule.regex.is_match(type_name))
            .flat_map(|rule| &rule.derives)
            .filter(|path| {
                let builtin = builtin_derive_name(path);
                (allow_debug || builtin != Some("Debug"))
                    && (allow_clone || builtin != Some("Clone"))
            })
            .filter_map(|path| {
                let key = builtin_derive_name(path)
                    .map(|name| DeriveKey::Builtin(name.to_string()))
                    .unwrap_or_else(|| DeriveKey::Path(path.clone()));
                seen.insert(key)
                    .then(|| syn::parse_str::<SynPath>(path).expect("validated Rust derive path"))
            })
            .collect()
    }

    pub(crate) fn dependencies(&self) -> &BTreeMap<String, RustDependency> {
        &self.dependencies
    }

    pub fn is_configured(&self) -> bool {
        !self.derive_rules.is_empty() || !self.dependencies.is_empty()
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum DeriveKey {
    Builtin(String),
    Path(String),
}

fn builtin_derive_name(path: &str) -> Option<&'static str> {
    match path.rsplit("::").next()? {
        "Debug" => Some("Debug"),
        "Clone" => Some("Clone"),
        _ => None,
    }
}

fn parse_derive_rule(rule: &str) -> anyhow::Result<RustDeriveRule> {
    let (pattern, derives) = rule
        .rsplit_once('=')
        .with_context(|| format!("invalid derive rule '{rule}': expected '<regex>=Trait,Trait'"))?;
    let regex =
        Regex::new(pattern).with_context(|| format!("invalid derive rule regex '{pattern}'"))?;
    let derives = derives
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let path = syn::parse_str::<SynPath>(value)
                .with_context(|| format!("invalid Rust derive path '{value}'"))?;
            Ok(quote::quote!(#path).to_string().replace(' ', ""))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if derives.is_empty() {
        bail!("derive rule '{rule}' has no derive paths")
    }
    Ok(RustDeriveRule {
        pattern: pattern.to_string(),
        regex,
        derives,
    })
}

fn parse_dependency_assignment(value: &str) -> anyhow::Result<(String, RustBridgeDependency)> {
    let doc = format!("[dependencies]\n{value}\n")
        .parse::<DocumentMut>()
        .with_context(|| format!("invalid Cargo dependency assignment '{value}'"))?;
    let table = doc["dependencies"]
        .as_table()
        .expect("dependencies is a table");
    if table.len() != 1 {
        bail!("Rust dependency must contain exactly one Cargo assignment: '{value}'")
    }
    let (name, item) = table.iter().next().unwrap();
    Ok((name.to_string(), dependency_from_toml(item)?))
}

fn dependency_from_toml(item: &Item) -> anyhow::Result<RustBridgeDependency> {
    if let Some(version) = item.as_str() {
        return Ok(RustBridgeDependency::Version(version.to_string()));
    }
    let table = item
        .as_inline_table()
        .with_context(|| "dependency spec must be a version string or inline table")?;
    let get_string = |key| -> anyhow::Result<Option<String>> {
        table
            .get(key)
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .with_context(|| format!("'{key}' must be a string"))
            })
            .transpose()
    };
    let features = table
        .get("features")
        .map(|value| {
            value
                .as_array()
                .with_context(|| "'features' must be an array")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .with_context(|| "features must be strings")
                })
                .collect()
        })
        .transpose()?
        .unwrap_or_default();
    let get_bool = |key| -> anyhow::Result<Option<bool>> {
        table
            .get(key)
            .map(|v| {
                v.as_bool()
                    .with_context(|| format!("'{key}' must be a boolean"))
            })
            .transpose()
    };
    let known = [
        "version",
        "path",
        "git",
        "package",
        "features",
        "default-features",
        "branch",
        "tag",
        "rev",
        "workspace",
        "optional",
    ];
    if let Some((key, _)) = table.iter().find(|(key, _)| !known.contains(key)) {
        bail!("unsupported Cargo dependency key '{key}'")
    }
    Ok(RustBridgeDependency::Detailed(
        RustBridgeDependencyDetails {
            version: get_string("version")?,
            path: get_string("path")?,
            git: get_string("git")?,
            package: get_string("package")?,
            features,
            default_features: get_bool("default-features")?,
            branch: get_string("branch")?,
            tag: get_string("tag")?,
            rev: get_string("rev")?,
            workspace: get_bool("workspace")?,
            optional: get_bool("optional")?,
        },
    ))
}

fn validate_dependency_name(name: &str) -> anyhow::Result<()> {
    if RESERVED_DEPENDENCIES.contains(&name) {
        bail!("dependency name '{name}' is owned by the Rust bridge generator")
    }
    if name.is_empty() {
        bail!("dependency name cannot be empty")
    }
    Ok(())
}

fn normalize_dependency(
    raw: RustBridgeDependency,
    base_dir: &Path,
) -> anyhow::Result<RustDependency> {
    let details = match raw {
        RustBridgeDependency::Version(version) => RustBridgeDependencyDetails {
            version: Some(version),
            ..Default::default()
        },
        RustBridgeDependency::Detailed(details) => details,
    };
    if details.workspace.is_some() {
        bail!("workspace dependencies are not supported in generated bridge crates")
    }
    if details.optional.is_some() {
        bail!("optional dependencies are not supported in generated bridge crates")
    }
    let sources = usize::from(details.version.is_some())
        + usize::from(details.path.is_some())
        + usize::from(details.git.is_some());
    if sources != 1 {
        bail!("dependency must specify exactly one of version, path, or git")
    }
    let selectors = usize::from(details.branch.is_some())
        + usize::from(details.tag.is_some())
        + usize::from(details.rev.is_some());
    if selectors > 1 {
        bail!("git dependency may specify only one of branch, tag, or rev")
    }
    if selectors > 0 && details.git.is_none() {
        bail!("branch, tag, and rev require a git dependency")
    }
    Ok(RustDependency {
        version: details.version,
        path: details
            .path
            .map(|path| fs::absolute_lexical_path_from_base_dir(Path::new(&path), base_dir)),
        git: details.git,
        package: details.package,
        features: details.features,
        default_features: details.default_features,
        branch: details.branch,
        tag: details.tag,
        rev: details.rev,
    })
}

pub(crate) fn dependency_item(dependency: &RustDependency) -> Item {
    if let Some(version) = &dependency.version
        && dependency.package.is_none()
        && dependency.features.is_empty()
        && dependency.default_features.is_none()
    {
        return toml_edit::value(version);
    }
    let mut table = toml_edit::InlineTable::new();
    for (key, value) in [
        ("version", dependency.version.as_ref()),
        ("git", dependency.git.as_ref()),
        ("package", dependency.package.as_ref()),
        ("branch", dependency.branch.as_ref()),
        ("tag", dependency.tag.as_ref()),
        ("rev", dependency.rev.as_ref()),
    ] {
        if let Some(value) = value {
            table.insert(key, Value::from(value.as_str()));
        }
    }
    if let Some(path) = &dependency.path {
        table.insert("path", Value::from(path.to_string_lossy().to_string()));
    }
    if !dependency.features.is_empty() {
        let mut values = toml_edit::Array::new();
        dependency.features.iter().for_each(|v| {
            values.push(v);
        });
        table.insert("features", Value::Array(values));
    }
    if let Some(value) = dependency.default_features {
        table.insert("default-features", Value::from(value));
    }
    Item::Value(Value::InlineTable(table))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn cli_config_parses_repeated_rules_and_full_cargo_dependencies() {
        let config = RustBridgeGeneratorConfig::from_cli(
            &["^Order=serde::Serialize,Clone".into(), "Result$=Eq".into()],
            &[
                "serde_with = { version = \"3\", features = [\"macros\"], default-features = false }".into(),
                "renamed = { package = \"actual\", git = \"https://example.test/repo\", rev = \"abc\" }".into(),
            ],
            Path::new("/work/app"),
        ).unwrap();

        assert_eq!(config.derives_for("OrderResult", &[], true, true).len(), 3);
        assert_eq!(config.dependencies.len(), 2);
        assert!(
            serde_json::to_string(&config)
                .unwrap()
                .contains("serde_with")
        );
    }

    #[test]
    fn config_validates_rules_sources_selectors_and_reserved_forms() {
        for (rule, expected) in [
            ("[=Clone", "regex"),
            (".*=not::", "derive path"),
            (".*=", "no derive"),
        ] {
            let error =
                RustBridgeGeneratorConfig::from_cli(&[rule.into()], &[], Path::new("/work"))
                    .unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        for dependency in [
            "serde = \"1\"",
            "x = { version = \"1\", path = \"../x\" }",
            "x = { git = \"https://example.test/x\", branch = \"main\", tag = \"v1\" }",
            "x = { version = \"1\", branch = \"main\" }",
            "x = { workspace = true }",
            "x = { version = \"1\", optional = true }",
        ] {
            assert!(
                RustBridgeGeneratorConfig::from_cli(&[], &[dependency.into()], Path::new("/work"))
                    .is_err(),
                "accepted {dependency}"
            );
        }
    }

    #[test]
    fn dependency_paths_resolve_from_manifest_source_and_cli_working_directory() {
        let manifest_target = crate::model::app_raw::RustBridgeSdkExternalTargets {
            common: crate::model::app_raw::BridgeSdkExternalTargets {
                agents: Default::default(),
                output_dir: None,
            },
            additional_derives: Vec::new(),
            additional_dependencies: BTreeMap::from([(
                "derive-fixture".into(),
                RustBridgeDependency::Detailed(RustBridgeDependencyDetails {
                    path: Some("../derive-fixture".into()),
                    ..Default::default()
                }),
            )]),
        };
        let config = RustBridgeGeneratorConfig::from_manifest(
            &manifest_target.additional_derives,
            manifest_target.additional_dependencies,
            Path::new("/repo/manifests"),
        )
        .unwrap();
        assert_eq!(
            config.dependencies["derive-fixture"].path.as_deref(),
            Some(Path::new("/repo/derive-fixture"))
        );

        let cli_config = RustBridgeGeneratorConfig::from_cli(
            &[],
            &["derive-fixture = { path = \"../derive-fixture\" }".into()],
            Path::new("/work/invocation"),
        )
        .unwrap();
        assert_eq!(
            cli_config.dependencies["derive-fixture"].path.as_deref(),
            Some(Path::new("/work/derive-fixture"))
        );
        let rendered = dependency_item(&cli_config.dependencies["derive-fixture"]).to_string();
        assert!(rendered.contains("/work/derive-fixture"), "{rendered}");
    }

    #[test]
    fn matching_rules_merge_and_deduplicate_builtins_and_each_other() {
        let config = RustBridgeGeneratorConfig::from_cli(
            &["^Order=Clone,Eq".into(), "Result$=Eq,Hash".into()],
            &[],
            Path::new("/work"),
        )
        .unwrap();
        let derives = config
            .derives_for("OrderResult", &["Debug", "Clone"], true, true)
            .into_iter()
            .map(|path| quote::quote!(#path).to_string())
            .collect::<Vec<_>>();
        assert_eq!(derives, ["Eq", "Hash"]);
        assert!(
            config
                .derives_for("Other", &["Debug", "Clone"], true, true)
                .is_empty()
        );
        assert_eq!(
            config
                .derives_for("Order", &[], false, false)
                .into_iter()
                .map(|path| quote::quote!(#path).to_string())
                .collect::<Vec<_>>(),
            ["Eq"]
        );
        assert_eq!(
            config
                .derives_for("Order", &[], true, false)
                .into_iter()
                .map(|path| quote::quote!(#path).to_string())
                .collect::<Vec<_>>(),
            ["Eq"]
        );
    }

    #[test]
    fn qualified_builtin_derives_are_deduplicated_and_restricted() {
        let config = RustBridgeGeneratorConfig::from_cli(
            &[".*=std::clone::Clone,std::fmt::Debug,serde::Serialize".into()],
            &[],
            Path::new("/work"),
        )
        .unwrap();

        let with_builtins = config
            .derives_for("Order", &["Debug", "Clone"], true, true)
            .into_iter()
            .map(|path| quote::quote!(#path).to_string())
            .collect::<Vec<_>>();
        assert_eq!(with_builtins, ["serde :: Serialize"]);

        let restricted = config
            .derives_for("StreamResult", &[], false, false)
            .into_iter()
            .map(|path| quote::quote!(#path).to_string())
            .collect::<Vec<_>>();
        assert_eq!(restricted, ["serde :: Serialize"]);
    }
}
