//! Reads the `golem-py-sdk` wheel without running Python.
//!
//! The wheel contains the `golem` package, the prepared runtime and bindings
//! in `golem/_prepared`, and `golem/_compatibility.json`, which names the
//! componentize-py revision that prepared them. Reading the wheel checks
//! every file against the wheel's `RECORD`, the wheel's version against this
//! assembler's version, and the componentize-py revision against the one
//! that this assembler links. It then verifies the prepared assets and checks
//! that their identity is [`crate::PREPARED_IDENTITY`]. Nothing is regenerated
//! or compiled when a check fails.

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use async_zip::tokio::read::fs::ZipFileReader;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use componentize_py::prepared::Prepared;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// The distribution name of the SDK.
pub const DISTRIBUTION: &str = "golem-py-sdk";

/// The version of the SDK that this assembler builds with.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The componentize-py revision that this assembler links. It must equal the
/// revision of the workspace's `componentize-py` dependency.
pub const COMPONENTIZE_PY_REVISION: &str = "cad25eefaa090c158bca5ba5882c389eade4bcac";

const PACKAGE: &str = "golem/";
const PREPARED: &str = "golem/_prepared/";
const COMPATIBILITY: &str = "golem/_compatibility.json";

/// The SDK sources and verified prepared assets of a wheel.
pub struct Sdk {
    _dir: TempDir,
    sdk_dir: PathBuf,
    prepared: Prepared,
}

impl Sdk {
    /// The directory that contains the `golem` package.
    pub fn sdk_dir(&self) -> &Path {
        &self.sdk_dir
    }

    pub fn prepared(&self) -> &Prepared {
        &self.prepared
    }
}

/// Reads and verifies the SDK wheel at `wheel`.
pub async fn read_sdk_wheel(wheel: &Path) -> Result<Sdk> {
    read(wheel)
        .await
        .with_context(|| format!("cannot use the Python SDK wheel {}", wheel.display()))
}

async fn read(wheel: &Path) -> Result<Sdk> {
    let files = read_files(wheel).await?;
    let dist_info = dist_info(&files)?;
    verify_record(&files, &dist_info)?;

    let metadata = text(&files, &format!("{dist_info}METADATA"))?;
    let name = header(metadata, "Name")?;
    ensure!(
        name == DISTRIBUTION,
        "the wheel is `{name}`, not `{DISTRIBUTION}`"
    );
    let version = header(metadata, "Version")?;
    ensure!(
        version == SDK_VERSION,
        "the wheel is {DISTRIBUTION} {version}, but this Golem CLI builds with \
         {DISTRIBUTION} {SDK_VERSION}; depend on `{DISTRIBUTION}=={SDK_VERSION}` and \
         update the lock file, or use the Golem CLI release that matches {version}"
    );

    let compatibility: serde_json::Value =
        serde_json::from_str(text(&files, COMPATIBILITY)?).context(COMPATIBILITY)?;
    let revision = compatibility
        .get("componentize-py")
        .and_then(|revision| revision.as_str())
        .ok_or_else(|| anyhow!("{COMPATIBILITY} names no componentize-py revision"))?;
    ensure!(
        revision == COMPONENTIZE_PY_REVISION,
        "the prepared runtime in {DISTRIBUTION} {version} comes from componentize-py \
         {revision}, but this Golem CLI links componentize-py {COMPONENTIZE_PY_REVISION}; \
         use the {DISTRIBUTION} build that matches this Golem CLI"
    );

    let dir = tempfile::tempdir()?;
    let sdk_dir = dir.path().join("sdk");
    let prepared_dir = dir.path().join("prepared");
    for (name, bytes) in &files {
        let target = if let Some(relative) = name.strip_prefix(PREPARED) {
            prepared_dir.join(relative)
        } else if name.starts_with(PACKAGE) && name != COMPATIBILITY {
            sdk_dir.join(name)
        } else {
            continue;
        };
        fs::create_dir_all(target.parent().expect("a file path has a parent"))?;
        fs::write(&target, bytes)?;
    }
    let prepared = crate::read_prepared(&prepared_dir)?;
    Ok(Sdk {
        _dir: dir,
        sdk_dir,
        prepared,
    })
}

async fn read_files(wheel: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let reader = ZipFileReader::new(wheel)
        .await
        .context("the wheel is not a ZIP archive")?;
    let mut files = BTreeMap::new();
    for index in 0..reader.file().entries().len() {
        let entry = &reader.file().entries()[index];
        if entry.dir()? {
            continue;
        }
        let name = entry
            .filename()
            .as_str()
            .context("a file name is not UTF-8")?
            .to_string();
        ensure!(
            is_safe(&name),
            "the wheel contains the unsafe path `{name}`"
        );
        let mut bytes = Vec::new();
        reader
            .reader_with_entry(index)
            .await?
            .read_to_end_checked(&mut bytes)
            .await
            .with_context(|| format!("cannot read `{name}`"))?;
        ensure!(
            files.insert(name.clone(), bytes).is_none(),
            "the wheel contains `{name}` twice"
        );
    }
    Ok(files)
}

fn is_safe(name: &str) -> bool {
    !name.contains('\\')
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn dist_info(files: &BTreeMap<String, Vec<u8>>) -> Result<String> {
    let mut dirs: Vec<_> = files
        .keys()
        .filter_map(|name| name.split_once('/'))
        .map(|(dir, _)| dir)
        .filter(|dir| dir.ends_with(".dist-info"))
        .collect();
    dirs.dedup();
    match dirs.as_slice() {
        [dir] => Ok(format!("{dir}/")),
        [] => bail!("the wheel has no .dist-info directory"),
        _ => bail!("the wheel has more than one .dist-info directory"),
    }
}

/// Checks that `RECORD` lists every other file with its SHA-256 and size,
/// and only those files.
fn verify_record(files: &BTreeMap<String, Vec<u8>>, dist_info: &str) -> Result<()> {
    let record_name = format!("{dist_info}RECORD");
    let record = text(files, &record_name)?;
    let mut listed = BTreeMap::new();
    for line in record.lines().filter(|line| !line.is_empty()) {
        let mut fields = line.rsplitn(3, ',');
        let (size, hash, name) = (fields.next(), fields.next(), fields.next());
        let (Some(size), Some(hash), Some(name)) = (size, hash, name) else {
            bail!("RECORD has the malformed line `{line}`");
        };
        if name == record_name {
            continue;
        }
        let hash = hash
            .strip_prefix("sha256=")
            .ok_or_else(|| anyhow!("RECORD has no SHA-256 for `{name}`"))?;
        listed.insert(name, (hash, size));
    }
    for (name, bytes) in files {
        if *name == record_name {
            continue;
        }
        let (hash, size) = listed
            .remove(name.as_str())
            .ok_or_else(|| anyhow!("RECORD does not list `{name}`"))?;
        ensure!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)) == hash
                && size == bytes.len().to_string(),
            "`{name}` does not match RECORD; the wheel is corrupt, so download it again"
        );
    }
    if let Some(name) = listed.keys().next() {
        bail!("RECORD lists `{name}`, which the wheel does not contain");
    }
    Ok(())
}

fn text<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<&'a str> {
    let bytes = files
        .get(name)
        .ok_or_else(|| anyhow!("the wheel has no `{name}`"))?;
    std::str::from_utf8(bytes).with_context(|| format!("`{name}` is not UTF-8"))
}

fn header<'a>(metadata: &'a str, name: &str) -> Result<&'a str> {
    metadata
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
        .ok_or_else(|| anyhow!("METADATA has no `{name}`"))
}

#[cfg(test)]
mod tests {
    use super::{COMPONENTIZE_PY_REVISION, is_safe};
    use test_r::test;

    #[test]
    fn revision_matches_the_workspace() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        let read = |file: &str| std::fs::read_to_string(format!("{root}/{file}")).unwrap();
        let pins = |text: &str, marker: &str| -> Vec<String> {
            text.lines()
                .filter(|line| line.contains(marker))
                .map(str::to_string)
                .collect()
        };
        let cargo = pins(&read("Cargo.toml"), "github.com/golemcloud/componentize-py");
        assert_eq!(cargo.len(), 1, "{cargo:?}");
        assert!(
            cargo
                .iter()
                .all(|line| line.contains(&format!("rev = \"{COMPONENTIZE_PY_REVISION}\""))),
            "{cargo:?}"
        );
        assert_eq!(
            pins(&read("Makefile.toml"), "rev = set "),
            [format!("rev = set {COMPONENTIZE_PY_REVISION}")]
        );
    }

    #[test]
    fn unsafe_paths() {
        for safe in ["golem/__init__.py", "golem_py_sdk-0.0.0.dist-info/RECORD"] {
            assert!(is_safe(safe), "{safe}");
        }
        for unsafe_ in ["../x", "/abs", "golem/../../x", "golem\\x.py", "./golem"] {
            assert!(!is_safe(unsafe_), "{unsafe_}");
        }
    }
}
