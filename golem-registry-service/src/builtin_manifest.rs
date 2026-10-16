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
}

pub(crate) fn inventory() -> anyhow::Result<BTreeMap<String, InventoryEntry>> {
    serde_json::from_str(include_str!("../../builtin-artifacts.json"))
        .context("failed to parse builtin-artifacts.json")
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
