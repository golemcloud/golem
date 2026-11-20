// Copyright 2024-2026 Golem Cloud
// SPDX-License-Identifier: LicenseRef-Golem-Source-License-1.1

use crate::app::context::find_main_source_from;
use crate::app::edit::golem_yaml::set_package_pin;
use crate::command::GolemCliGlobalFlags;
use crate::model::app_raw;
use anyhow::{Context, ensure};
pub use golem_package::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod preparation;
pub(crate) use preparation::clean_roots;
pub(crate) use preparation::{PackagePreparation, PreparedPackages};

const LOCK_NAME: &str = "golem.lock";
const MAX_LOCK_BYTES: u64 = 512 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct PackageDigest {
    pub digest: String,
}

impl crate::model::cli_output::StructuredOutput for PackageDigest {
    const KIND: &'static str = "package.digest";
}

impl crate::model::text_format::TextOutput for PackageDigest {
    fn log(&self) {
        crate::log::print_line(&self.digest);
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PackageInstallResult {
    pub name: String,
    pub version: String,
    pub digest: String,
}

impl crate::model::cli_output::StructuredOutput for PackageInstallResult {
    const KIND: &'static str = "package.install";
}

impl crate::model::text_format::TextOutput for PackageInstallResult {}

fn stage_write(path: &Path, bytes: &[u8]) -> anyhow::Result<tempfile::NamedTempFile> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.is_file()
    {
        file.as_file().set_permissions(metadata.permissions())?;
    }
    file.write_all(bytes)?;
    Ok(file)
}

pub(crate) fn write_package(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    (|| -> anyhow::Result<()> {
        stage_write(path, bytes)?.persist(path)?;
        Ok(())
    })()
    .with_context(|| format!("Failed to write {}", path.display()))
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Lock {
    #[serde(deserialize_with = "deserialize_pins")]
    packages: Pins,
}

pub(crate) fn deserialize_pins<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Pins, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Pins;
        fn expecting(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            fmt.write_str("package pins by unique package name")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Pins, M::Error> {
            use serde::de::Error;
            let mut pins = Pins::new();
            while let Some(name) = map.next_key::<String>()? {
                if pins.len() >= MAX_PACKAGES {
                    return Err(M::Error::custom("Too many package pins"));
                }
                let pin = map.next_value().map_err(|err| {
                    M::Error::custom(format!("Invalid package pin {name:?}: {err}"))
                })?;
                if pins.insert(name.clone(), pin).is_some() {
                    return Err(M::Error::custom(format!("Duplicate package pin {name:?}")));
                }
            }
            Ok(pins)
        }
    }
    deserializer.deserialize_map(Visitor)
}

pub(crate) fn load_lock(root: &Path) -> anyhow::Result<Pins> {
    Ok(read_lock(root)?.0.packages)
}

fn read_lock(root: &Path) -> anyhow::Result<(Lock, Option<Vec<u8>>)> {
    let path = root.join(LOCK_NAME);
    (|| -> anyhow::Result<_> {
        match read_regular(&path, MAX_LOCK_BYTES) {
            Ok(bytes) => Ok((serde_yaml::from_slice(&bytes)?, Some(bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((Lock::default(), None))
            }
            Err(error) => Err(error.into()),
        }
    })()
    .with_context(|| {
        format!(
            "Cannot read {}; repair it and run `golem install <file>`",
            path.display()
        )
    })
}

pub(crate) fn install(
    flags: &GolemCliGlobalFlags,
    file: &Path,
) -> anyhow::Result<PackageInstallResult> {
    ensure!(
        !flags.disable_app_manifest_discovery,
        "Installing a package requires an application manifest"
    );
    let cwd = crate::fs::current_dir_lexical()?;
    let manifest_path = match &flags.app_manifest_path {
        Some(path) => crate::fs::absolute_lexical_path_from_base_dir(path, &cwd),
        None => find_main_source_from(&cwd).context("No application manifest found")?,
    };
    let root = manifest_path
        .parent()
        .context("Manifest has no parent directory")?;
    let path = crate::fs::absolute_lexical_path_from_base_dir(file, &cwd);
    let package = read(open_regular(&path)?)
        .with_context(|| format!("Cannot read package {}", path.display()))?;
    let identity = &package.config().package;
    let pin = FilePin {
        source: match path.strip_prefix(root) {
            Ok(relative) => crate::fs::path_to_unix_str(relative)?,
            Err(_) => crate::fs::path_to_str(&path)?.into(),
        },
        version: identity.version.clone(),
        digest: package.digest().into(),
    };

    crate::log::log_action(
        "Installing",
        format!(
            "{}@{} ({}) from {}: {} component(s)",
            identity.name,
            identity.version,
            pin.digest,
            pin.source,
            package.config().components.len()
        ),
    );

    let manifest = fs::read_to_string(&manifest_path)?;
    let mut expected = app_raw::Application::from_yaml_str(&manifest)?.packages;
    let updated = set_package_pin(&manifest, &identity.name, &pin)?;
    expected.insert(identity.name.clone(), pin.clone());
    ensure!(
        app_raw::Application::from_yaml_str(&updated)?.packages == expected,
        "Could not update the package pins in the manifest"
    );

    let (Lock { packages: mut lock }, previous_lock) = read_lock(root)?;
    lock.insert(identity.name.clone(), pin);
    // Validate the installed pin without requiring this command to repair unrelated pins.
    let one = Pins::from([(identity.name.clone(), expected[&identity.name].clone())]);
    check_lock(&one, &one)?;
    ensure!(
        lock.len() <= MAX_PACKAGES,
        "Too many packages in golem.lock"
    );
    let lock_bytes = serde_yaml::to_string(&Lock { packages: lock })?;
    ensure!(
        lock_bytes.len() as u64 <= MAX_LOCK_BYTES,
        "golem.lock exceeds size limit"
    );
    save_install(
        &manifest_path,
        updated.as_bytes(),
        &root.join(LOCK_NAME),
        lock_bytes.as_bytes(),
        previous_lock.as_deref(),
    )?;
    Ok(PackageInstallResult {
        name: identity.name.clone(),
        version: identity.version.clone(),
        digest: package.digest().into(),
    })
}

fn save_install(
    manifest_path: &Path,
    manifest: &[u8],
    lock_path: &Path,
    lock: &[u8],
    previous_lock: Option<&[u8]>,
) -> anyhow::Result<()> {
    let rollback = previous_lock
        .map(|bytes| stage_write(lock_path, bytes))
        .transpose()?;
    let staged_lock = stage_write(lock_path, lock)?;
    let staged_manifest = stage_write(manifest_path, manifest)?;
    staged_lock.persist(lock_path)?;
    if let Err(error) = staged_manifest.persist(manifest_path) {
        let restored = match rollback {
            Some(previous) => previous.persist(lock_path).map(|_| ()).map_err(|e| e.error),
            None => fs::remove_file(lock_path),
        };
        restored.with_context(|| format!("Could not restore golem.lock after failing to save the manifest ({error}); rerun `golem install <file>`"))?;
        return Err(error).context("Could not save the manifest; golem.lock was restored");
    }
    Ok(())
}

pub(crate) struct LocalEffects<'a> {
    pub app_root: &'a Path,
    pub cache_root: PathBuf,
}

impl LocalEffects<'_> {
    fn cache_path(&self, digest: &str) -> PathBuf {
        self.cache_root.join(&digest["sha256:".len()..])
    }
}

#[async_trait::async_trait]
impl PackageReader for LocalEffects<'_> {
    fn authorize_file(&self, _name: &str, _pin: &FilePin) -> anyhow::Result<()> {
        Ok(())
    }

    async fn cached(&mut self, digest: &str, limit: u64) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(read_regular(&self.cache_path(digest), limit).ok())
    }

    async fn fetch_file(&mut self, pin: &FilePin, limit: u64) -> anyhow::Result<Vec<u8>> {
        let path = self.app_root.join(&pin.source);
        read_regular(&path, limit).with_context(|| format!("Cannot fetch {}", path.display()))
    }
}

#[async_trait::async_trait]
impl PackageEffects for LocalEffects<'_> {
    async fn cache(&mut self, digest: &str, bytes: &[u8]) -> anyhow::Result<()> {
        fs::create_dir_all(&self.cache_root)?;
        write_package(&self.cache_path(digest), bytes)
    }

    async fn create_dir(&mut self, root: &Path, relative: &Path) -> anyhow::Result<()> {
        Ok(fs::create_dir_all(root.join(relative))?)
    }

    async fn write_file(
        &mut self,
        root: &Path,
        relative: &Path,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().context("Unpack path has no parent")?)?;
        File::create_new(path)?.write_all(bytes)?;
        Ok(())
    }
}

fn open_regular(path: &Path) -> std::io::Result<File> {
    let not_a_file =
        || std::io::Error::new(std::io::ErrorKind::InvalidData, "Expected a regular file");
    if !fs::metadata(path)?.is_file() {
        return Err(not_a_file());
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(not_a_file());
    }
    Ok(file)
}

fn read_regular(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = open_regular(path)?;
    let too_large =
        || std::io::Error::new(std::io::ErrorKind::InvalidData, "File exceeds size limit");
    if file.metadata()?.len() > limit {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn failed_manifest_replace_restores_the_original_lock() {
        let app = tempfile::tempdir().unwrap();
        let manifest = app.path().join("golem.yaml");
        fs::create_dir(&manifest).unwrap(); // A directory cannot be replaced with a file.
        let lock = app.path().join(LOCK_NAME);
        assert!(save_install(&manifest, b"new manifest", &lock, b"new lock", None).is_err());
        assert!(!lock.exists());
        fs::write(&lock, b"original lock").unwrap();
        assert!(
            save_install(
                &manifest,
                b"new manifest",
                &lock,
                b"new lock",
                Some(b"original lock")
            )
            .is_err()
        );
        assert_eq!(fs::read(lock).unwrap(), b"original lock");
    }
}
