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

use crate::config::{BuiltinArtifactSource, BuiltinArtifactsConfig};
use anyhow::{Context, anyhow};
use fd_lock::RwLock;
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

#[derive(Clone)]
pub struct BuiltinArtifactResolver {
    cache_dir: PathBuf,
    sources: BTreeMap<String, BuiltinArtifactSource>,
    client: reqwest::Client,
    in_process_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl BuiltinArtifactResolver {
    pub fn new(config: &BuiltinArtifactsConfig) -> anyhow::Result<Self> {
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
        })
    }

    pub async fn resolve_many(
        &self,
        artifact_ids: impl IntoIterator<Item = &'static str>,
    ) -> anyhow::Result<BTreeMap<&'static str, Arc<Vec<u8>>>> {
        let artifact_ids = artifact_ids.into_iter().collect::<BTreeSet<_>>();
        let artifacts =
            futures::future::try_join_all(artifact_ids.into_iter().map(|artifact_id| async move {
                Ok::<_, anyhow::Error>((artifact_id, self.resolve(artifact_id).await?))
            }))
            .await?;
        Ok(artifacts.into_iter().collect())
    }

    pub async fn resolve(&self, artifact_id: &str) -> anyhow::Result<Arc<Vec<u8>>> {
        let source =
            self.sources.get(artifact_id).cloned().ok_or_else(|| {
                anyhow!("no built-in artifact source configured for '{artifact_id}'")
            })?;
        let url = Url::parse(&source.url).with_context(|| {
            format!("invalid URL configured for built-in artifact '{artifact_id}'")
        })?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "built-in artifact '{artifact_id}' must use an HTTP or HTTPS URL"
        );

        let expected_sha256 = parse_sha256(artifact_id, source.sha256.as_deref())?;
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
        let bytes = tokio::task::spawn_blocking(move || {
            resolve_locked(
                &handle,
                &client,
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
        let bytes = read_and_verify(&cache_path, artifact_id, expected_sha256.as_ref())?;
        tracing::info!(
            artifact_id,
            cache_path = %cache_path.display(),
            "Using cached built-in artifact"
        );
        return Ok(bytes);
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
        let resolver = BuiltinArtifactResolver::new(&config).unwrap();
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
        let offline_resolver = BuiltinArtifactResolver::new(&offline_config).unwrap();
        assert_eq!(
            offline_resolver.resolve("test").await.unwrap().as_slice(),
            payload
        );
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
        let resolver = BuiltinArtifactResolver::new(&BuiltinArtifactsConfig {
            cache_dir: Some(cache.path().to_path_buf()),
            source_overrides: BTreeMap::from([(
                "test".to_string(),
                BuiltinArtifactSource {
                    url: format!("http://127.0.0.1:{port}/artifact.wasm"),
                    sha256: Some(expected.clone()),
                },
            )]),
        })
        .unwrap();

        let error = resolver.resolve("test").await.unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"), "{error:#}");
        assert!(!cache.path().join(format!("{expected}.wasm")).exists());
        server.abort();
    }
}
