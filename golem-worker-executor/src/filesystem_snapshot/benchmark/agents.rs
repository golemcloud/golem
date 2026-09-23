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

//! The repositories of the agents of a scenario, a CPU setting and a tree.
//!
//! Each agent has its own repository, as in production. The repository of an agent is below the
//! path `agents/<agent>` of the namespace of its scenario, CPU setting and tree. Thus a copy of
//! the repository of one agent into another agent is a copy of blobs in one namespace, which the
//! S3 backend does in the bucket with `CopyObject`.

use super::TARGET_LABEL;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The first name of the path of the repository of each agent.
pub(super) const AGENTS: &str = "agents";

/// The agent whose repository the phases copy to the other agents.
pub(super) const FIRST_AGENT: &str = "0";

/// The number of blob copies that are in progress at the same time.
const COPY_CONCURRENCY: usize = 64;

/// The name of the file that a repository writes when it is made.
const CONFIG: &str = "config";

/// Gives the path of the repository of the agent in its namespace.
fn agent_root(agent: &str) -> PathBuf {
    Path::new(AGENTS).join(agent)
}

/// A blob storage that keeps the repository of one agent below its path in the namespace.
///
/// Each call goes to `inner`, with the path below the path of the agent. Each path that `inner`
/// gives is relative to the path of the agent again.
#[derive(Debug)]
pub(super) struct AgentStorage {
    inner: Arc<dyn BlobStorage>,
    root: Box<Path>,
}

impl AgentStorage {
    pub(super) fn new(inner: Arc<dyn BlobStorage>, agent: &str) -> Self {
        Self {
            inner,
            root: agent_root(agent).into_boxed_path(),
        }
    }

    fn path(&self, path: &Path) -> PathBuf {
        self.root.join(path)
    }

    /// Gives the path that `inner` gave, relative to the path of the agent.
    fn relative(&self, path: &Path) -> anyhow::Result<PathBuf> {
        path.strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                anyhow::anyhow!(
                    "the storage gave the path {}, which is not below {}",
                    path.display(),
                    self.root.display()
                )
            })
    }
}

#[async_trait]
impl BlobStorage for AgentStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw(target_label, op_label, namespace, &self.path(path))
            .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.inner
            .get_stream(target_label, op_label, namespace, &self.path(path))
            .await
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw_slice(
                target_label,
                op_label,
                namespace,
                &self.path(path),
                start,
                end,
            )
            .await
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner
            .get_metadata(target_label, op_label, namespace, &self.path(path))
            .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.inner
            .put_raw(target_label, op_label, namespace, &self.path(path), data)
            .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent(target_label, op_label, namespace, &self.path(path), data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.inner
            .put_stream(target_label, op_label, namespace, &self.path(path), stream)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.inner
            .delete(target_label, op_label, namespace, &self.path(path))
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir(target_label, op_label, namespace, &self.path(path))
            .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir(target_label, op_label, namespace, &self.path(path))
            .await?
            .iter()
            .map(|path| self.relative(path))
            .collect()
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below(target_label, op_label, namespace, &self.path(path))
            .await?
            .iter()
            .map(|blob| {
                self.relative(&blob.path).map(|path| ListedBlob {
                    path: path.into_boxed_path(),
                    size: blob.size,
                })
            })
            .collect()
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.inner
            .delete_dir(target_label, op_label, namespace, &self.path(path))
            .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists(target_label, op_label, namespace, &self.path(path))
            .await
    }
}

/// Copies the repository of [`FIRST_AGENT`] to each agent from `1` to `agents - 1` that has no
/// repository, and gives the numbers of the copied repositories and blobs.
///
/// The config of a repository is its last copied blob, so an agent with a config has each blob of
/// the repository, also after a copy that stopped. The copies of the other blobs of all agents
/// are in progress at the same time, up to [`COPY_CONCURRENCY`].
pub(super) async fn copy_first_agent(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    agents: usize,
) -> anyhow::Result<Value> {
    let source = &agent_root(FIRST_AGENT);
    let blobs = storage
        .list_blobs_below(TARGET_LABEL, "list_agent", namespace.clone(), source)
        .await?
        .iter()
        .map(|blob| blob.path.strip_prefix(source).map(Path::to_path_buf))
        .collect::<Result<Box<[_]>, _>>()?;
    anyhow::ensure!(
        blobs.iter().any(|path| path == Path::new(CONFIG)),
        "the repository of the agent {FIRST_AGENT} has no {CONFIG}"
    );
    let missing = futures::stream::iter(1..agents)
        .map(|agent| async move {
            let config = agent_root(&agent.to_string()).join(CONFIG);
            storage
                .get_metadata(TARGET_LABEL, "find_agent", namespace.clone(), &config)
                .await
                .map(|metadata| metadata.is_none().then_some(agent))
        })
        .buffered(COPY_CONCURRENCY)
        .try_filter_map(|agent| async move { Ok(agent) })
        .try_collect::<Vec<_>>()
        .await?;
    let (configs, others): (Vec<_>, Vec<_>) = missing
        .iter()
        .flat_map(|agent| blobs.iter().map(move |blob| (*agent, blob)))
        .partition(|(_, blob)| *blob == Path::new(CONFIG));
    futures::stream::iter(others.iter().copied())
        .map(|(agent, blob)| copy_blob(storage, namespace, source, agent, blob))
        .buffer_unordered(COPY_CONCURRENCY)
        .try_collect::<()>()
        .await?;
    futures::stream::iter(configs.iter().copied())
        .map(|(agent, blob)| copy_blob(storage, namespace, source, agent, blob))
        .buffer_unordered(COPY_CONCURRENCY)
        .try_collect::<()>()
        .await?;
    Ok(json!({
        "agents_copied": missing.len(),
        "blobs_copied": configs.len() + others.len(),
    }))
}

/// Copies the blob of the repository at `source` to the repository of the agent.
async fn copy_blob(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    source: &Path,
    agent: usize,
    blob: &Path,
) -> anyhow::Result<()> {
    storage
        .copy(
            TARGET_LABEL,
            "copy_agent",
            namespace.clone(),
            &source.join(blob),
            &agent_root(&agent.to_string()).join(blob),
        )
        .await
}

/// The directories of a repository in the order in which a copy of the repository lists and
/// copies them: the reverse of the order in which a save writes them. A snapshot that the copy has
/// thus has its index and its packs too. The config goes last.
const COPY_ORDER: [&str; 3] = ["snapshots", "index", "data"];

/// Gives each blob of the repository of the agent, with the path relative to the repository, in
/// the order of the paths.
pub(super) async fn agent_blobs(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    agent: &str,
) -> anyhow::Result<Box<[ListedBlob]>> {
    let root = agent_root(agent);
    let mut blobs = storage
        .list_blobs_below(TARGET_LABEL, "list_agent", namespace.clone(), &root)
        .await?
        .iter()
        .map(|blob| {
            Ok(ListedBlob {
                path: blob.path.strip_prefix(&root)?.into(),
                size: blob.size,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    blobs.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(blobs.into_boxed_slice())
}

/// Gives the sum of the sizes of the blobs.
pub(super) fn total_bytes(blobs: &[ListedBlob]) -> u64 {
    blobs.iter().map(|blob| blob.size).sum()
}

/// Copies the repository of the agent `from` to the agent `to` on the server, as the copy of a
/// scope does it, and gives the numbers of the copied blobs and bytes.
///
/// The copy lists the snapshot files, the index files and the packs, in this order, and then
/// copies them in the same order, each group after the one before it. The config goes last, so an
/// agent with a config has each blob of the copy.
pub(super) async fn copy_agent(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    from: &str,
    to: &str,
) -> anyhow::Result<Value> {
    let source = &agent_root(from);
    let groups = futures::stream::iter(COPY_ORDER)
        .then(|directory| async move {
            storage
                .list_blobs_below(
                    TARGET_LABEL,
                    "list_agent",
                    namespace.clone(),
                    &source.join(directory),
                )
                .await
        })
        .try_collect::<Vec<_>>()
        .await?;
    let config = storage
        .get_metadata(
            TARGET_LABEL,
            "find_agent",
            namespace.clone(),
            &source.join(CONFIG),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("the repository of the agent {from} has no {CONFIG}"))?;
    let groups = groups
        .into_iter()
        .chain(std::iter::once(Box::from([ListedBlob {
            path: source.join(CONFIG).into_boxed_path(),
            size: config.size,
        }])))
        .collect::<Box<[_]>>();
    let target = &agent_root(to);
    futures::stream::iter(groups.iter())
        .map(Ok)
        .try_for_each(|group| async move {
            futures::stream::iter(group.iter())
                .map(|blob| async move {
                    let relative = blob.path.strip_prefix(source)?;
                    storage
                        .copy(
                            TARGET_LABEL,
                            "copy_agent",
                            namespace.clone(),
                            &blob.path,
                            &target.join(relative),
                        )
                        .await
                })
                .buffer_unordered(COPY_CONCURRENCY)
                .try_collect::<()>()
                .await
        })
        .await?;
    Ok(json!({
        "blobs_copied": groups.iter().map(|group| group.len()).sum::<usize>(),
        "bytes_copied": groups.iter().map(|group| total_bytes(group)).sum::<u64>(),
    }))
}

/// Deletes the repository of the agent, and tells whether it had one.
pub(super) async fn delete_agent(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    agent: &str,
) -> anyhow::Result<bool> {
    storage
        .delete_dir(
            TARGET_LABEL,
            "delete_agent",
            namespace.clone(),
            &agent_root(agent),
        )
        .await
}
