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

//! Installs the Go toolchain that Go components are built with.
//!
//! Go components are compiled by componentize-go, which needs a Go runtime
//! carrying patches that upstream Go does not have yet (see
//! [`crate::versions::go_toolchain`]). componentize-go can download such a
//! toolchain itself, but it pins its own version, never replaces one it already
//! downloaded, and prefers whatever `go` is on PATH. So the CLI installs the
//! toolchain Golem pins into the very directory componentize-go looks in, and
//! puts it first on PATH for every Go command it runs: componentize-go then
//! finds an acceptable `go` immediately and downloads nothing.

use crate::fs;
use crate::log::{LogColorize, log_action};
use crate::model::app::ApplicationConfig;
use anyhow::{Context, anyhow, bail};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Points the CLI at an already installed toolchain instead of downloading the
/// pinned release. Used to test a change to the Go fork before releasing it.
pub const GO_TOOLCHAIN_ENV_VAR: &str = "GOLEM_GO_TOOLCHAIN";

/// Records which release is installed in a toolchain directory. componentize-go
/// keeps no such marker, so a directory without one was put there by
/// componentize-go (or an older CLI) and is replaced.
const INSTALLED_TAG_FILE_NAME: &str = "GOLEM_GO_TOOLCHAIN_TAG";

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct GoToolchain {
    pub root: PathBuf,
    pub bin_dir: PathBuf,
    pub go: PathBuf,
}

impl GoToolchain {
    fn at(root: PathBuf) -> Self {
        let bin_dir = root.join("bin");
        let go = bin_dir.join(go_binary_name());
        Self { root, bin_dir, go }
    }
}

/// Makes the pinned Go toolchain available, downloading it if needed, and
/// returns where it lives.
pub async fn ensure_go_toolchain(config: &ApplicationConfig) -> anyhow::Result<GoToolchain> {
    if let Some(root) = go_toolchain_override()? {
        return Ok(GoToolchain::at(root));
    }

    let toolchain = GoToolchain::at(install_dir()?);
    if is_installed(&toolchain) {
        return Ok(toolchain);
    }

    if config.offline {
        bail!(
            "The Golem Go toolchain ({tag}) is not installed, and offline mode is enabled.\n\
             Install it by building without --offline, or set {env} to a Go toolchain \
             that has it: {url}",
            tag = crate::versions::go_toolchain::TAG,
            env = GO_TOOLCHAIN_ENV_VAR,
            url = download_url(),
        );
    }

    // componentize-go guards the same directory with this lock while it
    // downloads its own toolchain, so taking it keeps a concurrent build (or a
    // direct componentize-go invocation) from writing the directory underneath
    // us.
    let parent_dir = toolchain
        .root
        .parent()
        .ok_or_else(|| {
            anyhow!(
                "Invalid Go toolchain directory: {}",
                toolchain.root.display()
            )
        })?
        .to_path_buf();
    fs::create_dir_all(&parent_dir)?;
    let lock = File::create(parent_dir.join("lock"))?;
    lock.lock()?;

    // Another process may have installed it while we waited for the lock.
    if is_installed(&toolchain) {
        return Ok(toolchain);
    }

    install(&toolchain, &parent_dir).await?;

    Ok(toolchain)
}

fn is_installed(toolchain: &GoToolchain) -> bool {
    if !toolchain.go.exists() {
        return false;
    }
    match std::fs::read_to_string(toolchain.root.join(INSTALLED_TAG_FILE_NAME)) {
        Ok(tag) => tag.trim() == crate::versions::go_toolchain::TAG,
        Err(_) => false,
    }
}

async fn install(toolchain: &GoToolchain, parent_dir: &Path) -> anyhow::Result<()> {
    let url = download_url();

    log_action(
        "Installing",
        format!(
            "the Golem Go toolchain {}, downloading {}",
            crate::versions::go_toolchain::TAG.log_color_highlight(),
            url.log_color_highlight()
        ),
    );

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let response = client
        .get(&url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .with_context(|| anyhow!("Failed to download the Golem Go toolchain from {url}"))?;
    let archive = response
        .bytes()
        .await
        .with_context(|| anyhow!("Failed to download the Golem Go toolchain from {url}"))?;

    let staging_dir = parent_dir.join(format!("{}.incoming", archive_stem()));
    if staging_dir.exists() {
        fs::remove(&staging_dir)?;
    }
    fs::create_dir_all(&staging_dir)?;

    // The archive holds a single top-level directory named after the asset.
    tar::Archive::new(bzip2::read::BzDecoder::new(archive.as_ref()))
        .unpack(&staging_dir)
        .with_context(|| anyhow!("Failed to extract the Golem Go toolchain from {url}"))?;

    let unpacked = staging_dir.join(archive_stem());
    if !unpacked.join("bin").join(go_binary_name()).exists() {
        bail!("The Golem Go toolchain downloaded from {url} has no bin/go");
    }
    let mut marker = File::create(unpacked.join(INSTALLED_TAG_FILE_NAME))?;
    writeln!(marker, "{}", crate::versions::go_toolchain::TAG)?;
    drop(marker);

    // Replaces whatever was there: an older Golem release, or the unpinned
    // toolchain componentize-go downloaded for itself.
    if toolchain.root.exists() {
        fs::remove(&toolchain.root)?;
    }
    fs::rename(&unpacked, &toolchain.root)?;
    fs::remove(&staging_dir)?;

    Ok(())
}

fn go_toolchain_override() -> anyhow::Result<Option<PathBuf>> {
    let Some(root) = std::env::var_os(GO_TOOLCHAIN_ENV_VAR) else {
        return Ok(None);
    };
    let root = PathBuf::from(root);
    let go = root.join("bin").join(go_binary_name());
    if !go.exists() {
        bail!(
            "{} is set to {}, which is not a Go toolchain: {} does not exist",
            GO_TOOLCHAIN_ENV_VAR,
            root.display(),
            go.display()
        );
    }
    Ok(Some(root))
}

/// The directory componentize-go installs its own Go toolchain into, and looks
/// in before downloading one.
fn install_dir() -> anyhow::Result<PathBuf> {
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow!("Failed to determine the user's cache directory"))?;
    Ok(cache_dir
        .join("componentize-go")
        .join("v2")
        .join(archive_stem()))
}

fn download_url() -> String {
    format!(
        "https://github.com/{repo}/releases/download/{tag}/{stem}.tbz",
        repo = crate::versions::go_toolchain::REPO,
        tag = crate::versions::go_toolchain::TAG,
        stem = archive_stem(),
    )
}

fn archive_stem() -> String {
    format!("go-{}-{}-bootstrap", go_os(), go_arch())
}

fn go_binary_name() -> &'static str {
    if cfg!(windows) { "go.exe" } else { "go" }
}

fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    }
}

fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn archive_stem_uses_go_naming() {
        let stem = archive_stem();
        assert!(stem.starts_with("go-"), "{stem}");
        assert!(stem.ends_with("-bootstrap"), "{stem}");
        assert!(!stem.contains("macos"), "{stem}");
        assert!(!stem.contains("x86_64"), "{stem}");
        assert!(!stem.contains("aarch64"), "{stem}");
    }

    #[test]
    fn download_url_points_at_the_pinned_release() {
        let url = download_url();
        assert!(
            url.starts_with("https://github.com/golemcloud/go/releases/download/"),
            "{url}"
        );
        assert!(url.contains(crate::versions::go_toolchain::TAG), "{url}");
        assert!(url.ends_with(".tbz"), "{url}");
    }

    #[test]
    fn install_dir_is_the_directory_componentize_go_looks_in() {
        let dir = install_dir().unwrap();
        assert!(dir.ends_with(archive_stem()), "{}", dir.display());
        assert_eq!(
            dir.parent().unwrap().file_name().unwrap(),
            "v2",
            "{}",
            dir.display()
        );
        assert_eq!(
            dir.parent().unwrap().parent().unwrap().file_name().unwrap(),
            "componentize-go",
            "{}",
            dir.display()
        );
    }

    #[test]
    fn installed_only_with_a_matching_tag() {
        let dir = tempfile::tempdir().unwrap();
        let toolchain = GoToolchain::at(dir.path().join(archive_stem()));

        assert!(!is_installed(&toolchain));

        std::fs::create_dir_all(&toolchain.bin_dir).unwrap();
        std::fs::write(&toolchain.go, b"").unwrap();
        assert!(
            !is_installed(&toolchain),
            "a toolchain without a tag marker is not ours"
        );

        std::fs::write(
            toolchain.root.join(INSTALLED_TAG_FILE_NAME),
            "go0.0.0-golem.0\n",
        )
        .unwrap();
        assert!(!is_installed(&toolchain), "a different release is not ours");

        std::fs::write(
            toolchain.root.join(INSTALLED_TAG_FILE_NAME),
            format!("{}\n", crate::versions::go_toolchain::TAG),
        )
        .unwrap();
        assert!(is_installed(&toolchain));
    }

    #[test]
    fn override_must_point_at_a_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var(GO_TOOLCHAIN_ENV_VAR, dir.path()) };
        let error = go_toolchain_override().unwrap_err().to_string();
        unsafe { std::env::remove_var(GO_TOOLCHAIN_ENV_VAR) };
        assert!(error.contains(GO_TOOLCHAIN_ENV_VAR), "{error}");
    }
}
