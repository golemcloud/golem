use anyhow::{Context, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Deserialize)]
pub(crate) struct InventoryEntry {
    pub component: String,
    #[allow(
        dead_code,
        reason = "Release build validation only uses the component mapping"
    )]
    pub path: PathBuf,
    #[serde(default)]
    pub optional: bool,
}

/// Selects optional built-ins for development builds: `all`, or a comma-separated list of
/// artifact IDs. Release builds use the release manifest, which lists every built-in.
pub(crate) const OPTIONAL_BUILTINS_ENV: &str = "GOLEM_OPTIONAL_BUILTINS";

pub(crate) fn inventory() -> anyhow::Result<BTreeMap<String, InventoryEntry>> {
    serde_json::from_str(include_str!("../../builtin-artifacts.json"))
        .context("failed to parse builtin-artifacts.json")
}

/// The inventory entries a development build provisions: every mandatory built-in and the
/// optional ones that `selection` (the value of [`OPTIONAL_BUILTINS_ENV`]) names.
#[cfg_attr(
    all(feature = "embed-builtin-artifacts", not(test)),
    allow(
        dead_code,
        reason = "Embedded builds select their artifacts in build.rs"
    )
)]
pub(crate) fn development_inventory(
    selection: Option<&str>,
) -> anyhow::Result<BTreeMap<String, InventoryEntry>> {
    select_development(inventory()?, selection)
}

fn select_development(
    mut inventory: BTreeMap<String, InventoryEntry>,
    selection: Option<&str>,
) -> anyhow::Result<BTreeMap<String, InventoryEntry>> {
    let selection = selection.map(str::trim).unwrap_or_default();
    if selection == "all" {
        return Ok(inventory);
    }
    let selected = selection
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    for id in &selected {
        ensure!(
            inventory.get(*id).is_some_and(|entry| entry.optional),
            "{OPTIONAL_BUILTINS_ENV} names '{id}', which is not an optional built-in in builtin-artifacts.json"
        );
    }
    inventory.retain(|id, entry| !entry.optional || selected.contains(id.as_str()));
    Ok(inventory)
}

#[derive(Deserialize)]
pub(crate) struct ManifestEntry {
    pub component: String,
    pub version: String,
    pub sha256: String,
}

pub(crate) fn parse_manifest(contents: &str) -> anyhow::Result<BTreeMap<String, ManifestEntry>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Manifest {
        schema_version: u32,
        repository: String,
        artifacts: BTreeMap<String, ManifestEntry>,
    }

    let manifest: Manifest =
        serde_json::from_str(contents).context("failed to parse builtin-artifacts.release.json")?;
    ensure!(
        manifest.schema_version == 1,
        "unsupported built-in manifest schema version"
    );
    ensure!(
        manifest.repository == "golemcloud/golem-builtins",
        "unsupported built-in artifact repository"
    );
    let inventory = inventory()?;
    ensure!(
        manifest.artifacts.keys().eq(inventory.keys()),
        "release manifest IDs must exactly match builtin-artifacts.json"
    );
    for (id, entry) in &manifest.artifacts {
        ensure!(
            entry.component == inventory[id].component,
            "release manifest component for '{id}' must be '{}'",
            inventory[id].component
        );
        semver::Version::parse(&entry.version)
            .with_context(|| format!("invalid version for built-in artifact '{id}'"))?;
        ensure!(
            entry.sha256.len() == 64
                && entry
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid SHA-256 for built-in artifact '{id}'"
        );
    }
    Ok(manifest.artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn fixture() -> BTreeMap<String, InventoryEntry> {
        serde_json::from_str(
            r#"{
                "bash": {"component": "bash", "path": "builtin-tools/bash.wasm"},
                "yq": {"component": "yq", "path": "builtin-tools/yq.wasm", "optional": true},
                "yq2": {"component": "yq2", "path": "builtin-tools/yq2.wasm", "optional": true}
            }"#,
        )
        .unwrap()
    }

    fn selected(selection: Option<&str>) -> Vec<String> {
        select_development(fixture(), selection)
            .unwrap()
            .into_keys()
            .collect()
    }

    #[test]
    fn development_builds_skip_unselected_optional_builtins() {
        assert_eq!(selected(None), ["bash"]);
        assert_eq!(selected(Some(" ")), ["bash"]);
        assert_eq!(selected(Some("yq")), ["bash", "yq"]);
        assert_eq!(selected(Some(" yq2 , ")), ["bash", "yq2"]);
        assert_eq!(selected(Some("all")), ["bash", "yq", "yq2"]);
    }

    #[test]
    fn a_selection_must_name_optional_builtins() {
        for selection in ["y", "bash", "yq,all"] {
            assert!(
                select_development(fixture(), Some(selection)).is_err(),
                "{selection}"
            );
        }
    }
}
