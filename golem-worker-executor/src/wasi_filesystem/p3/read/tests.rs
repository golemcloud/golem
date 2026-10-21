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

use super::*;
use crate::services::golem_config::FilesystemStorageMode;
use std::future::poll_fn;
use std::time::Duration;
use test_r::test;

struct ReadProducerFixture {
    _root: tempfile::TempDir,
    filesystem: agent_filesystem::ResidentFilesystem,
    window: crate::services::resource_usage_metering::ResourceUsageMeteringWindow,
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
}

impl ReadProducerFixture {
    async fn new(contents: Bytes) -> Self {
        use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
        use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
        use crate::services::agent_filesystem::{
            ResolvedStorageLimits, bind_resource_usage_metering, create_fresh,
            finish_reconstruction, finish_replay, materialize_baseline, no_initial_files,
            open_resource_usage_window, reconstruction_generation_handle,
            resident_generation_handle, scratch_directory,
        };
        use crate::services::golem_config::FilesystemStorageConfig;
        use crate::services::linear_memory::LinearMemoryTracker;
        use crate::services::resource_limits::AtomicResourceEntry;
        use crate::services::resource_usage_metering::ResourceUsageAccount;
        use golem_common::model::account::AccountId;
        use golem_common::model::agent::AgentMode;
        use golem_common::model::component::ComponentId;
        use golem_common::model::environment::EnvironmentId;
        use golem_common::model::{AgentId, OwnedAgentId};
        use std::sync::Arc;
        use std::time::Instant;
        use uuid::Uuid;

        let root = tempfile::tempdir().unwrap();
        let profile = FilesystemStorageConfig {
            mode: FilesystemStorageMode::Directory {
                root: root.path().into(),
            },
            ..FilesystemStorageConfig::default()
        };
        let agent = OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId::from_agent_name_string(ComponentId::new(), "p3-pull-read").unwrap(),
        );
        let provisioning =
            SandboxFilesystemProvisioning::new(&profile.mode, profile.cleanup_retry.clone())
                .unwrap();
        let created = create_fresh(
            provisioning,
            scratch_directory().await,
            agent.clone(),
            ResolvedStorageLimits::Unlimited,
        )
        .await
        .unwrap();
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
        let memory = LinearMemoryTracker::new(
            0,
            0,
            AgentMode::Durable,
            false,
            entry.clone(),
            Arc::new(std::sync::Mutex::new(MemoryGrant::inert(0))),
            Instant::now(),
        );
        let reconstructing = bind_resource_usage_metering(
            created,
            ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone()),
        )
        .unwrap();
        let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
        let account_id = AccountId(Uuid::new_v4());
        scheduler.register_account(account_id, entry).await;
        let permit = scheduler.acquire(account_id, agent.agent_id).await;
        let window = open_resource_usage_window(&reconstructing, permit)
            .await
            .unwrap();
        let reconstructing = materialize_baseline(
            reconstructing,
            no_initial_files().await,
            None::<std::convert::Infallible>,
        )
        .await
        .unwrap();
        let generation_handle = reconstruction_generation_handle(&reconstructing).unwrap();
        let opened = route_open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "large-file").unwrap(),
            types::PathFlags::SYMLINK_FOLLOW,
            types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE,
            types::DescriptorFlags::READ | types::DescriptorFlags::WRITE,
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap();
        let OpenNode::File(file) = &opened.node else {
            panic!("sandbox file open returned a directory")
        };
        let (written, _) = route_stream_chunk(
            &generation_handle,
            file,
            WritePlacement::At(0),
            contents.clone(),
        )
        .unwrap()
        .await
        .unwrap()
        .unwrap();
        assert_eq!(written, contents.len());
        let descriptor = AgentDescriptor::new(opened.node, PathBuf::from("large-file"));
        let reconstructing = finish_replay(reconstructing).await.unwrap();
        let filesystem = finish_reconstruction(reconstructing).await.unwrap();
        let generation_handle = resident_generation_handle(&filesystem);

        Self {
            _root: root,
            filesystem,
            window,
            generation_handle,
            descriptor,
        }
    }

    async fn close(self) {
        use crate::services::agent_filesystem::{delete, seal};
        use crate::services::resource_usage_metering::close_window;
        use std::time::Instant;

        drop(self.descriptor);
        close_window(self.window, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        delete(seal(self.filesystem)).await.unwrap();
    }
}

#[test]
async fn p3_read_stream_prefetch_is_bounded_and_exhausted_before_the_next_read() {
    let contents = Bytes::from(
        (0..FILESYSTEM_READ_CHUNK_SIZE * 32)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let fixture = ReadProducerFixture::new(contents.clone()).await;
    let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
    let mut producer = FilesystemReadProducer::new(
        fixture.generation_handle.clone(),
        fixture.descriptor.clone(),
        0,
        result_tx,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap();

    assert_eq!(producer.read_call_count(), 1);
    assert!(producer.pending.is_none());
    assert_eq!(producer.buffered.len(), FILESYSTEM_READ_CHUNK_SIZE);
    assert_eq!(producer.offset, FILESYSTEM_READ_CHUNK_SIZE as u64);
    assert_eq!(producer.take_buffered(4096), contents.slice(..4096));
    assert_eq!(producer.read_call_count(), 1);
    assert_eq!(
        producer.take_buffered(usize::MAX),
        contents.slice(4096..FILESYSTEM_READ_CHUNK_SIZE)
    );
    assert_eq!(producer.read_call_count(), 1);

    let chunk = poll_fn(|cx| producer.poll_read_chunk(cx, FILESYSTEM_READ_CHUNK_SIZE))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        chunk,
        contents.slice(FILESYSTEM_READ_CHUNK_SIZE..FILESYSTEM_READ_CHUNK_SIZE * 2)
    );
    assert_eq!(producer.read_call_count(), 2);
    assert_eq!(producer.offset, (FILESYSTEM_READ_CHUNK_SIZE * 2) as u64);
    assert!(producer.pending.is_none());
    assert!(matches!(
        result_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    drop(producer);
    result_rx.await.unwrap().unwrap().unwrap();
    fixture.close().await;
}

#[test]
async fn dropping_p3_read_stream_before_eof_stops_further_reads() {
    let contents = Bytes::from(vec![0x5a; FILESYSTEM_READ_CHUNK_SIZE * 8]);
    let fixture = ReadProducerFixture::new(contents.clone()).await;
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let producer = FilesystemReadProducer::new(
        fixture.generation_handle.clone(),
        fixture.descriptor.clone(),
        17,
        result_tx,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap();

    assert_eq!(producer.read_call_count(), 1);
    assert_eq!(
        producer.buffered,
        contents.slice(17..17 + FILESYSTEM_READ_CHUNK_SIZE)
    );
    assert_eq!(producer.offset, 17 + FILESYSTEM_READ_CHUNK_SIZE as u64);
    let read_calls = producer.read_calls.clone();

    drop(producer);
    result_rx.await.unwrap().unwrap().unwrap();
    assert_eq!(read_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    fixture.close().await;
}

#[test]
async fn p3_read_stream_preserves_offset_and_detects_eof_after_prefetch() {
    let contents = Bytes::from(
        (0..FILESYSTEM_READ_CHUNK_SIZE + 123)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let fixture = ReadProducerFixture::new(contents.clone()).await;
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let mut producer = FilesystemReadProducer::new(
        fixture.generation_handle.clone(),
        fixture.descriptor.clone(),
        17,
        result_tx,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap();

    assert_eq!(
        producer.take_buffered(usize::MAX),
        contents.slice(17..17 + FILESYSTEM_READ_CHUNK_SIZE)
    );
    assert_eq!(producer.read_call_count(), 1);
    let chunk = poll_fn(|cx| producer.poll_read_chunk(cx, FILESYSTEM_READ_CHUNK_SIZE))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(chunk, contents.slice(17 + FILESYSTEM_READ_CHUNK_SIZE..));
    assert_eq!(producer.offset, contents.len() as u64);
    assert_eq!(producer.read_call_count(), 2);
    assert!(
        poll_fn(|cx| producer.poll_read_chunk(cx, FILESYSTEM_READ_CHUNK_SIZE))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(producer.offset, contents.len() as u64);
    assert_eq!(producer.read_call_count(), 3);

    drop(producer);
    result_rx.await.unwrap().unwrap().unwrap();
    fixture.close().await;
}
