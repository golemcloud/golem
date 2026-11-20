// Copyright 2024-2026 Golem Cloud
// SPDX-License-Identifier: LicenseRef-Golem-Source-License-1.1

use super::{LocalEffects, PackageAsset, PackageInspection};
use anyhow::{Context, ensure};
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
    pub inspection: PackageInspection,
    pub identity: String,
    pub root: PathBuf,
    pub cleanup: BTreeSet<PathBuf>,
    manifest: PathBuf,
    manifest_digest: String,
    app_root: PathBuf,
    cache_root: PathBuf,
}

pub(crate) struct PreparedPackages {
    inputs: PackagePreparation,
    _lease: File,
}

impl PackagePreparation {
    pub async fn inspect(
        app_root: &Path,
        cache_root: PathBuf,
        manifest: &Path,
        pins: &super::Pins,
        lock: &super::Pins,
    ) -> anyhow::Result<Self> {
        let mut effects = LocalEffects {
            app_root,
            cache_root: cache_root.clone(),
        };
        let inspection = golem_package::inspect(pins, lock, &mut effects).await?;
        let identity = inspection.identity()?;
        let manifest_digest = format!(
            "sha256:{:x}",
            Sha256::digest(super::read_regular(manifest, super::MAX_PACKAGE_BYTES)?)
        );
        let parent = app_root
            .join(crate::model::app::TEMP_DIR)
            .join("package-unpack");
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
        for path in [parent.join("registry.lock"), root.join("lease")] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) => ensure!(
                    metadata.is_file(),
                    "Invalid package lock {}",
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
            4 + pins.len() + inspection.assets().len() + cleanup.len() <= MAX_PREPARATION_OUTPUTS,
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
            inputs.insert(self.app_root.join(&pin.source));
            inputs.insert(
                self.cache_root
                    .join(pin.digest.strip_prefix("sha256:").expect("checked pin")),
            );
        }
        inputs.into_iter().collect()
    }

    pub fn outputs(&self) -> Vec<PathBuf> {
        let mut outputs = vec![
            self.root.clone(),
            self.root.join("lease"),
            self.manifest(),
            self.registry(),
        ];
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
            "  package inputs: {}; files={:?}; outputs={:?}; effects=read files, write cache/assets/leases, remove inactive roots; conditional cleanup={:?}; assets={:?}",
            self.identity,
            self.inputs(),
            self.outputs(),
            self.cleanup,
            self.inspection.assets()
        ));
    }

    fn registry(&self) -> PathBuf {
        self.root
            .parent()
            .expect("unpack parent")
            .join("registry.lock")
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
        let mut effects = LocalEffects {
            app_root: &self.app_root,
            cache_root: self.cache_root.clone(),
        };
        let current = golem_package::inspect(self.inspection.pins(), &lock, &mut effects).await?;
        ensure!(
            current.identity()? == self.identity,
            "Package preparation inputs changed"
        );
        Ok(())
    }

    pub async fn execute(&self) -> anyhow::Result<(Arc<PreparedPackages>, bool)> {
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
        let registry_path = self.registry();
        match fs::symlink_metadata(&registry_path) {
            Ok(metadata) => ensure!(metadata.is_file(), "Invalid package registry lock"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let registry = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&registry_path)?;
        registry.lock()?;
        if self.root.exists() && self.verify_outputs().is_err() {
            remove_inactive(&self.root)?;
            ensure!(
                !self.root.exists(),
                "Cannot repair package outputs while another consumer holds their lease"
            );
        }
        for root in &self.cleanup {
            remove_inactive(root)?;
        }
        let existing = match fs::symlink_metadata(&self.root) {
            Ok(metadata) => {
                ensure!(metadata.is_dir(), "Package root is not a directory");
                let lease = open_lease(&self.root)?;
                lease.lock_shared()?;
                self.verify_outputs()?;
                Some(lease)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let reused = existing.is_some();
        let lease = match existing {
            Some(lease) => lease,
            None => {
                fs::create_dir(&self.root)?;
                let lease = File::options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(self.root.join("lease"))?;
                lease.lock()?;
                let mut effects = LocalEffects {
                    app_root: &self.app_root,
                    cache_root: self.cache_root.clone(),
                };
                golem_package::materialize(&self.inspection, &self.root, &mut effects).await?;
                File::create_new(self.manifest())?
                    .write_all(&serde_json::to_vec(self.inspection.declarations())?)?;
                lease.unlock()?;
                lease.lock_shared()?;
                lease
            }
        };
        Ok((
            Arc::new(PreparedPackages {
                inputs: self.clone(),
                _lease: lease,
            }),
            reused,
        ))
    }

    pub fn verify_outputs(&self) -> anyhow::Result<()> {
        ensure!(
            fs::symlink_metadata(&self.root)?.is_dir(),
            "Invalid package output directory"
        );
        for lock in [self.registry(), self.root.join("lease")] {
            ensure!(
                fs::symlink_metadata(lock)?.is_file(),
                "Invalid package output lock"
            );
        }
        for pin in self.inspection.pins().values() {
            let path = self
                .cache_root
                .join(pin.digest.strip_prefix("sha256:").expect("checked pin"));
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "Invalid package cache output"
            );
            let package =
                super::read(super::read_regular(&path, super::MAX_PACKAGE_BYTES)?.as_slice())?;
            ensure!(
                package.digest() == pin.digest,
                "Package cache output changed"
            );
        }
        for (relative, asset) in self.inspection.assets() {
            let path = self.root.join(relative);
            let mut parent = path.as_path();
            while parent != self.root {
                ensure!(
                    !fs::symlink_metadata(parent)?.is_symlink(),
                    "Symlink in package output {}",
                    parent.display()
                );
                parent = parent.parent().context("Package output leaves its root")?;
            }
            match asset {
                PackageAsset::Directory { .. } => ensure!(
                    fs::symlink_metadata(&path)?.is_dir(),
                    "Package directory changed"
                ),
                PackageAsset::File { digest, size, .. } => {
                    let metadata = fs::symlink_metadata(&path)?;
                    ensure!(
                        metadata.is_file() && metadata.len() == *size,
                        "Package asset size changed"
                    );
                    let bytes = super::read_regular(&path, *size)?;
                    ensure!(
                        format!("sha256:{:x}", Sha256::digest(bytes)) == *digest,
                        "Package asset digest changed"
                    );
                }
            }
        }
        ensure!(
            fs::symlink_metadata(self.manifest())?.is_file(),
            "Package declaration snapshot changed"
        );
        ensure!(
            super::read_regular(&self.manifest(), super::MAX_PACKAGE_BYTES)?
                == serde_json::to_vec(self.inspection.declarations())?,
            "Package declaration snapshot changed"
        );
        for entry in walkdir::WalkDir::new(&self.root).min_depth(1) {
            let entry = entry?;
            let relative = entry.path().strip_prefix(&self.root)?;
            ensure!(
                relative == Path::new("lease")
                    || relative == Path::new("package-manifest.json")
                    || self.inspection.assets().contains_key(relative)
                    || (entry.file_type().is_dir()
                        && self
                            .inspection
                            .assets()
                            .keys()
                            .any(|asset| asset.starts_with(relative))),
                "Unexpected package output {}",
                entry.path().display()
            );
        }
        Ok(())
    }
}

/// Keep the registry namespace stable while clean removes admitted, inactive roots.
pub(crate) fn clean_roots(app_root: &Path) -> anyhow::Result<Option<File>> {
    let parent = app_root
        .join(crate::model::app::TEMP_DIR)
        .join("package-unpack");
    if !parent.try_exists()? {
        return Ok(None);
    }
    ensure!(
        fs::symlink_metadata(&parent)?.is_dir(),
        "Invalid package cleanup directory"
    );
    let registry_path = parent.join("registry.lock");
    match fs::symlink_metadata(&registry_path) {
        Ok(metadata) => ensure!(metadata.is_file(), "Invalid package registry lock"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let registry = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(registry_path)?;
    registry.lock()?;
    let mut roots = Vec::new();
    for entry in fs::read_dir(&parent)? {
        let entry = entry?;
        if entry.file_name().as_encoded_bytes().starts_with(b"root-") && entry.file_type()?.is_dir()
        {
            ensure!(
                roots.len() < MAX_PREPARATION_OUTPUTS,
                "Package cleanup candidate limit exceeded"
            );
            let root = entry.path();
            let lease = match open_lease(&root) {
                Ok(lease) => {
                    lease.try_lock().context(
                        "Cannot clean package outputs while another consumer holds their lease",
                    )?;
                    Some(lease)
                }
                Err(_error)
                    if root
                        .join("lease")
                        .symlink_metadata()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    None
                }
                Err(error) => return Err(error),
            };
            roots.push((root, lease));
        }
    }
    for (root, _lease) in roots {
        fs::remove_dir_all(root)?;
    }
    Ok(Some(registry))
}

fn open_lease(root: &Path) -> anyhow::Result<File> {
    let path = root.join("lease");
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "Invalid package lease"
    );
    Ok(File::options().read(true).write(true).open(path)?)
}

fn remove_inactive(root: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) => ensure!(metadata.is_dir(), "Invalid package cleanup root"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let lease = match open_lease(root) {
        Ok(lease) => Some(lease),
        Err(_error)
            if root
                .join("lease")
                .symlink_metadata()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(lease) = &lease {
        match lease.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    fs::remove_dir_all(root)?;
    Ok(())
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
