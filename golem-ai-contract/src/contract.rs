// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::check_corpus;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Index {
    files: Vec<String>,
    recording_sets: Vec<String>,
}

fn relative(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "contract paths must be relative without parent components"
    );
    Ok(path)
}

/// Validate declared assets, case IDs and complete recording sets without changing files.
pub fn check(directory: &Path) -> Result<()> {
    let index: Index =
        serde_json::from_slice(&std::fs::read(directory.join("corpus-index.json"))?)?;
    let mut files = BTreeSet::new();
    for file in index.files {
        let bytes = std::fs::read(directory.join(relative(&file)?))?;
        let value: Value = serde_json::from_slice(&bytes)?;
        if value.get("cases").is_some() || value.get("caseIds").is_some() {
            check_case_ids(&value).with_context(|| format!("checking {file}"))?;
        }
        ensure!(files.insert(file), "duplicate contract file");
    }
    let mut recording_sets = BTreeSet::new();
    for set in index.recording_sets {
        ensure!(
            recording_sets.insert(set.clone()),
            "duplicate recording set"
        );
        let path = directory.join(relative(&set)?);
        check_corpus(&path).with_context(|| format!("checking recording set {set}"))?;
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let file = format!("{set}/{}", entry.file_name().to_string_lossy());
            ensure!(files.insert(file), "duplicate contract file");
        }
    }
    // A forgotten index entry must not silently omit new fixtures or schemas from ports.
    let actual = asset_paths(directory, directory)?;
    ensure!(actual == files, "unlisted or missing contract assets");
    Ok(())
}

fn asset_paths(root: &Path, directory: &Path) -> Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        ensure!(
            !entry.file_type()?.is_symlink(),
            "contract assets cannot be symlinks"
        );
        if entry.file_type()?.is_dir() {
            files.extend(asset_paths(root, &path)?);
        } else if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("json" | "body")
        ) && path != root.join("corpus-index.json")
        {
            files.insert(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(files)
}

fn check_case_ids(value: &Value) -> Result<()> {
    let declared: Vec<String> = serde_json::from_value(value["caseIds"].clone())?;
    let cases = value["cases"]
        .as_array()
        .context("cases must be an array")?;
    let mut ids = BTreeSet::new();
    for case in cases {
        let id = case["id"].as_str().context("case must have a string ID")?;
        ensure!(
            !id.is_empty() && ids.insert(id.to_string()),
            "duplicate or empty case ID"
        );
    }
    ensure!(
        ids.len() == declared.len() && ids == declared.into_iter().collect(),
        "cases differ from declared case-ID set"
    );
    Ok(())
}
