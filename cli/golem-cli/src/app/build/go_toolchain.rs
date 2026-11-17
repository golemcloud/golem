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
//!
//! It also installs the preview1 adapter componentize-go wraps the module with
//! (see [`crate::versions::go_adapter`]), which the build passes as `--adapt`.

use crate::fs;
use crate::log::{LogColorize, log_action};
use crate::model::app::ApplicationConfig;
use anyhow::{Context, anyhow, bail};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Points the CLI at an already installed toolchain instead of downloading the
/// pinned release. Used to test a change to the Go fork before releasing it.
pub const GO_TOOLCHAIN_ENV_VAR: &str = "GOLEM_GO_TOOLCHAIN";

/// Points the CLI at a local preview1 adapter instead of downloading the pinned
/// release. Used to test a change to the adapter before releasing it.
pub const GO_ADAPTER_ENV_VAR: &str = "GOLEM_GO_ADAPTER";

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
    if let Some(root) = go_toolchain_override(std::env::var_os(GO_TOOLCHAIN_ENV_VAR))? {
        return Ok(GoToolchain::at(root));
    }

    let toolchain = GoToolchain::at(install_dir()?);
    if is_installed(&toolchain) {
        return Ok(toolchain);
    }

    let Some(expected_sha256) = archive_sha256() else {
        bail!(
            "The Golem Go toolchain ({tag}) has no build for {os}/{arch}; it is published for {supported}.\n\
             Set {env} to a toolchain built from https://github.com/{repo} for this platform.",
            tag = crate::versions::go_toolchain::TAG,
            os = go_os(),
            arch = go_arch(),
            supported = crate::versions::go_toolchain::ARCHIVES
                .iter()
                .map(|(os, arch, _)| format!("{os}/{arch}"))
                .collect::<Vec<_>>()
                .join(", "),
            env = GO_TOOLCHAIN_ENV_VAR,
            repo = crate::versions::go_toolchain::REPO,
        );
    };

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
    // Another process may hold the lock for a whole download; wait for it off
    // the async workers.
    let lock = tokio::task::spawn_blocking(move || lock.lock().map(|()| lock))
        .await
        .context("Failed to wait for the Go toolchain lock")??;

    // Another process may have installed it while we waited for the lock.
    if is_installed(&toolchain) {
        return Ok(toolchain);
    }

    install(&toolchain, &parent_dir, expected_sha256).await?;
    drop(lock);

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

async fn install(
    toolchain: &GoToolchain,
    parent_dir: &Path,
    expected_sha256: &str,
) -> anyhow::Result<()> {
    let url = download_url();

    log_action(
        "Installing",
        format!(
            "the Golem Go toolchain {}, downloading {}",
            crate::versions::go_toolchain::TAG.log_color_highlight(),
            url.log_color_highlight()
        ),
    );

    let archive = download(&url)
        .await
        .with_context(|| anyhow!("Failed to download the Golem Go toolchain from {url}"))?;
    verify_archive(&archive, expected_sha256, &url)?;

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

async fn download(url: &str) -> anyhow::Result<bytes::Bytes> {
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    Ok(client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())?
        .bytes()
        .await?)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Makes the pinned preview1 adapter available, downloading it if needed, and
/// returns its path.
pub async fn ensure_go_adapter(config: &ApplicationConfig) -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os(GO_ADAPTER_ENV_VAR) {
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!(
                "{} is set to {}, which is not a file",
                GO_ADAPTER_ENV_VAR,
                path.display()
            );
        }
        return Ok(path);
    }

    let path = adapter_path()?;
    if adapter_installed(&path) {
        return Ok(path);
    }

    let url = adapter_download_url();
    if config.offline {
        bail!(
            "The Golem preview1 adapter ({tag}) is not installed, and offline mode is enabled.\n\
             Install it by building without --offline, or set {env} to a copy of {url}",
            tag = crate::versions::go_adapter::TAG,
            env = GO_ADAPTER_ENV_VAR,
        );
    }

    log_action(
        "Installing",
        format!(
            "the Golem preview1 adapter {}, downloading {}",
            crate::versions::go_adapter::TAG.log_color_highlight(),
            url.log_color_highlight()
        ),
    );
    let adapter = download(&url)
        .await
        .with_context(|| anyhow!("Failed to download the Golem preview1 adapter from {url}"))?;
    verify_adapter(&adapter, crate::versions::go_adapter::SHA256, &url)?;

    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid adapter path: {}", path.display()))?;
    fs::create_dir_all(dir)?;
    // A concurrent build may install the same file; each writes its own copy
    // and the rename replaces the file atomically.
    let incoming = dir.join(format!(
        "{}.{}.incoming",
        crate::versions::go_adapter::ASSET,
        std::process::id()
    ));
    fs::write(&incoming, &adapter)?;
    fs::rename(&incoming, &path)?;

    Ok(path)
}

fn adapter_installed(path: &Path) -> bool {
    std::fs::read(path)
        .map(|adapter| sha256_hex(&adapter) == crate::versions::go_adapter::SHA256)
        .unwrap_or(false)
}

/// Fails unless `adapter` hashes to the pinned sha256, so a corrupted download
/// or a replaced release asset is never built into a component.
fn verify_adapter(adapter: &[u8], expected_sha256: &str, url: &str) -> anyhow::Result<()> {
    let actual = sha256_hex(adapter);
    if actual != expected_sha256 {
        bail!(
            "The Golem preview1 adapter downloaded from {url} does not match the checksum this \
             CLI pins for {tag}: expected sha256 {expected_sha256}, got {actual}. Retry the \
             build; if it keeps failing, set {env} to an adapter built from https://github.com/{repo}.",
            tag = crate::versions::go_adapter::TAG,
            env = GO_ADAPTER_ENV_VAR,
            repo = crate::versions::go_adapter::REPO,
        );
    }
    Ok(())
}

fn adapter_path() -> anyhow::Result<PathBuf> {
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow!("Failed to determine the user's cache directory"))?;
    Ok(cache_dir
        .join("golem")
        .join("go-adapter")
        .join(crate::versions::go_adapter::TAG)
        .join(crate::versions::go_adapter::ASSET))
}

fn adapter_download_url() -> String {
    format!(
        "https://github.com/{repo}/releases/download/{tag}/{asset}",
        repo = crate::versions::go_adapter::REPO,
        tag = crate::versions::go_adapter::TAG,
        asset = crate::versions::go_adapter::ASSET,
    )
}

/// Where `--adapt` goes in the arguments of a `go` command, or `None` when the
/// command does not wrap a module into a component with componentize-go or
/// already names an adapter. `args` follow the `go` program itself.
pub fn componentize_go_adapter_position(args: &[String]) -> Option<usize> {
    match args {
        [tool, componentize_go, subcommand, rest @ ..]
            if tool == "tool"
                && componentize_go == "componentize-go"
                && matches!(subcommand.as_str(), "build" | "test")
                && !rest
                    .iter()
                    .any(|arg| arg == "--adapt" || arg.starts_with("--adapt=")) =>
        {
            Some(3)
        }
        _ => None,
    }
}

/// Fails unless `archive` hashes to the sha256 pinned for this platform, so a
/// corrupted download or a replaced release asset is never unpacked and run.
fn verify_archive(archive: &[u8], expected_sha256: &str, url: &str) -> anyhow::Result<()> {
    let actual = sha256_hex(archive);
    if actual != expected_sha256 {
        bail!(
            "The Golem Go toolchain downloaded from {url} does not match the checksum this CLI pins \
             for {tag}: expected sha256 {expected_sha256}, got {actual}. Retry the build; if it \
             keeps failing, set {env} to a toolchain built from https://github.com/{repo}.",
            tag = crate::versions::go_toolchain::TAG,
            env = GO_TOOLCHAIN_ENV_VAR,
            repo = crate::versions::go_toolchain::REPO,
        );
    }
    Ok(())
}

/// The toolchain `GOLEM_GO_TOOLCHAIN` points at, given that variable's value.
fn go_toolchain_override(value: Option<OsString>) -> anyhow::Result<Option<PathBuf>> {
    let Some(root) = value else {
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

/// The pinned sha256 of this platform's bootstrap archive, or `None` when the
/// release publishes no archive for this platform.
fn archive_sha256() -> Option<&'static str> {
    crate::versions::go_toolchain::ARCHIVES
        .iter()
        .find(|(os, arch, _)| *os == go_os() && *arch == go_arch())
        .map(|(_, _, sha256)| *sha256)
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
        assert!(go_toolchain_override(None).unwrap().is_none());
        let error = go_toolchain_override(Some(dir.path().as_os_str().to_owned()))
            .unwrap_err()
            .to_string();
        assert!(error.contains(GO_TOOLCHAIN_ENV_VAR), "{error}");
    }

    #[test]
    fn an_archive_must_match_its_pinned_checksum() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        verify_archive(b"", empty, "https://example.test/go.tbz").unwrap();
        let error = verify_archive(b"tampered", empty, "https://example.test/go.tbz")
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match the checksum"), "{error}");
        assert!(error.contains(empty), "{error}");
    }

    fn args(command: &str) -> Vec<String> {
        shlex::split(command).unwrap()[1..].to_vec()
    }

    #[test]
    fn the_template_builds_get_the_adapter_after_the_subcommand() {
        let template = include_str!("../../../templates/go/common-on-demand/golem.yaml");
        let builds: Vec<&str> = template
            .lines()
            .filter_map(|line| line.trim().strip_prefix("- command: "))
            .filter(|command| command.starts_with("go tool componentize-go"))
            .collect();
        assert_eq!(builds.len(), 2, "{builds:?}");
        for build in builds {
            assert_eq!(
                componentize_go_adapter_position(&args(build)),
                Some(3),
                "{build}"
            );
        }
        assert_eq!(
            componentize_go_adapter_position(&args("go tool componentize-go test --wasip1")),
            Some(3)
        );
    }

    #[test]
    fn other_go_commands_and_named_adapters_are_left_alone() {
        for command in [
            "go mod tidy",
            "go build ./...",
            "go tool componentize-go bindings -o out",
            "go tool other build",
            "go tool componentize-go build --adapt my.wasm -o out.wasm",
            "go tool componentize-go build -o out.wasm --adapt=my.wasm",
        ] {
            assert_eq!(
                componentize_go_adapter_position(&args(command)),
                None,
                "{command}"
            );
        }
    }

    #[test]
    fn an_adapter_must_match_its_pinned_checksum() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        verify_adapter(b"", empty, "https://example.test/adapter.wasm").unwrap();
        let error = verify_adapter(b"tampered", empty, "https://example.test/adapter.wasm")
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match the checksum"), "{error}");
        assert!(error.contains(GO_ADAPTER_ENV_VAR), "{error}");

        let pinned = crate::versions::go_adapter::SHA256;
        assert_eq!(pinned.len(), 64);
        assert!(
            pinned
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn adapter_url_points_at_the_pinned_release() {
        assert_eq!(
            adapter_download_url(),
            format!(
                "https://github.com/golemcloud/wasmtime/releases/download/{}/wasi_snapshot_preview1.reactor.wasm",
                crate::versions::go_adapter::TAG
            )
        );
    }

    #[test]
    fn every_pinned_archive_has_a_sha256() {
        for (os, arch, sha256) in crate::versions::go_toolchain::ARCHIVES {
            assert_eq!(sha256.len(), 64, "{os}/{arch}");
            assert!(
                sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{os}/{arch}"
            );
        }
    }
}
