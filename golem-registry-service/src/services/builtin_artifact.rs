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
use url::Url;

// Bump when metadata extraction semantics or its serialized representation changes.
const METADATA_CACHE_REVISION: u32 = 1;

#[derive(Clone)]
pub struct BuiltinArtifactResolver {
    cache_dir: PathBuf,
    sources: BTreeMap<String, ResolvedBuiltinArtifact>,
    client: reqwest::Client,
    in_process_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    startup_progress: StartupProgress,
}

impl BuiltinArtifactResolver {
    pub fn new(
        config: &BuiltinArtifactsConfig,
        startup_progress: StartupProgress,
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
        let artifacts =
            futures::future::try_join_all(artifact_ids.into_iter().map(|artifact_id| async move {
                let bytes = self.resolve(artifact_id).await?;
                self.startup_progress.step_completed();
                Ok::<_, anyhow::Error>((artifact_id, bytes))
            }))
            .await?;
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
        let bytes = tokio::task::spawn_blocking(move || {
            resolve_locked(
                &handle,
                &client,
                &startup_progress,
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
        tokio::task::spawn_blocking(move || {
            extract_metadata_locked(&handle, &cache_dir, &cache_key, &wasm_bytes)
        })
        .await
        .context("built-in artifact metadata extraction task panicked")?
    }
}

fn extract_metadata_locked(
    handle: &Handle,
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
    let metadata = handle.block_on(extract_component_metadata_from_bytes(
        wasm_bytes, true, true,
    ))?;
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
        .block_on(client.get(url.clone()).send())
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
        .block_on(response.chunk())
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
        let mut resolver =
            BuiltinArtifactResolver::new(&config, StartupProgress::default()).unwrap();
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
        let resolver = BuiltinArtifactResolver::new(&config, StartupProgress::default()).unwrap();
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
        let pinned = BuiltinArtifactResolver::new(&config, StartupProgress::default()).unwrap();
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
        let resolver = BuiltinArtifactResolver::new(&config, StartupProgress::default()).unwrap();
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
        let offline_resolver =
            BuiltinArtifactResolver::new(&offline_config, StartupProgress::default()).unwrap();
        assert_eq!(
            offline_resolver.resolve("test").await.unwrap().as_slice(),
            payload
        );
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
        )
        .unwrap();

        assert_eq!(resolver.resolve("test").await.unwrap().as_slice(), payload);
        assert_eq!(std::fs::read(cache_path).unwrap(), payload);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
