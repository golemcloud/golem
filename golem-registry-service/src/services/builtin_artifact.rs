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

#[cfg(test)]
use crate::config::BuiltinArtifactSource;
use crate::config::{BuiltinArtifactsConfig, ResolvedBuiltinArtifact};
use crate::startup::{BuiltinArtifactKind, StartupPhase, StartupProgress};
use anyhow::{Context, anyhow};
use fd_lock::RwLock;
use golem_common::model::agent::extraction::{
    ExtractedComponentMetadata, extract_component_metadata_from_bytes,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use url::Url;

// Bump when metadata extraction semantics or its serialized representation changes.
const METADATA_CACHE_REVISION: u32 = 1;

/// A download or metadata extraction was cancelled before it finished, because the caller's
/// [`CancellationToken`] fired. This is a plain error, not a panic: it lets a resolution in
/// progress during process shutdown return promptly instead of leaving a blocking-pool thread
/// parked on a Tokio timer that the runtime tears down out from under it.
#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Lets a caller stop built-in artifact resolution and wait for it to actually stop, instead of
/// merely asking it to stop.
///
/// Cancelling [`CancellationToken`] alone only makes a `resolve`/`extract_metadata` call notice
/// cancellation and return early on its *next* poll; it does not stop a blocking-pool thread
/// that is already parked in `Handle::block_on` on a timer-bearing future, and an unbiased
/// `tokio::select!` can still poll that timer branch one more time after a wake-up races a
/// runtime shutdown — leaving a live Tokio timer for the runtime to tear down out from under it.
/// [`BuiltinArtifactShutdown::shutdown`] cancels the token and then awaits every tracked task, so
/// the caller does not proceed to drop the runtime while a task might still resume and touch a
/// timer the runtime is tearing down.
///
/// Clone and share this between a [`BuiltinArtifactResolver`] and the code that must shut it
/// down: both need the same `TaskTracker` so the tracker actually has every task registered when
/// the shutting-down side waits on it.
#[derive(Clone, Default)]
pub struct BuiltinArtifactShutdown {
    cancellation: CancellationToken,
    tasks: TaskTracker,
}

impl BuiltinArtifactShutdown {
    /// A fresh handle, not yet shut down. Clone it to share one `BuiltinArtifactResolver`
    /// between the resolver itself and the code that will later call [`Self::shutdown`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A handle that is never shut down: for tests and callers outside a startup race, where
    /// there is no signal-racing future to cancel resolution for. Equivalent to [`Self::new`];
    /// named separately so a call site reads as an explicit choice not to cancel.
    pub fn never() -> Self {
        Self::default()
    }

    /// Cancels outstanding and future resolutions, then waits for every `spawn_blocking` task
    /// started through this handle to actually return. Call this before returning control to
    /// code that will drop the owning Tokio runtime (such as a startup signal handler or a
    /// startup error path), so a task that is parked in `Handle::block_on` on a timer-bearing
    /// future gets to observe cancellation and unwind while the runtime is still alive, instead
    /// of racing the runtime's teardown and panicking when its timer is polled after shutdown.
    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }

    /// Exposes the tracker so a test can check it is already idle at a specific point, rather
    /// than only that `shutdown()` eventually completes (which an unawaited background drain
    /// racing the assertion could also satisfy).
    #[cfg(test)]
    fn tasks_for_test(&self) -> &TaskTracker {
        &self.tasks
    }
}

#[derive(Clone)]
pub struct BuiltinArtifactResolver {
    cache_dir: PathBuf,
    sources: BTreeMap<String, ResolvedBuiltinArtifact>,
    client: reqwest::Client,
    in_process_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    startup_progress: StartupProgress,
    shutdown: BuiltinArtifactShutdown,
}

impl BuiltinArtifactResolver {
    /// `shutdown` lets the caller stop an in-progress download or metadata extraction and wait
    /// for it to actually stop before proceeding to drop the owning Tokio runtime; see
    /// [`BuiltinArtifactShutdown`]. Pass [`BuiltinArtifactShutdown::never`] when nothing should
    /// ever cancel resolution, e.g. in tests or callers outside the startup path.
    pub fn new(
        config: &BuiltinArtifactsConfig,
        startup_progress: StartupProgress,
        shutdown: BuiltinArtifactShutdown,
    ) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "golem-registry-service/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(600))
            .build()
            .context("failed to create built-in artifact HTTP client")?;

        Ok(Self {
            cache_dir: config.resolved_cache_dir()?,
            sources: config.resolved_artifacts()?,
            client,
            in_process_locks: Arc::new(Mutex::new(HashMap::new())),
            startup_progress,
            shutdown,
        })
    }

    pub async fn resolve_many(
        &self,
        kind: BuiltinArtifactKind,
        artifact_ids: impl IntoIterator<Item = &'static str>,
    ) -> anyhow::Result<BTreeMap<&'static str, Arc<Vec<u8>>>> {
        let artifact_ids = artifact_ids.into_iter().collect::<BTreeSet<_>>();
        self.startup_progress
            .set_phase(StartupPhase::FetchingBuiltinArtifacts {
                kind,
                completed: 0,
                total: artifact_ids.len(),
                downloaded_bytes: 0,
            });
        let result =
            futures::future::try_join_all(artifact_ids.into_iter().map(|artifact_id| async move {
                let bytes = self.resolve(artifact_id).await?;
                self.startup_progress.step_completed();
                Ok::<_, anyhow::Error>((artifact_id, bytes))
            }))
            .await;
        let artifacts = match result {
            Ok(artifacts) => artifacts,
            Err(error) => {
                // `try_join_all` returns as soon as one resolution fails and drops the other
                // sibling futures still in flight. Dropping a sibling's `spawn_blocking` join
                // handle only detaches its task; the task keeps running, parked in
                // `Handle::block_on` on a timer-bearing future. Shutting down here, instead of
                // leaving that for the caller, makes sure every sibling has actually returned
                // before this function hands the error back to a caller that may go on to drop
                // the owning runtime.
                self.shutdown.shutdown().await;
                return Err(error);
            }
        };
        Ok(artifacts.into_iter().collect())
    }

    pub async fn resolve(&self, artifact_id: &str) -> anyhow::Result<Arc<Vec<u8>>> {
        let source =
            self.sources.get(artifact_id).cloned().ok_or_else(|| {
                anyhow!("no built-in artifact source configured for '{artifact_id}'")
            })?;
        #[cfg(not(feature = "embed-builtin-artifacts"))]
        let ResolvedBuiltinArtifact::Url(source) = source;
        #[cfg(feature = "embed-builtin-artifacts")]
        let source = match source {
            ResolvedBuiltinArtifact::Url(source) => source,
            ResolvedBuiltinArtifact::Embedded(compressed) => {
                return tokio::task::spawn_blocking(move || {
                    zstd::stream::decode_all(compressed).map(Arc::new)
                })
                .await
                .context("built-in artifact decompression task panicked")?
                .with_context(|| {
                    format!("failed to decompress built-in artifact '{artifact_id}'")
                });
            }
        };
        let url = Url::parse(&source.url).with_context(|| {
            format!("invalid URL configured for built-in artifact '{artifact_id}'")
        })?;
        let expected_sha256 = parse_sha256(artifact_id, source.sha256.as_deref())?;
        if url.scheme() == "file" {
            let path = url.to_file_path().map_err(|_| {
                anyhow!("invalid file URL for built-in artifact '{artifact_id}': {url}")
            })?;
            let bytes = tokio::fs::read(&path).await.with_context(|| {
                format!(
                    "failed to read built-in artifact '{artifact_id}' from '{}'; run cargo make ensure-builtin-artifacts to build local artifacts",
                    path.display()
                )
            })?;
            if let Some(expected) = expected_sha256 {
                let artifact_id = artifact_id.to_string();
                return tokio::task::spawn_blocking(move || {
                    let actual = Sha256::digest(&bytes).into();
                    verify_sha256(&artifact_id, Some(&expected), &actual)?;
                    Ok(Arc::new(bytes))
                })
                .await
                .context("built-in artifact checksum task panicked")?;
            }
            return Ok(Arc::new(bytes));
        }
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "built-in artifact '{artifact_id}' must use an HTTP, HTTPS or file URL"
        );

        let cache_key = cache_key(&url, expected_sha256.as_ref());
        let in_process_lock = {
            let mut locks = self.in_process_locks.lock().await;
            locks
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _in_process_guard = in_process_lock.lock().await;

        let cache_dir = self.cache_dir.clone();
        let client = self.client.clone();
        let artifact_id = artifact_id.to_string();
        let handle = Handle::current();
        let startup_progress = self.startup_progress.clone();
        let cancellation = self.shutdown.cancellation.clone();
        // Tracked, not a bare `tokio::task::spawn_blocking`: this closure calls
        // `handle.block_on` on a timer-bearing HTTP request future, so `shutdown` must be able to
        // wait for it to actually return, not just ask it to notice cancellation.
        let bytes = self
            .shutdown
            .tasks
            .spawn_blocking(move || {
                resolve_locked(
                    &handle,
                    &client,
                    &startup_progress,
                    &cancellation,
                    &cache_dir,
                    &artifact_id,
                    &url,
                    expected_sha256,
                    &cache_key,
                )
            })
            .await
            .context("built-in artifact resolver task panicked")??;

        Ok(Arc::new(bytes))
    }

    pub async fn extract_metadata(
        &self,
        wasm_bytes: Arc<Vec<u8>>,
    ) -> anyhow::Result<ExtractedComponentMetadata> {
        let (wasm_bytes, cache_key) = tokio::task::spawn_blocking(move || {
            let digest = hex::encode(Sha256::digest(wasm_bytes.as_slice()));
            (
                wasm_bytes,
                format!("{digest}.metadata-v{METADATA_CACHE_REVISION}"),
            )
        })
        .await
        .context("built-in artifact metadata hash task panicked")?;
        let in_process_lock = {
            let mut locks = self.in_process_locks.lock().await;
            locks
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _in_process_guard = in_process_lock.lock().await;
        let cache_dir = self.cache_dir.clone();
        let handle = Handle::current();
        let cancellation = self.shutdown.cancellation.clone();
        // Tracked for the same reason as the download above: this closure calls
        // `handle.block_on` on a future that may hold timers or I/O of its own.
        self.shutdown
            .tasks
            .spawn_blocking(move || {
                extract_metadata_locked(&handle, &cancellation, &cache_dir, &cache_key, &wasm_bytes)
            })
            .await
            .context("built-in artifact metadata extraction task panicked")?
    }
}

fn extract_metadata_locked(
    handle: &Handle,
    cancellation: &CancellationToken,
    cache_dir: &Path,
    cache_key: &str,
    wasm_bytes: &[u8],
) -> anyhow::Result<ExtractedComponentMetadata> {
    std::fs::create_dir_all(cache_dir).with_context(|| {
        format!(
            "failed to create built-in metadata cache directory '{}'",
            cache_dir.display()
        )
    })?;
    let cache_path = cache_dir.join(format!("{cache_key}.json"));
    let lock_path = cache_dir.join(format!("{cache_key}.lock"));
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "failed to open built-in metadata cache lock '{}'",
                lock_path.display()
            )
        })?;
    let mut lock = RwLock::new(lock_file);
    let _guard = lock.write().with_context(|| {
        format!(
            "failed to lock built-in metadata cache entry '{}'",
            lock_path.display()
        )
    })?;
    match std::fs::read(&cache_path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(metadata) => return Ok(metadata),
            Err(error) => tracing::warn!(
                cache_path = %cache_path.display(),
                error = %error,
                "Regenerating invalid cached built-in metadata"
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            cache_path = %cache_path.display(),
            error = %error,
            "Regenerating unreadable cached built-in metadata"
        ),
    }
    let metadata = handle
        .block_on(async {
            tokio::select! {
                result = extract_component_metadata_from_bytes(wasm_bytes, true, true) => result,
                () = cancellation.cancelled() => Err(anyhow::Error::new(Cancelled)),
            }
        })
        .with_context(|| {
            format!("failed to extract metadata for cached built-in artifact '{cache_key}'")
        })?;
    let mut temporary = tempfile::NamedTempFile::new_in(cache_dir)
        .context("failed to create temporary built-in metadata cache file")?;
    serde_json::to_writer(&mut temporary, &metadata)
        .context("failed to serialize built-in artifact metadata")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("failed to flush built-in artifact metadata")?;
    temporary.persist(&cache_path).with_context(|| {
        format!(
            "failed to publish built-in metadata cache entry '{}'",
            cache_path.display()
        )
    })?;
    Ok(metadata)
}

fn parse_sha256(artifact_id: &str, value: Option<&str>) -> anyhow::Result<Option<[u8; 32]>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let decoded = hex::decode(value).with_context(|| {
        format!("invalid SHA-256 configured for built-in artifact '{artifact_id}'")
    })?;
    let digest: [u8; 32] = decoded.try_into().map_err(|_| {
        anyhow!("SHA-256 for built-in artifact '{artifact_id}' must contain exactly 64 hex digits")
    })?;
    Ok(Some(digest))
}

fn cache_key(url: &Url, expected_sha256: Option<&[u8; 32]>) -> String {
    match expected_sha256 {
        Some(digest) => hex::encode(digest),
        None => format!(
            "url-{}",
            hex::encode(Sha256::digest(url.as_str().as_bytes()))
        ),
    }
}

fn resolve_locked(
    handle: &Handle,
    client: &reqwest::Client,
    startup_progress: &StartupProgress,
    cancellation: &CancellationToken,
    cache_dir: &Path,
    artifact_id: &str,
    url: &Url,
    expected_sha256: Option<[u8; 32]>,
    cache_key: &str,
) -> anyhow::Result<Vec<u8>> {
    std::fs::create_dir_all(cache_dir).with_context(|| {
        format!(
            "failed to create built-in artifact cache directory '{}'",
            cache_dir.display()
        )
    })?;

    let cache_path = cache_dir.join(format!("{cache_key}.wasm"));
    let lock_path = cache_dir.join(format!("{cache_key}.lock"));
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "failed to open built-in artifact cache lock '{}'",
                lock_path.display()
            )
        })?;
    let mut lock = RwLock::new(lock_file);
    let _guard = lock.write().with_context(|| {
        format!(
            "failed to lock built-in artifact cache entry '{}'",
            lock_path.display()
        )
    })?;

    if cache_path.is_file() {
        match read_and_verify(&cache_path, artifact_id, expected_sha256.as_ref()) {
            Ok(bytes) => {
                tracing::info!(
                    artifact_id,
                    cache_path = %cache_path.display(),
                    "Using cached built-in artifact"
                );
                return Ok(bytes);
            }
            Err(error) if expected_sha256.is_some() => {
                tracing::warn!(
                    artifact_id,
                    cache_path = %cache_path.display(),
                    error = %error,
                    "Discarding corrupt cached built-in artifact"
                );
                std::fs::remove_file(&cache_path).with_context(|| {
                    format!(
                        "failed to remove corrupt built-in artifact '{}' from '{}'",
                        artifact_id,
                        cache_path.display()
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }

    tracing::info!(artifact_id, "Downloading built-in artifact");
    let response = handle
        .block_on(async {
            tokio::select! {
                result = client.get(url.clone()).send() => result.map_err(anyhow::Error::new),
                () = cancellation.cancelled() => Err(anyhow::Error::new(Cancelled)),
            }
        })
        .with_context(|| format!("failed to download built-in artifact '{artifact_id}'"))?
        .error_for_status()
        .with_context(|| format!("failed to download built-in artifact '{artifact_id}'"))?;

    let mut response = response;
    let mut temporary = tempfile::NamedTempFile::new_in(cache_dir).with_context(|| {
        format!(
            "failed to create temporary file in built-in artifact cache '{}'",
            cache_dir.display()
        )
    })?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = handle
        .block_on(async {
            tokio::select! {
                result = response.chunk() => result.map_err(anyhow::Error::new),
                () = cancellation.cancelled() => Err(anyhow::Error::new(Cancelled)),
            }
        })
        .with_context(|| format!("failed while downloading built-in artifact '{artifact_id}'"))?
    {
        hasher.update(&chunk);
        startup_progress.builtin_artifact_bytes_downloaded(chunk.len() as u64);
        temporary
            .write_all(&chunk)
            .with_context(|| format!("failed to cache built-in artifact '{artifact_id}'"))?;
    }
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("failed to flush built-in artifact '{artifact_id}'"))?;

    let actual_sha256: [u8; 32] = hasher.finalize().into();
    verify_sha256(artifact_id, expected_sha256.as_ref(), &actual_sha256)?;
    temporary.persist(&cache_path).map_err(|error| {
        anyhow!(
            "failed to publish built-in artifact '{}' to cache '{}': {}",
            artifact_id,
            cache_path.display(),
            error.error
        )
    })?;

    tracing::info!(
        artifact_id,
        cache_path = %cache_path.display(),
        "Cached built-in artifact"
    );
    read_and_verify(&cache_path, artifact_id, expected_sha256.as_ref())
}

fn read_and_verify(
    path: &Path,
    artifact_id: &str,
    expected_sha256: Option<&[u8; 32]>,
) -> anyhow::Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| {
        format!(
            "failed to read cached built-in artifact '{}' from '{}'",
            artifact_id,
            path.display()
        )
    })?;
    if let Some(expected_sha256) = expected_sha256 {
        let actual_sha256: [u8; 32] = Sha256::digest(&bytes).into();
        verify_sha256(artifact_id, Some(expected_sha256), &actual_sha256)?;
    }
    Ok(bytes)
}

fn verify_sha256(
    artifact_id: &str,
    expected_sha256: Option<&[u8; 32]>,
    actual_sha256: &[u8; 32],
) -> anyhow::Result<()> {
    if let Some(expected_sha256) = expected_sha256 {
        anyhow::ensure!(
            expected_sha256 == actual_sha256,
            "SHA-256 mismatch for built-in artifact '{}': expected {}, got {}",
            artifact_id,
            hex::encode(expected_sha256),
            hex::encode(actual_sha256)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::listener::{Acceptor, Listener};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    #[cfg(feature = "embed-builtin-artifacts")]
    #[test]
    async fn builtin_artifact_embedded_defaults_resolve_without_source_files() {
        let cache = tempfile::tempdir().unwrap();
        let resolver = BuiltinArtifactResolver::new(
            &BuiltinArtifactsConfig {
                cache_dir: Some(cache.path().join("unused")),
                ..Default::default()
            },
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();
        // Expectations are compiled into the test; no checkout files are read at runtime.
        for (id, expected) in [
            (
                "web_fetch",
                include_bytes!("../../../builtin-tools/web-fetch.wasm").as_slice(),
            ),
            (
                "filesystem_tools",
                include_bytes!("../../../builtin-tools/filesystem-tools.wasm").as_slice(),
            ),
        ] {
            assert_eq!(resolver.resolve(id).await.unwrap().as_slice(), expected);
        }
        assert!(!cache.path().join("unused").exists());
    }

    #[cfg(golem_builtin_release)]
    #[test]
    #[test_r::timeout("30s")]
    async fn builtin_artifact_release_defaults_use_verified_cache() {
        let embedded = include_str!(concat!(env!("OUT_DIR"), "/builtin-artifacts.release.json"));
        assert!(
            !embedded.is_empty(),
            "release tests require an embedded manifest"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/builtin-artifacts.release.json"))
                .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(embedded).unwrap(),
            fixture,
            "run release tests with the checked-in fixture manifest"
        );

        let payload = b"component payload";
        let digest = hex::encode(Sha256::digest(payload));
        let cache = tempfile::tempdir().unwrap();
        let config = BuiltinArtifactsConfig {
            cache_dir: Some(cache.path().to_path_buf()),
            ..Default::default()
        };
        assert!(config.source_overrides.is_empty());
        let mut resolver = BuiltinArtifactResolver::new(
            &config,
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();
        // Keep cache misses offline without changing the embedded HTTPS sources.
        let unavailable_proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        resolver.client = reqwest::Client::builder()
            .proxy(
                reqwest::Proxy::all(format!(
                    "http://{}",
                    unavailable_proxy.local_addr().unwrap()
                ))
                .unwrap(),
            )
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let cache_path = cache.path().join(format!("{digest}.wasm"));
        std::fs::write(&cache_path, payload).unwrap();
        assert_eq!(resolver.sources.len(), 8);
        for (id, entry) in fixture["artifacts"].as_object().unwrap() {
            let ResolvedBuiltinArtifact::Url(source) = &resolver.sources[id];
            let component = entry["component"].as_str().unwrap();
            assert_eq!(
                source.url,
                format!(
                    "https://github.com/golemcloud/golem-builtins/releases/download/{component}-v1.2.3/{component}.wasm"
                )
            );
            assert_eq!(source.sha256.as_deref(), Some(digest.as_str()));
            assert_eq!(resolver.resolve(id).await.unwrap().as_slice(), payload);
        }

        std::fs::write(&cache_path, b"corrupt").unwrap();
        let error = resolver.resolve("python").await.unwrap_err();
        assert!(
            error.to_string().contains("failed to download"),
            "{error:#}"
        );
        assert!(
            !cache_path.exists(),
            "corrupt pinned cache must be discarded"
        );
    }

    #[test]
    async fn file_sources_observe_replacements_and_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("replacement component.wasm");
        let cache = directory.path().join("unused-cache");
        let mut config = BuiltinArtifactsConfig {
            cache_dir: Some(cache.clone()),
            source_overrides: BTreeMap::from([(
                "python".to_string(),
                BuiltinArtifactSource {
                    url: Url::from_file_path(&path).unwrap().into(),
                    sha256: None,
                },
            )]),
        };
        let resolver = BuiltinArtifactResolver::new(
            &config,
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();
        std::fs::write(&path, b"first").unwrap();
        assert_eq!(
            resolver.resolve("python").await.unwrap().as_slice(),
            b"first"
        );
        std::fs::write(&path, b"second").unwrap();
        assert_eq!(
            resolver.resolve("python").await.unwrap().as_slice(),
            b"second"
        );
        config.source_overrides.get_mut("python").unwrap().sha256 =
            Some(hex::encode(Sha256::digest(b"second")));
        let pinned = BuiltinArtifactResolver::new(
            &config,
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();
        assert_eq!(
            pinned.resolve("python").await.unwrap().as_slice(),
            b"second"
        );
        std::fs::write(&path, b"third").unwrap();
        assert!(
            pinned
                .resolve("python")
                .await
                .unwrap_err()
                .to_string()
                .contains("SHA-256 mismatch")
        );
        std::fs::remove_file(&path).unwrap();
        let error = resolver.resolve("python").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cargo make ensure-builtin-artifacts")
        );
        assert!(error.to_string().contains("replacement component.wasm"));
        assert!(!cache.exists());
    }

    #[test]
    fn rejects_invalid_sha256() {
        let error = parse_sha256("test", Some("abcd")).unwrap_err();
        assert!(error.to_string().contains("64 hex digits"), "{error:#}");
    }

    #[test]
    fn content_addressed_cache_keys_ignore_the_url() {
        let digest = [7; 32];
        let first = Url::parse("https://example.com/first").unwrap();
        let second = Url::parse("https://example.com/second").unwrap();
        assert_eq!(
            cache_key(&first, Some(&digest)),
            cache_key(&second, Some(&digest))
        );
        assert_ne!(cache_key(&first, None), cache_key(&second, None));
    }

    #[test]
    fn validates_cached_content() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact.wasm");
        std::fs::write(&path, b"component").unwrap();
        let expected: [u8; 32] = Sha256::digest(b"component").into();
        assert_eq!(
            read_and_verify(&path, "test", Some(&expected)).unwrap(),
            b"component"
        );

        let error = read_and_verify(&path, "test", Some(&[0; 32])).unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"), "{error:#}");
    }

    #[test]
    async fn concurrent_resolves_download_once_and_reuse_the_cache() {
        let payload = b"component payload".to_vec();
        let expected = hex::encode(Sha256::digest(&payload));
        let requests = Arc::new(AtomicUsize::new(0));
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await
            .unwrap();
        let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
        let endpoint = poem::endpoint::make({
            let payload = payload.clone();
            let requests = requests.clone();
            move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                let payload = payload.clone();
                async move { poem::Response::builder().body(payload) }
            }
        });
        let server = tokio::spawn(async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(endpoint)
                .await
                .unwrap();
        });

        let cache = tempfile::tempdir().unwrap();
        let config = BuiltinArtifactsConfig {
            cache_dir: Some(cache.path().to_path_buf()),
            source_overrides: BTreeMap::from([(
                "test".to_string(),
                BuiltinArtifactSource {
                    url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                    sha256: Some(expected.clone()),
                },
            )]),
        };
        let resolver = BuiltinArtifactResolver::new(
            &config,
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();
        let (first, second) = tokio::join!(resolver.resolve("test"), resolver.resolve("test"));
        assert_eq!(first.unwrap().as_slice(), payload);
        assert_eq!(second.unwrap().as_slice(), payload);
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        server.abort();
        let offline_config = BuiltinArtifactsConfig {
            cache_dir: config.cache_dir,
            source_overrides: BTreeMap::from([(
                "test".to_string(),
                BuiltinArtifactSource {
                    url: "http://127.0.0.1:1/unreachable.wasm".to_string(),
                    sha256: Some(expected),
                },
            )]),
        };
        let offline_resolver = BuiltinArtifactResolver::new(
            &offline_config,
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();
        assert_eq!(
            offline_resolver.resolve("test").await.unwrap().as_slice(),
            payload
        );
    }

    /// Regression test for the SIGABRT reported in
    /// <https://github.com/golemcloud/golem/issues/4575>: a listener that accepts the connection
    /// but never sends a response holds `resolve`'s blocking thread inside `handle.block_on` on
    /// the request future, which keeps a live Tokio timer (the client's 600s timeout) in flight.
    /// `BuiltinArtifactShutdown::shutdown` must make `resolve` return a plain error and the
    /// blocking task actually finish promptly, instead of hanging until that timeout.
    ///
    /// This exercises the full production sequence, not just cancellation: it awaits
    /// `shutdown()` the same way `command_handler.rs` does on the real shutdown path, so it
    /// would not pass with a fix that only cancels the token without waiting for the tracked
    /// task to return (that leaves a window where the task's own `block_on` can still observe a
    /// runtime shutdown racing in from elsewhere and panic).
    #[test]
    #[test_r::timeout("10s")]
    async fn shutdown_stops_a_connecting_download_promptly() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accept connections and hold them open without ever writing a response, like the
        // loopback HTTPS endpoint in the GitHub issue's second reproducer, but over plain TCP so
        // the test needs no TLS setup: the client blocks the same way while reading the
        // response headers. An explicit stop signal (rather than relying on thread detachment)
        // lets the test join the acceptor thread and actually release the listener and any held
        // connections before returning.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        listener.set_nonblocking(true).unwrap();
        let held_connections = {
            let stop = stop.clone();
            let accepted = accepted.clone();
            std::thread::spawn(move || {
                let mut held = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            held.push(stream);
                            accepted.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };

        let cache = tempfile::tempdir().unwrap();
        let config = BuiltinArtifactsConfig {
            cache_dir: Some(cache.path().to_path_buf()),
            source_overrides: BTreeMap::from([(
                "test".to_string(),
                BuiltinArtifactSource {
                    url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                    sha256: None,
                },
            )]),
        };
        let shutdown = BuiltinArtifactShutdown::new();
        let resolver =
            BuiltinArtifactResolver::new(&config, StartupProgress::default(), shutdown.clone())
                .unwrap();

        let resolve = tokio::spawn(async move { resolver.resolve("test").await });
        // Wait for the listener to actually accept the connection before shutting down, so the
        // test exercises cancellation of a download that is already connecting rather than one
        // that has not started.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while accepted.load(Ordering::SeqCst) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the test listener never accepted a connection"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // The same call `command_handler.rs` makes on the real shutdown path: cancel, then wait
        // for the tracked blocking task to actually return, before this function (like `main`)
        // would otherwise go on to drop the runtime.
        tokio::time::timeout(Duration::from_secs(5), shutdown.shutdown())
            .await
            .expect("shutdown must not hang");

        // `now_or_never`, not a timeout with a grace period: a `shutdown()` that only cancels
        // without waiting would still let the tracked blocking closure notice cancellation and
        // return within any such grace period on its own, which would not actually show that
        // `shutdown()` itself waited for it as required. `tasks_for_test` observes the tracked
        // task directly; `resolve` is a thin `tokio::spawn` wrapper the scheduler still needs a
        // separate poll to propagate through, so it gets a short bounded wait instead, without
        // weakening what this test actually proves.
        use futures::FutureExt;
        assert!(
            shutdown.tasks_for_test().wait().now_or_never().is_some(),
            "the tracked blocking task must already be finished once shutdown() returns, not \
             merely cancelled and left to finish on its own time",
        );

        let error = tokio::time::timeout(Duration::from_millis(500), resolve)
            .await
            .expect("the resolver task must finish promptly once its tracked work has")
            .expect("the resolver task must not panic")
            .expect_err("a cancelled download must fail, not succeed");
        assert!(
            error.to_string().contains("failed to download"),
            "{error:#}"
        );

        stop.store(true, Ordering::SeqCst);
        held_connections.join().unwrap();
    }

    #[test]
    async fn resolve_many_reports_fetch_progress() {
        let payload = b"component payload".to_vec();
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await
            .unwrap();
        let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
        let endpoint = poem::endpoint::make({
            let payload = payload.clone();
            move |_| {
                let payload = payload.clone();
                async move { poem::Response::builder().body(payload) }
            }
        });
        let server = tokio::spawn(async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(endpoint)
                .await
                .unwrap();
        });

        let cache = tempfile::tempdir().unwrap();
        let progress = StartupProgress::default();
        let resolver = BuiltinArtifactResolver::new(
            &BuiltinArtifactsConfig {
                cache_dir: Some(cache.path().to_path_buf()),
                source_overrides: BTreeMap::from([(
                    "test".to_string(),
                    BuiltinArtifactSource {
                        url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                        sha256: Some(hex::encode(Sha256::digest(&payload))),
                    },
                )]),
            },
            progress.clone(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();

        resolver
            .resolve_many(BuiltinArtifactKind::Plugins, ["test"])
            .await
            .unwrap();
        assert_eq!(
            progress.current(),
            StartupPhase::FetchingBuiltinArtifacts {
                kind: BuiltinArtifactKind::Plugins,
                completed: 1,
                total: 1,
                downloaded_bytes: payload.len() as u64,
            }
        );

        server.abort();
    }

    /// Regression test for the second abort path reported against this fix on
    /// <https://github.com/golemcloud/golem/issues/4575>: `resolve_many` resolves several
    /// artifacts concurrently with `try_join_all`, which returns as soon as the first one fails
    /// and drops the other sibling futures still in flight. Dropping a sibling's `spawn_blocking`
    /// join handle only detaches its task; the task keeps running, parked in `handle.block_on`
    /// on a timer-bearing future, unless `resolve_many` itself also shuts the resolver down
    /// before returning the error. This test needs two concurrent artifacts, one that fails
    /// immediately and one that stays connecting, to exercise that interleaving; a single
    /// failed download does not reach the code path this covers.
    #[test]
    #[test_r::timeout("10s")]
    async fn resolve_many_drains_a_stalled_sibling_after_another_fails() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        listener.set_nonblocking(true).unwrap();
        let held_connections = {
            let stop = stop.clone();
            let accepted = accepted.clone();
            std::thread::spawn(move || {
                let mut held = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            held.push(stream);
                            accepted.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };

        let cache = tempfile::tempdir().unwrap();
        let shutdown = BuiltinArtifactShutdown::new();
        let resolver = BuiltinArtifactResolver::new(
            &BuiltinArtifactsConfig {
                cache_dir: Some(cache.path().to_path_buf()),
                source_overrides: BTreeMap::from([
                    (
                        // Nothing listens on port 1, so this fails as soon as the connection is
                        // refused, well before the "stalling" sibling below finishes connecting.
                        "fails_immediately".to_string(),
                        BuiltinArtifactSource {
                            url: "http://127.0.0.1:1/unreachable.wasm".to_string(),
                            sha256: None,
                        },
                    ),
                    (
                        "stalls_connecting".to_string(),
                        BuiltinArtifactSource {
                            url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                            sha256: None,
                        },
                    ),
                ]),
            },
            StartupProgress::default(),
            shutdown.clone(),
        )
        .unwrap();

        let resolve_many = tokio::spawn({
            let resolver = resolver.clone();
            async move {
                resolver
                    .resolve_many(
                        BuiltinArtifactKind::Tools,
                        ["fails_immediately", "stalls_connecting"],
                    )
                    .await
            }
        });

        let error = tokio::time::timeout(Duration::from_secs(5), resolve_many)
            .await
            .expect("resolve_many must not hang waiting for the stalled sibling to time out")
            .expect("the resolve_many task must not panic")
            .expect_err("one artifact failing must fail resolve_many as a whole");
        assert!(
            error.to_string().contains("fails_immediately")
                || error.to_string().contains("unreachable"),
            "the original failure's error must be preserved, got: {error:#}"
        );

        // By the time `resolve_many` has returned its error, the stalled sibling's tracked task
        // must have already stopped on its own: `self.shutdown.tasks` must already be at zero,
        // so closing and waiting on it resolves immediately rather than needing to wait for a
        // task that is still running. A bare `CancellationToken::cancel()` with no drain, or no
        // drain on the error path at all, leaves that task still parked in `handle.block_on` at
        // this point, racing whatever drops the runtime next — exactly the second abort path
        // this test guards against. `now_or_never` makes that distinction observable: it
        // returns `None` if the tracker is not yet idle, instead of waiting for it to become so.
        use futures::FutureExt;
        shutdown.tasks_for_test().close();
        assert!(
            shutdown.tasks_for_test().wait().now_or_never().is_some(),
            "resolve_many must already have drained the stalled sibling before returning its \
             error, not merely cancelled it"
        );

        stop.store(true, Ordering::SeqCst);
        held_connections.join().unwrap();
    }

    #[test]
    async fn checksum_mismatch_does_not_publish_the_download() {
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await
            .unwrap();
        let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
        let server = tokio::spawn(async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(poem::endpoint::make(|_| async {
                    poem::Response::builder().body("unexpected")
                }))
                .await
                .unwrap();
        });
        let cache = tempfile::tempdir().unwrap();
        let expected = hex::encode([0; 32]);
        let cache_path = cache.path().join(format!("{expected}.wasm"));
        std::fs::write(&cache_path, b"corrupt").unwrap();
        let resolver = BuiltinArtifactResolver::new(
            &BuiltinArtifactsConfig {
                cache_dir: Some(cache.path().to_path_buf()),
                source_overrides: BTreeMap::from([(
                    "test".to_string(),
                    BuiltinArtifactSource {
                        url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                        sha256: Some(expected.clone()),
                    },
                )]),
            },
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();

        let error = resolver.resolve("test").await.unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"), "{error:#}");
        assert!(!cache_path.exists());
        server.abort();
    }

    #[test]
    async fn corrupt_checksum_pinned_cache_is_redownloaded() {
        let payload = b"replacement component".to_vec();
        let expected = hex::encode(Sha256::digest(&payload));
        let requests = Arc::new(AtomicUsize::new(0));
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await
            .unwrap();
        let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
        let endpoint = poem::endpoint::make({
            let payload = payload.clone();
            let requests = requests.clone();
            move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                let payload = payload.clone();
                async move { poem::Response::builder().body(payload) }
            }
        });
        let server = tokio::spawn(async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(endpoint)
                .await
                .unwrap();
        });

        let cache = tempfile::tempdir().unwrap();
        let cache_path = cache.path().join(format!("{expected}.wasm"));
        std::fs::write(&cache_path, b"corrupt").unwrap();
        let resolver = BuiltinArtifactResolver::new(
            &BuiltinArtifactsConfig {
                cache_dir: Some(cache.path().to_path_buf()),
                source_overrides: BTreeMap::from([(
                    "test".to_string(),
                    BuiltinArtifactSource {
                        url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                        sha256: Some(expected),
                    },
                )]),
            },
            StartupProgress::default(),
            BuiltinArtifactShutdown::never(),
        )
        .unwrap();

        assert_eq!(resolver.resolve("test").await.unwrap().as_slice(), payload);
        assert_eq!(std::fs::read(cache_path).unwrap(), payload);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
