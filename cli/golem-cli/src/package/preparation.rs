// Copyright 2024-2026 Golem Cloud
// SPDX-License-Identifier: LicenseRef-Golem-Source-License-1.1

use super::{LocalEffects, PackageInspection};
use crate::app::build::task_result_marker::TaskResultMarker;
use anyhow::ensure;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_PREPARATION_OUTPUTS: usize = 16_384;

/// A read-only package inventory, separate from component recipes and their execution.
#[derive(Clone, Debug)]
pub(crate) struct PackagePreparation {
    inspection: PackageInspection,
    identity: String,
    root: PathBuf,
    cleanup: BTreeSet<PathBuf>,
    manifest: PathBuf,
    manifest_digest: String,
    app_root: PathBuf,
    cache_root: PathBuf,
}

pub(crate) struct PreparedPackages {
    inputs: PackagePreparation,
}

impl PackagePreparation {
    pub fn inspection(&self) -> &PackageInspection {
        &self.inspection
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn inspect(
        app_root: &Path,
        cache_root: PathBuf,
        manifest: &Path,
        pins: &super::Pins,
        lock: &super::Pins,
    ) -> anyhow::Result<Self> {
        let mut effects = LocalEffects::new(app_root, cache_root.clone());
        let inspection = golem_package::inspect(pins, lock, &mut effects).await?;
        let identity = inspection.identity()?;
        let manifest_digest = format!(
            "sha256:{:x}",
            Sha256::digest(super::read_regular(manifest, super::MAX_PACKAGE_BYTES)?)
        );
        let parent = crate::model::app::Application::package_preparation_dir(app_root);
        let root = parent.join(format!(
            "root-{}",
            identity.strip_prefix("sha256:").expect("SHA-256 identity")
        ));
        for path in [
            parent.parent().expect("temporary directory"),
            &parent,
            &root,
        ] {
            match fs::symlink_metadata(path) {
                Ok(metadata) => ensure!(
                    metadata.is_dir(),
                    "Invalid package output directory {}",
                    path.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        let mut cleanup = BTreeSet::new();
        match fs::read_dir(&parent) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if entry.file_name().as_encoded_bytes().starts_with(b"root-")
                        && entry.file_type()?.is_dir()
                        && entry.path() != root
                    {
                        ensure!(
                            cleanup.len() < MAX_PREPARATION_OUTPUTS,
                            "Package cleanup candidate limit exceeded"
                        );
                        cleanup.insert(entry.path());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        ensure!(
            2 + pins.len() + inspection.assets().len() + cleanup.len() <= MAX_PREPARATION_OUTPUTS,
            "Package output inventory limit exceeded"
        );
        ensure!(
            serde_json::to_vec(inspection.declarations())?.len() as u64 <= super::MAX_PACKAGE_BYTES,
            "Package declaration snapshot limit exceeded"
        );
        Ok(Self {
            inspection,
            identity,
            root,
            cleanup,
            manifest: manifest.into(),
            manifest_digest,
            app_root: app_root.into(),
            cache_root,
        })
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join("package-manifest.json")
    }

    pub fn inputs(&self) -> Vec<PathBuf> {
        let mut inputs = BTreeSet::from([self.app_root.join("golem.lock"), self.manifest.clone()]);
        for pin in self.inspection.pins().values() {
            if let super::PackageSource::File(path) = super::PackageSource::parse(&pin.source) {
                inputs.insert(self.app_root.join(path));
            }
            inputs.insert(
                self.cache_root
                    .join(pin.digest.strip_prefix("sha256:").expect("checked pin")),
            );
        }
        inputs.into_iter().collect()
    }

    pub fn outputs(&self) -> Vec<PathBuf> {
        let mut outputs = vec![self.root.clone(), self.manifest()];
        outputs.extend(
            self.inspection
                .assets()
                .keys()
                .map(|path| self.root.join(path)),
        );
        outputs.extend(self.inspection.pins().values().map(|pin| {
            self.cache_root
                .join(pin.digest.strip_prefix("sha256:").expect("checked pin"))
        }));
        outputs
    }

    pub fn log(&self) {
        crate::log::log_action("Planned", "package preparation (not applied)");
        crate::log::logln(format!(
            "  package inputs: {}; files={:?}; outputs={:?}; effects=read sources, write cache/assets, remove obsolete roots; conditional cleanup={:?}; assets={:?}",
            self.identity,
            self.inputs(),
            self.outputs(),
            self.cleanup,
            self.inspection.assets()
        ));
    }

    pub async fn revalidate(&self) -> anyhow::Result<()> {
        ensure!(
            format!(
                "sha256:{:x}",
                Sha256::digest(super::read_regular(
                    &self.manifest,
                    super::MAX_PACKAGE_BYTES
                )?)
            ) == self.manifest_digest,
            "Package manifest input changed; reload the project before execution"
        );
        let lock = super::load_lock(&self.app_root)?;
        let mut effects = LocalEffects::new(&self.app_root, self.cache_root.clone());
        let current = golem_package::inspect(self.inspection.pins(), &lock, &mut effects).await?;
        ensure!(
            current.identity()? == self.identity,
            "Package preparation inputs changed"
        );
        Ok(())
    }

    pub async fn execute(
        &self,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<(Arc<PreparedPackages>, bool)> {
        self.revalidate().await?;
        let parent = self.root.parent().expect("unpack parent");
        for path in [parent.parent().expect("temporary directory"), parent] {
            match fs::create_dir(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    ensure!(
                        fs::symlink_metadata(path)?.is_dir(),
                        "Invalid package output directory {}",
                        path.display()
                    );
                }
                Err(error) => return Err(error.into()),
            }
        }
        let reused = self.root.exists() && self.verify_outputs().is_ok();
        if self.root.exists() && !reused {
            fs::remove_dir_all(&self.root)?;
        }
        for root in &self.cleanup {
            if root.try_exists()? {
                fs::remove_dir_all(root)?;
            }
        }
        if !reused {
            self.result_marker()?
                .run_async(async || {
                    fs::create_dir(&self.root)?;
                    let mut effects = LocalEffects::new(&self.app_root, self.cache_root.clone())
                        .with_cache_population(cancellation);
                    golem_package::materialize(&self.inspection, &self.root, &mut effects).await?;
                    File::create_new(self.manifest())?
                        .write_all(&serde_json::to_vec(self.inspection.declarations())?)?;
                    Ok(())
                })
                .await?;
        }
        Ok((
            Arc::new(PreparedPackages {
                inputs: self.clone(),
            }),
            reused,
        ))
    }

    fn result_marker(&self) -> anyhow::Result<TaskResultMarker> {
        Ok(TaskResultMarker::new(
            &self
                .app_root
                .join(crate::model::app::TEMP_DIR)
                .join("task-results"),
            self,
        )?
        .track_outputs(self.root.clone()))
    }

    pub fn verify_outputs(&self) -> anyhow::Result<()> {
        ensure!(
            self.result_marker()?.is_up_to_date(),
            "Package outputs are incomplete or changed"
        );
        Ok(())
    }
}

impl PreparedPackages {
    pub async fn validate(&self, current: &PackagePreparation) -> anyhow::Result<()> {
        ensure!(
            self.inputs.identity == current.identity && self.inputs.root == current.root,
            "Prepared package inputs changed; prepare the current packages before building"
        );
        current.revalidate().await?;
        current.verify_outputs()
    }
}

impl crate::app::build::task_result_marker::TaskResultMarkerHashSource for &PackagePreparation {
    fn kind() -> &'static str {
        "PackagePreparation"
    }

    fn id(&self) -> anyhow::Result<Option<String>> {
        Ok(Some(self.identity.clone()))
    }

    fn source(
        &self,
    ) -> anyhow::Result<crate::app::build::task_result_marker::TaskResultMarkerHashSourceKind> {
        Ok(
            crate::app::build::task_result_marker::TaskResultMarkerHashSourceKind::HashFromString(
                serde_json::to_string(&(
                    &self.identity,
                    &self.manifest_digest,
                    &self.root,
                    &self.cache_root,
                ))?,
            ),
        )
    }
}
