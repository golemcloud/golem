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
use crate::services::agent_filesystem::{AccessMode, FileDisposition, Follow, OpenOptions};
use crate::services::golem_config::FilesystemStorageMode;
use crate::wasi_filesystem::p2::types::{
    p2_agent_error, p2_agent_open_error, p2_agent_open_request, p2_agent_stat,
    p2_agent_write_result, p2_link_access_error, p2_time_changes, p2_visible_descriptor_flags,
};
use crate::wasi_filesystem::{
    AgentOpenDecision, AgentOpenPolicyError, decide_agent_existing_open, decide_agent_open,
    flush_level, replay_time_changes, resize_attribute_changes,
};
use fs_set_times::{SystemTimeSpec, set_symlink_times, set_times};
use golem_common::model::oplog::types::SerializableDateTime;
use std::time::{Duration, SystemTime};
use test_r::test;

fn p3_datetime_parts(time: SystemTime) -> (i64, u32) {
    let datetime = p3_agent_datetime(time).unwrap();
    (datetime.seconds, datetime.nanoseconds)
}

fn p3_timestamp(seconds: i64, nanoseconds: u32) -> types::NewTimestamp {
    types::NewTimestamp::Timestamp(wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
        seconds,
        nanoseconds,
    })
}

fn p3_decoded_time(seconds: i64, nanoseconds: u32) -> SystemTime {
    p3_native_time(p3_timestamp(seconds, nanoseconds))
        .unwrap()
        .unwrap()
}

fn open_request() -> AgentOpenRequest {
    AgentOpenRequest {
        create: false,
        directory: false,
        exclusive: false,
        truncate: false,
        follow: false,
        read: false,
        write: false,
        mutate_directory: false,
        unsupported_sync: false,
    }
}

fn decided_open(request: AgentOpenRequest) -> OpenOptions {
    match decide_agent_open(request).unwrap() {
        AgentOpenDecision::Open(options) => options,
        AgentOpenDecision::ObserveAttributes => {
            panic!("open policy unexpectedly requested attributes")
        }
    }
}

#[test]
fn agent_open_policy_rejects_unsupported_sync() {
    assert_eq!(
        decide_agent_open(AgentOpenRequest {
            unsupported_sync: true,
            ..open_request()
        }),
        Err(AgentOpenPolicyError::Unsupported)
    );
}

#[test]
fn agent_open_policy_rejects_each_directory_mutation_flag() {
    for request in [
        AgentOpenRequest {
            directory: true,
            create: true,
            ..open_request()
        },
        AgentOpenRequest {
            directory: true,
            exclusive: true,
            ..open_request()
        },
        AgentOpenRequest {
            directory: true,
            truncate: true,
            ..open_request()
        },
    ] {
        assert_eq!(
            decide_agent_open(request),
            Err(AgentOpenPolicyError::Invalid)
        );
    }
}

#[test]
fn agent_open_policy_selects_access_modes() {
    for (read, mutate_directory, expected) in [
        (false, false, AccessMode::None),
        (true, false, AccessMode::Read),
        (false, true, AccessMode::Write),
        (true, true, AccessMode::ReadWrite),
    ] {
        assert_eq!(
            decided_open(AgentOpenRequest {
                directory: true,
                read,
                write: true,
                mutate_directory,
                ..open_request()
            }),
            OpenOptions::Existing {
                expected: ObjectKind::Directory,
                access: expected,
                follow: Follow::No,
            }
        );
    }
}

#[test]
fn agent_open_policy_selects_follow_mode() {
    assert_eq!(
        decided_open(AgentOpenRequest {
            directory: true,
            follow: false,
            read: true,
            ..open_request()
        }),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::Read,
            follow: Follow::No,
        }
    );
    assert_eq!(
        decided_open(AgentOpenRequest {
            directory: true,
            follow: true,
            read: true,
            ..open_request()
        }),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::Read,
            follow: Follow::Yes,
        }
    );
}

#[test]
fn agent_open_policy_selects_each_file_disposition() {
    for (request, disposition) in [
        (
            AgentOpenRequest {
                create: true,
                ..open_request()
            },
            FileDisposition::CreateIfMissing,
        ),
        (
            AgentOpenRequest {
                create: true,
                exclusive: true,
                ..open_request()
            },
            FileDisposition::CreateExclusive,
        ),
        (
            AgentOpenRequest {
                truncate: true,
                ..open_request()
            },
            FileDisposition::TruncateExisting,
        ),
        (
            AgentOpenRequest {
                create: true,
                truncate: true,
                ..open_request()
            },
            FileDisposition::CreateOrTruncate,
        ),
    ] {
        assert_eq!(
            decided_open(request),
            OpenOptions::File {
                access: AccessMode::None,
                disposition,
                follow: Follow::No,
            }
        );
    }
}

#[test]
fn agent_open_policy_uses_observed_existing_kind_and_rejects_symlinks() {
    assert_eq!(
        decide_agent_open(AgentOpenRequest {
            read: true,
            follow: true,
            ..open_request()
        }),
        Ok(AgentOpenDecision::ObserveAttributes)
    );
    assert_eq!(
        decide_agent_existing_open(
            AgentOpenRequest {
                read: true,
                follow: true,
                ..open_request()
            },
            ObjectKind::File
        ),
        Ok(OpenOptions::Existing {
            expected: ObjectKind::File,
            access: AccessMode::Read,
            follow: Follow::Yes,
        })
    );
    assert_eq!(
        decide_agent_existing_open(
            AgentOpenRequest {
                read: true,
                ..open_request()
            },
            ObjectKind::Directory
        ),
        Ok(OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::Read,
            follow: Follow::No,
        })
    );
    assert_eq!(
        decide_agent_existing_open(open_request(), ObjectKind::Symlink),
        Err(AgentOpenPolicyError::SymlinkLoop)
    );
}

#[test]
fn observed_open_capabilities_depend_on_kind_not_irrelevant_libc_flags() {
    for read in [false, true] {
        for write in [false, true] {
            for mutate_directory in [false, true] {
                let request = AgentOpenRequest {
                    read,
                    write,
                    mutate_directory,
                    ..open_request()
                };
                assert_eq!(
                    decide_agent_open(request),
                    Ok(AgentOpenDecision::ObserveAttributes)
                );
                for kind in [ObjectKind::Directory, ObjectKind::File] {
                    let mutable = if kind == ObjectKind::Directory {
                        mutate_directory
                    } else {
                        write
                    };
                    let access = match (read, mutable) {
                        (false, false) => AccessMode::None,
                        (true, false) => AccessMode::Read,
                        (false, true) => AccessMode::Write,
                        (true, true) => AccessMode::ReadWrite,
                    };
                    assert_eq!(
                        decide_agent_existing_open(request, kind),
                        Ok(OpenOptions::Existing {
                            expected: kind,
                            access,
                            follow: Follow::No,
                        })
                    );
                }
            }
        }
    }
}

#[test]
fn p3_agent_datetime_normalizes_exact_epoch() {
    assert_eq!(p3_datetime_parts(SystemTime::UNIX_EPOCH), (0, 0));
}

#[test]
fn p3_agent_datetime_normalizes_positive_fractional_time() {
    assert_eq!(
        p3_datetime_parts(SystemTime::UNIX_EPOCH + Duration::from_millis(500)),
        (0, 500_000_000)
    );
}

#[test]
fn p3_agent_datetime_normalizes_negative_whole_seconds() {
    assert_eq!(
        p3_datetime_parts(SystemTime::UNIX_EPOCH - Duration::from_secs(2)),
        (-2, 0)
    );
}

#[test]
fn p3_agent_datetime_normalizes_negative_fractional_time() {
    assert_eq!(
        p3_datetime_parts(SystemTime::UNIX_EPOCH - Duration::from_millis(500)),
        (-1, 500_000_000)
    );
}

#[test]
fn p3_native_time_decodes_normalized_signed_fractions() {
    assert_eq!(p3_decoded_time(0, 0), SystemTime::UNIX_EPOCH);
    assert_eq!(
        p3_decoded_time(0, 500_000_000),
        SystemTime::UNIX_EPOCH + Duration::from_millis(500)
    );
    assert_eq!(
        p3_decoded_time(-2, 0),
        SystemTime::UNIX_EPOCH - Duration::from_secs(2)
    );
    assert_eq!(
        p3_decoded_time(-1, 500_000_000),
        SystemTime::UNIX_EPOCH - Duration::from_millis(500)
    );
    assert_eq!(
        p3_decoded_time(-2, 500_000_000),
        SystemTime::UNIX_EPOCH - Duration::from_millis(1_500)
    );
}

#[test]
fn p3_timestamp_encoding_and_decoding_round_trip() {
    for time in [
        SystemTime::UNIX_EPOCH,
        SystemTime::UNIX_EPOCH + Duration::from_millis(500),
        SystemTime::UNIX_EPOCH + Duration::new(12, 34),
        SystemTime::UNIX_EPOCH - Duration::from_secs(2),
        SystemTime::UNIX_EPOCH - Duration::from_millis(500),
        SystemTime::UNIX_EPOCH - Duration::from_millis(1_500),
    ] {
        let (seconds, nanoseconds) = p3_datetime_parts(time);
        assert_eq!(p3_decoded_time(seconds, nanoseconds), time);
    }
}

#[test]
fn p3_native_time_handles_signed_boundaries_and_rejects_invalid_fractions() {
    for (seconds, nanoseconds) in [
        (i64::MIN, 0),
        (i64::MIN, 500_000_000),
        (i64::MAX, 0),
        (i64::MAX, 999_999_999),
    ] {
        match p3_native_time(p3_timestamp(seconds, nanoseconds)) {
            Ok(Some(time)) => assert_eq!(p3_datetime_parts(time), (seconds, nanoseconds)),
            Err(error) => assert!(matches!(
                error.downcast().unwrap(),
                types::ErrorCode::Overflow
            )),
            Ok(None) => panic!("explicit timestamp decoded as no change"),
        }
    }

    let error = p3_native_time(p3_timestamp(0, 1_000_000_000)).unwrap_err();
    assert!(matches!(
        error.downcast().unwrap(),
        types::ErrorCode::Overflow
    ));
}

#[test]
fn p3_time_changes_decode_negative_accessed_and_modified_fractions_identically() {
    assert_eq!(
        p3_time_changes(p3_timestamp(-1, 500_000_000), p3_timestamp(-2, 500_000_000),).unwrap(),
        TimeChanges {
            accessed: TimeChange::Set(SystemTime::UNIX_EPOCH - Duration::from_millis(500)),
            modified: TimeChange::Set(SystemTime::UNIX_EPOCH - Duration::from_millis(1_500),),
        }
    );
}

#[test]
fn p2_agent_stat_keeps_pre_epoch_timestamps_as_overflow() {
    let error = p2_agent_stat(AgentAttributes {
        kind: ObjectKind::File,
        link_count: 1,
        size: 0,
        accessed: Some(SystemTime::UNIX_EPOCH - Duration::from_millis(1)),
        modified: None,
        read_only: false,
    })
    .unwrap_err();

    assert_eq!(
        error.downcast().unwrap(),
        wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Overflow
    );
}

#[test]
fn p2_p3_agent_open_request_translation_is_identical() {
    use wasmtime_wasi::p2::bindings::filesystem::types::{
        DescriptorFlags as P2DescriptorFlags, OpenFlags as P2OpenFlags, PathFlags as P2PathFlags,
    };

    let p2 = p2_agent_open_request(
        P2PathFlags::SYMLINK_FOLLOW,
        P2OpenFlags::CREATE | P2OpenFlags::TRUNCATE,
        P2DescriptorFlags::READ
            | P2DescriptorFlags::WRITE
            | P2DescriptorFlags::MUTATE_DIRECTORY
            | P2DescriptorFlags::DATA_INTEGRITY_SYNC,
    );
    let p3 = p3_agent_open_request(
        types::PathFlags::SYMLINK_FOLLOW,
        types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE,
        types::DescriptorFlags::READ
            | types::DescriptorFlags::WRITE
            | types::DescriptorFlags::MUTATE_DIRECTORY
            | types::DescriptorFlags::DATA_INTEGRITY_SYNC,
    );

    assert_eq!(p2, p3);
    assert!(p2.mutate_directory);
    assert_eq!(decide_agent_open(p2), decide_agent_open(p3));
}

#[test]
fn p2_p3_agent_stat_result_mapping_is_identical() {
    let accessed = SystemTime::UNIX_EPOCH + Duration::new(12, 34);
    let modified = SystemTime::UNIX_EPOCH + Duration::new(56, 78);
    let attributes = AgentAttributes {
        kind: ObjectKind::File,
        link_count: 3,
        size: 7,
        accessed: Some(accessed),
        modified: Some(modified),
        read_only: false,
    };

    let p2 = p2_agent_stat(attributes.clone()).unwrap();
    let p3 = p3_agent_stat(attributes).unwrap();

    assert_eq!(
        p2.type_,
        wasmtime_wasi::p2::bindings::filesystem::types::DescriptorType::RegularFile
    );
    assert!(matches!(p3.type_, types::DescriptorType::RegularFile));
    assert_eq!(p2.link_count, p3.link_count);
    assert_eq!(p2.size, p3.size);
    assert_eq!(
        i64::try_from(p2.data_access_timestamp.unwrap().seconds).unwrap(),
        p3.data_access_timestamp.unwrap().seconds
    );
    assert_eq!(
        i64::try_from(p2.data_modification_timestamp.unwrap().seconds).unwrap(),
        p3.data_modification_timestamp.unwrap().seconds
    );
}

#[test]
fn p2_p3_agent_open_error_mapping_is_identical() {
    use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

    assert_eq!(
        p2_agent_open_error(AgentOpenRouteError::Invalid)
            .downcast()
            .unwrap(),
        P2ErrorCode::Invalid
    );
    assert!(matches!(
        p3_agent_open_error(AgentOpenRouteError::Invalid)
            .downcast()
            .unwrap(),
        types::ErrorCode::Invalid
    ));
    assert_eq!(
        p2_agent_open_error(AgentOpenRouteError::Unsupported)
            .downcast()
            .unwrap(),
        P2ErrorCode::Unsupported
    );
    assert!(matches!(
        p3_agent_open_error(AgentOpenRouteError::Unsupported)
            .downcast()
            .unwrap(),
        types::ErrorCode::Unsupported
    ));
    assert_eq!(
        p2_agent_open_error(AgentOpenRouteError::SymlinkLoop)
            .downcast()
            .unwrap(),
        P2ErrorCode::Loop
    );
    assert!(matches!(
        p3_agent_open_error(AgentOpenRouteError::SymlinkLoop)
            .downcast()
            .unwrap(),
        types::ErrorCode::Loop
    ));
}

#[test]
fn p2_p3_attribute_and_flush_translation_is_identical() {
    use wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime as P2Datetime;
    use wasmtime_wasi::p2::bindings::filesystem::types::NewTimestamp as P2NewTimestamp;

    let p2 = p2_time_changes(
        P2NewTimestamp::Now,
        P2NewTimestamp::Timestamp(P2Datetime {
            seconds: 12,
            nanoseconds: 34,
        }),
    )
    .unwrap();
    let p3 = p3_time_changes(
        types::NewTimestamp::Now,
        types::NewTimestamp::Timestamp(
            wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                seconds: 12,
                nanoseconds: 34,
            },
        ),
    )
    .unwrap();

    assert_eq!(p2, p3);
    assert_eq!(
        resize_attribute_changes(99),
        AttributeChanges::File {
            size: 99,
            times: TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Keep,
            },
        }
    );
    assert_eq!(flush_level(true), agent_filesystem::FlushLevel::Data);
    assert_eq!(
        flush_level(false),
        agent_filesystem::FlushLevel::DataAndMetadata
    );
}

#[test]
fn replay_timestamp_restoration_uses_the_same_time_changes() {
    let accessed = SystemTime::UNIX_EPOCH + Duration::from_secs(50);
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(60);

    assert_eq!(
        replay_time_changes(Some(accessed), Some(modified)),
        TimeChanges {
            accessed: TimeChange::Set(accessed),
            modified: TimeChange::Set(modified),
        }
    );
    assert_eq!(
        replay_time_changes(None, Some(modified)),
        TimeChanges {
            accessed: TimeChange::Keep,
            modified: TimeChange::Set(modified),
        }
    );
}

#[test]
fn p2_p3_sandbox_escape_errors_are_access_denied() {
    use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

    let root = tempfile::tempdir().unwrap();
    let directory =
        cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
    let refusal = || {
        AgentFilesystemError::Sandbox(FilesystemStorageError::io(
            "open sandbox path",
            Some(root.path()),
            directory.open("../outside").unwrap_err(),
        ))
    };
    let p2: P2ErrorCode = p2_agent_error(refusal()).downcast().unwrap();
    let p3 = p3_agent_error(refusal()).downcast().unwrap();
    assert_eq!(p2, P2ErrorCode::Access);
    assert!(matches!(p3, types::ErrorCode::Access));
}

#[test]
fn p2_p3_read_only_attribute_error_mapping_is_identical() {
    use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

    let p2: P2ErrorCode = p2_agent_error(AgentFilesystemError::Access(
        agent_filesystem::AccessError::NotPermitted,
    ))
    .downcast()
    .unwrap();
    let p3 = p3_agent_error(AgentFilesystemError::Access(
        agent_filesystem::AccessError::NotPermitted,
    ))
    .downcast()
    .unwrap();

    assert_eq!(p2, P2ErrorCode::NotPermitted);
    assert!(matches!(p3, types::ErrorCode::NotPermitted));
}

#[test]
fn p2_p3_cross_project_link_mapping_is_identical() {
    use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

    let p2: P2ErrorCode = p2_link_access_error(agent_filesystem::AccessError::WrongGeneration)
        .downcast()
        .unwrap();
    let p3 = p3_link_access_error(agent_filesystem::AccessError::WrongGeneration)
        .downcast()
        .unwrap();
    assert_eq!(p2, P2ErrorCode::CrossDevice);
    assert!(matches!(p3, types::ErrorCode::CrossDevice));

    #[cfg(target_os = "linux")]
    {
        let storage_error = || {
            FilesystemStorageError::io(
                "hard link",
                None,
                std::io::Error::from_raw_os_error(libc::EXDEV),
            )
        };
        let p2: P2ErrorCode = p2_agent_error(AgentFilesystemError::Sandbox(storage_error()))
            .downcast()
            .unwrap();
        let p3 = p3_agent_error(AgentFilesystemError::Sandbox(storage_error()))
            .downcast()
            .unwrap();
        assert_eq!(p2, P2ErrorCode::CrossDevice);
        assert!(matches!(p3, types::ErrorCode::CrossDevice));
    }
}

#[test]
async fn staged_p2_p3_attribute_namespace_and_replay_routes_execute_through_agent_filesystem() {
    use crate::filesystem::sandbox::SandboxFilesystemProvisioning;
    use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
    use crate::services::agent_filesystem::{
        ResolvedStorageLimits, bind_resource_usage_metering, close, create_fresh, delete,
        finish_reconstruction, finish_replay, materialize_baseline, no_initial_files, open,
        open_resource_usage_window, reconstruction_generation_handle, resident_generation_handle,
        scratch_directory, seal,
    };
    use crate::services::golem_config::FilesystemStorageConfig;
    use crate::services::linear_memory::LinearMemoryTracker;
    use crate::services::resource_limits::AtomicResourceEntry;
    use crate::services::resource_usage_metering::{ResourceUsageAccount, close_window};
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
        &AgentId::from_agent_name_string(ComponentId::new(), "staged-attributes").unwrap(),
    );
    let provisioning =
        SandboxFilesystemProvisioning::new(&profile.mode, profile.cleanup_retry.clone()).unwrap();
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
        PathTarget::at_root(&generation_handle, "file").unwrap(),
        types::PathFlags::SYMLINK_FOLLOW,
        types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE,
        types::DescriptorFlags::READ | types::DescriptorFlags::WRITE,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap();
    let node = opened.node;
    let OpenNode::File(file) = &node else {
        panic!("sandbox file open returned a directory")
    };

    assert_eq!(
        crate::wasi_filesystem::p2::types::route_write(
            &generation_handle,
            file,
            0,
            Bytes::from_static(b"p2-replay"),
        )
        .unwrap()
        .await
        .unwrap(),
        9
    );
    assert_eq!(
        route_append_via_stream_chunk(&generation_handle, file, Bytes::from_static(b"-p3"))
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        (3, WritePlacement::Append)
    );

    crate::wasi_filesystem::p2::types::route_set_size(&generation_handle, &node, 12)
        .unwrap()
        .await
        .unwrap();
    route_set_size(&generation_handle, &node, 12)
        .unwrap()
        .await
        .unwrap();
    let p2_time = wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
        seconds: 100,
        nanoseconds: 0,
    };
    crate::wasi_filesystem::p2::types::route_set_times(
        &generation_handle,
        AgentTarget::Open(&node),
        wasmtime_wasi::p2::bindings::filesystem::types::NewTimestamp::NoChange,
        wasmtime_wasi::p2::bindings::filesystem::types::NewTimestamp::Timestamp(p2_time),
    )
    .unwrap()
    .await
    .unwrap();
    route_set_times(
        &generation_handle,
        AgentTarget::Open(&node),
        types::NewTimestamp::Timestamp(
            wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                seconds: 200,
                nanoseconds: 0,
            },
        ),
        types::NewTimestamp::NoChange,
    )
    .unwrap()
    .await
    .unwrap();
    crate::wasi_filesystem::p2::types::route_flush(&generation_handle, &node, true)
        .unwrap()
        .await
        .unwrap();
    route_flush(&generation_handle, &node, false)
        .unwrap()
        .await
        .unwrap();

    let root_node = open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, ".").unwrap(),
        OpenOptions::Existing {
            expected: ObjectKind::Directory,
            access: AccessMode::ReadWrite,
            follow: Follow::Yes,
        },
    )
    .unwrap()
    .await
    .unwrap()
    .node;
    let OpenNode::Directory(root_directory) = &root_node else {
        panic!("sandbox root open returned a file")
    };

    use wasmtime_wasi::p2::bindings::filesystem::types::{
        DescriptorFlags as P2Flags, ErrorCode as P2Error, OpenFlags as P2Open, PathFlags as P2Path,
    };
    for (flags, access) in [
        (types::DescriptorFlags::empty(), AccessMode::None),
        (types::DescriptorFlags::READ, AccessMode::Read),
        (types::DescriptorFlags::WRITE, AccessMode::None),
        (types::DescriptorFlags::MUTATE_DIRECTORY, AccessMode::Write),
        (
            types::DescriptorFlags::READ | types::DescriptorFlags::MUTATE_DIRECTORY,
            AccessMode::ReadWrite,
        ),
    ] {
        let child = route_open(
            &generation_handle,
            PathTarget::at(root_directory, "."),
            types::PathFlags::empty(),
            types::OpenFlags::empty(),
            flags,
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap()
        .node;
        assert_eq!(child.access(), access);
        let OpenNode::Directory(directory) = &child else {
            panic!("observed root is not a directory")
        };
        if !access.can_read() {
            assert_eq!(
                agent_filesystem::list_directory(&generation_handle, directory).unwrap_err(),
                agent_filesystem::AccessError::NotPermitted
            );
        }
        let p2 = crate::wasi_filesystem::p2::types::route_create_directory(
            &generation_handle,
            PathTarget::at(directory, "capability-p2"),
        )
        .await;
        let p3 = route_create_directory(
            &generation_handle,
            PathTarget::at(directory, "capability-p3"),
        )
        .await;
        if access.can_write() {
            p2.unwrap();
            p3.unwrap();
            crate::wasi_filesystem::p2::types::route_unlink(
                &generation_handle,
                PathTarget::at(directory, "capability-p2"),
                ObjectKind::Directory,
            )
            .await
            .unwrap();
            route_unlink(
                &generation_handle,
                PathTarget::at(directory, "capability-p3"),
                ObjectKind::Directory,
            )
            .await
            .unwrap();
        } else {
            assert_eq!(p2.unwrap_err().downcast().unwrap(), P2Error::ReadOnly);
            assert!(matches!(
                p3.unwrap_err().downcast().unwrap(),
                types::ErrorCode::ReadOnly
            ));
            for (path, p2_open, p3_open, p2_flags, p3_flags) in [
                (
                    ".",
                    P2Open::empty(),
                    types::OpenFlags::empty(),
                    P2Flags::WRITE,
                    types::DescriptorFlags::WRITE,
                ),
                (
                    "file",
                    P2Open::empty(),
                    types::OpenFlags::empty(),
                    P2Flags::READ | P2Flags::MUTATE_DIRECTORY,
                    types::DescriptorFlags::READ | types::DescriptorFlags::MUTATE_DIRECTORY,
                ),
                (
                    "file",
                    P2Open::empty(),
                    types::OpenFlags::empty(),
                    P2Flags::WRITE | P2Flags::MUTATE_DIRECTORY,
                    types::DescriptorFlags::WRITE | types::DescriptorFlags::MUTATE_DIRECTORY,
                ),
                (
                    ".",
                    P2Open::empty(),
                    types::OpenFlags::empty(),
                    P2Flags::MUTATE_DIRECTORY,
                    types::DescriptorFlags::MUTATE_DIRECTORY,
                ),
                (
                    "new",
                    P2Open::CREATE,
                    types::OpenFlags::CREATE,
                    P2Flags::empty(),
                    types::DescriptorFlags::empty(),
                ),
                (
                    "file",
                    P2Open::TRUNCATE,
                    types::OpenFlags::TRUNCATE,
                    P2Flags::READ,
                    types::DescriptorFlags::READ,
                ),
            ] {
                let p2 = crate::wasi_filesystem::p2::types::route_open(
                    &generation_handle,
                    PathTarget::at(directory, path),
                    P2Path::empty(),
                    p2_open,
                    p2_flags,
                    Box::pin(std::future::pending()),
                )
                .await
                .err()
                .unwrap();
                let p3 = route_open(
                    &generation_handle,
                    PathTarget::at(directory, path),
                    types::PathFlags::empty(),
                    p3_open,
                    p3_flags,
                    Box::pin(std::future::pending()),
                )
                .await
                .err()
                .unwrap();
                assert_eq!(p2.downcast().unwrap(), P2Error::ReadOnly);
                assert!(matches!(p3.downcast().unwrap(), types::ErrorCode::ReadOnly));
            }
        }
        let descriptor = AgentDescriptor::new(child, "/".into());
        let visible = agent_descriptor_flags(&descriptor).unwrap();
        assert_eq!(
            visible.contains(types::DescriptorFlags::READ),
            access.can_read()
        );
        assert_eq!(
            visible.contains(types::DescriptorFlags::MUTATE_DIRECTORY),
            access.can_write()
        );
        assert!(!visible.contains(types::DescriptorFlags::WRITE));
        drop(descriptor);
    }
    let combined = route_open(
        &generation_handle,
        PathTarget::at(root_directory, "file"),
        types::PathFlags::empty(),
        types::OpenFlags::empty(),
        types::DescriptorFlags::WRITE | types::DescriptorFlags::MUTATE_DIRECTORY,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap()
    .node;
    assert_eq!(combined.access(), AccessMode::Write);
    let OpenNode::File(combined_file) = &combined else {
        panic!("combined libc flags did not open a file")
    };
    assert_eq!(
        crate::wasi_filesystem::p2::types::route_write(
            &generation_handle,
            combined_file,
            0,
            Bytes::from_static(b"p"),
        )
        .unwrap()
        .await
        .unwrap(),
        1
    );
    let descriptor = AgentDescriptor::new(combined, "/file".into());
    assert_eq!(
        agent_descriptor_flags(&descriptor).unwrap(),
        types::DescriptorFlags::WRITE
    );
    drop(descriptor);

    let no_access = route_open(
        &generation_handle,
        PathTarget::at(root_directory, "file"),
        types::PathFlags::empty(),
        types::OpenFlags::empty(),
        types::DescriptorFlags::empty(),
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap()
    .node;
    let OpenNode::File(no_access_file) = &no_access else {
        panic!("observed file is not a file")
    };
    let denied_read = agent_filesystem::read_file(
        &generation_handle,
        no_access_file,
        agent_filesystem::ReadRange {
            offset: 0,
            length: 1,
        },
    )
    .unwrap_err();
    let denied_write = crate::wasi_filesystem::p2::types::route_write(
        &generation_handle,
        no_access_file,
        0,
        Bytes::from_static(b"x"),
    )
    .err()
    .unwrap();
    assert_eq!(denied_write.downcast().unwrap(), P2Error::NotPermitted);
    assert_eq!(
        p2_agent_error(AgentFilesystemError::Access(denied_read))
            .downcast()
            .unwrap(),
        P2Error::NotPermitted
    );
    assert!(matches!(
        p3_agent_error(AgentFilesystemError::Access(denied_read))
            .downcast()
            .unwrap(),
        types::ErrorCode::NotPermitted
    ));
    let descriptor = AgentDescriptor::new(no_access, "/file".into());
    assert_eq!(
        agent_descriptor_flags(&descriptor).unwrap(),
        types::DescriptorFlags::empty()
    );
    drop(descriptor);
    route_replay_times(
        &generation_handle,
        AgentTarget::Open(&node),
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(300)),
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(400)),
    )
    .unwrap()
    .await
    .unwrap();

    crate::wasi_filesystem::p2::types::route_create_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p2-directory"),
    )
    .await
    .unwrap();
    route_create_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p3-directory"),
    )
    .await
    .unwrap();
    let p2_existing_directory = crate::wasi_filesystem::p2::types::route_create_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p2-directory"),
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert_eq!(
        p2_existing_directory,
        wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Exist
    );
    let p3_existing_directory = route_create_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p3-directory"),
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert!(matches!(p3_existing_directory, types::ErrorCode::Exist));
    crate::wasi_filesystem::p2::types::route_create_symlink(
        &generation_handle,
        PathTarget::at(root_directory, "p2-symlink"),
        "p2-directory",
    )
    .await
    .unwrap();
    route_create_symlink(
        &generation_handle,
        PathTarget::at(root_directory, "p3-symlink"),
        "p3-directory",
    )
    .await
    .unwrap();
    let p2_existing_symlink = crate::wasi_filesystem::p2::types::route_create_symlink(
        &generation_handle,
        PathTarget::at(root_directory, "p2-symlink"),
        "p2-directory",
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert_eq!(
        p2_existing_symlink,
        wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Exist
    );
    let p3_existing_symlink = route_create_symlink(
        &generation_handle,
        PathTarget::at(root_directory, "p3-symlink"),
        "p3-directory",
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert!(matches!(p3_existing_symlink, types::ErrorCode::Exist));
    let p2_missing_file = crate::wasi_filesystem::p2::types::route_unlink(
        &generation_handle,
        PathTarget::at(root_directory, "p2-missing-file"),
        ObjectKind::File,
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert_eq!(
        p2_missing_file,
        wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NoEntry
    );
    let p3_missing_file = route_unlink(
        &generation_handle,
        PathTarget::at(root_directory, "p3-missing-file"),
        ObjectKind::File,
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert!(matches!(p3_missing_file, types::ErrorCode::NoEntry));
    let p2_missing_directory = crate::wasi_filesystem::p2::types::route_remove_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p2-missing-directory"),
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert_eq!(
        p2_missing_directory,
        wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NoEntry
    );
    let p3_missing_directory = route_remove_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p3-missing-directory"),
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert!(matches!(p3_missing_directory, types::ErrorCode::NoEntry));
    let p2_follow_error = crate::wasi_filesystem::p2::types::route_hard_link(
        &generation_handle,
        PathTarget::at(root_directory, "file"),
        wasmtime_wasi::p2::bindings::filesystem::types::PathFlags::SYMLINK_FOLLOW,
        PathTarget::at(root_directory, "p2-invalid-link"),
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert_eq!(
        p2_follow_error,
        wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Invalid
    );
    let p3_follow_error = route_hard_link(
        &generation_handle,
        PathTarget::at(root_directory, "file"),
        types::PathFlags::SYMLINK_FOLLOW,
        PathTarget::at(root_directory, "p3-invalid-link"),
    )
    .await
    .unwrap_err()
    .downcast()
    .unwrap();
    assert!(matches!(p3_follow_error, types::ErrorCode::Invalid));
    crate::wasi_filesystem::p2::types::route_hard_link(
        &generation_handle,
        PathTarget::at(root_directory, "file"),
        wasmtime_wasi::p2::bindings::filesystem::types::PathFlags::empty(),
        PathTarget::at(root_directory, "p2-hard-link"),
    )
    .await
    .unwrap();
    route_hard_link(
        &generation_handle,
        PathTarget::at(root_directory, "file"),
        types::PathFlags::empty(),
        PathTarget::at(root_directory, "p3-hard-link"),
    )
    .await
    .unwrap();
    crate::wasi_filesystem::p2::types::route_rename(
        &generation_handle,
        PathTarget::at(root_directory, "p2-hard-link"),
        PathTarget::at(root_directory, "p2-moved"),
    )
    .await
    .unwrap();
    route_rename(
        &generation_handle,
        PathTarget::at(root_directory, "p3-hard-link"),
        PathTarget::at(root_directory, "p3-moved"),
    )
    .await
    .unwrap();
    crate::wasi_filesystem::p2::types::route_unlink(
        &generation_handle,
        PathTarget::at(root_directory, "p2-moved"),
        ObjectKind::File,
    )
    .await
    .unwrap();
    route_unlink(
        &generation_handle,
        PathTarget::at(root_directory, "p3-moved"),
        ObjectKind::File,
    )
    .await
    .unwrap();
    crate::wasi_filesystem::p2::types::route_unlink(
        &generation_handle,
        PathTarget::at(root_directory, "p2-symlink"),
        ObjectKind::Symlink,
    )
    .await
    .unwrap();
    route_unlink(
        &generation_handle,
        PathTarget::at(root_directory, "p3-symlink"),
        ObjectKind::Symlink,
    )
    .await
    .unwrap();
    crate::wasi_filesystem::p2::types::route_remove_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p2-directory"),
    )
    .await
    .unwrap();
    route_remove_directory(
        &generation_handle,
        PathTarget::at(root_directory, "p3-directory"),
    )
    .await
    .unwrap();

    close(node).await.unwrap();
    close(root_node).await.unwrap();

    let reconstructing = finish_replay(reconstructing).await.unwrap();
    let filesystem = finish_reconstruction(reconstructing).await.unwrap();
    let generation_handle = resident_generation_handle(&filesystem);
    let reopened = route_open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, "file").unwrap(),
        types::PathFlags::SYMLINK_FOLLOW,
        types::OpenFlags::empty(),
        types::DescriptorFlags::READ,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap();
    let OpenNode::File(file) = &reopened.node else {
        panic!("sandbox file reopen returned a directory")
    };
    let observed = route_attributes(&generation_handle, AgentTarget::Open(&reopened.node))
        .await
        .unwrap();
    assert_eq!(observed.size, 12);
    assert_eq!(observed.data_access_timestamp.unwrap().seconds, 300);
    assert_eq!(observed.data_modification_timestamp.unwrap().seconds, 400);
    assert_eq!(
        route_read_file(&generation_handle, file, 0, 64)
            .await
            .unwrap(),
        Bytes::from_static(b"p2-replay-p3")
    );
    close(reopened.node).await.unwrap();

    let root_node = route_open(
        &generation_handle,
        PathTarget::at_root(&generation_handle, ".").unwrap(),
        types::PathFlags::SYMLINK_FOLLOW,
        types::OpenFlags::DIRECTORY,
        types::DescriptorFlags::READ,
        Box::pin(std::future::pending()),
    )
    .await
    .unwrap()
    .node;
    let OpenNode::Directory(root_directory) = &root_node else {
        panic!("sandbox root reopen returned a file")
    };
    assert_eq!(
        route_list_directory(&generation_handle, root_directory)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>(),
        vec!["file".to_string()]
    );
    close(root_node).await.unwrap();
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    delete(seal(filesystem)).await.unwrap();
}

#[test]
fn p2_p3_write_progress_and_placement_translation_is_identical() {
    for placement in [WritePlacement::At(11), WritePlacement::Append] {
        let p2 = p2_agent_write_result(Ok(WriteResult { written: 7 }), placement).unwrap();
        let p3 = p3_agent_write_result(Ok(WriteResult { written: 7 }), placement)
            .unwrap()
            .unwrap();

        assert_eq!(p2.0 as usize, p3.0);
        assert_eq!(p2.1, p3.1);
        assert_eq!(
            p2.1,
            match placement {
                WritePlacement::At(_) => WritePlacement::At(18),
                WritePlacement::Append => WritePlacement::Append,
            }
        );
    }
}

#[test]
fn p2_p3_write_quota_capacity_and_permission_errors_are_identical() {
    fn storage_error() -> FilesystemStorageError {
        FilesystemStorageError::io("write", None, std::io::ErrorKind::StorageFull.into())
    }

    use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

    for (p2_error, p3_error, p2_expected, p3_expected) in [
        (
            AgentFilesystemError::AgentQuota(storage_error()),
            AgentFilesystemError::AgentQuota(storage_error()),
            P2ErrorCode::Quota,
            "ErrorCode::Quota",
        ),
        (
            AgentFilesystemError::PhysicalCapacity(storage_error()),
            AgentFilesystemError::PhysicalCapacity(storage_error()),
            P2ErrorCode::InsufficientSpace,
            "ErrorCode::InsufficientSpace",
        ),
        (
            AgentFilesystemError::Access(agent_filesystem::AccessError::NotPermitted),
            AgentFilesystemError::Access(agent_filesystem::AccessError::NotPermitted),
            P2ErrorCode::NotPermitted,
            "ErrorCode::NotPermitted",
        ),
    ] {
        let p2: P2ErrorCode = p2_agent_write_result(Err(p2_error), WritePlacement::At(0))
            .unwrap_err()
            .downcast()
            .unwrap();
        let p3 = p3_agent_write_result(Err(p3_error), WritePlacement::At(0))
            .unwrap()
            .unwrap_err();

        assert_eq!(p2, p2_expected);
        assert_eq!(format!("{p3:?}"), p3_expected);
    }

    let p3_stream = filesystem_write_result_to_wasi(Err(FilesystemWriteFailure::Guest(
        types::ErrorCode::Quota,
    )))
    .unwrap()
    .unwrap_err();
    assert_eq!(format!("{p3_stream:?}"), "ErrorCode::Quota");
}

#[test]
fn p3_write_runtime_invalidation_is_a_future_trap() {
    assert!(
        p3_agent_write_result(
            Err(AgentFilesystemError::RuntimeInvalidated),
            WritePlacement::Append,
        )
        .is_err()
    );
}

#[test]
fn p2_p3_read_only_flag_mapping_is_identical() {
    use wasmtime_wasi::p2::bindings::filesystem::types::DescriptorFlags as P2DescriptorFlags;

    let p2_flags =
        P2DescriptorFlags::READ | P2DescriptorFlags::WRITE | P2DescriptorFlags::FILE_INTEGRITY_SYNC;
    let p3_flags = types::DescriptorFlags::READ
        | types::DescriptorFlags::WRITE
        | types::DescriptorFlags::FILE_INTEGRITY_SYNC;

    let p2_visible = p2_visible_descriptor_flags(p2_flags, true);
    let p3_visible = p3_visible_descriptor_flags(p3_flags, true);

    assert_eq!(
        p2_visible.contains(P2DescriptorFlags::READ),
        p3_visible.contains(types::DescriptorFlags::READ)
    );
    assert_eq!(
        p2_visible.contains(P2DescriptorFlags::WRITE),
        p3_visible.contains(types::DescriptorFlags::WRITE)
    );
    assert_eq!(
        p2_visible.contains(P2DescriptorFlags::FILE_INTEGRITY_SYNC),
        p3_visible.contains(types::DescriptorFlags::FILE_INTEGRITY_SYNC)
    );
}

#[test]
async fn immutable_initial_file_stat_preserves_typed_observation_trap() {
    let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
    let call = async {
        observe_filesystem_operation(std::future::ready(kind), std::future::pending())
            .await
            .map_err(wasmtime::Error::from)
    };
    let error = p3_stable_initial_file_stat(call)
        .await
        .unwrap_err()
        .downcast()
        .unwrap_err();
    assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&kind));
}

#[test]
async fn immutable_initial_file_stat_traps_filesystem_errors_without_recording() {
    let error = p3_stable_initial_file_stat(std::future::ready(Ok(Err(
        SerializableP3FileSystemError::Generic("missing pinned file".to_string()),
    ))))
    .await
    .unwrap_err();
    assert!(error.downcast().is_err());
}

#[test]
fn metadata_hash_from_stat_matches_p2_hash() {
    let stat = types::DescriptorStat {
        type_: types::DescriptorType::RegularFile,
        link_count: 1,
        size: 42,
        data_access_timestamp: None,
        data_modification_timestamp: Some(
            wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                seconds: 123,
                nanoseconds: 456,
            },
        ),
        status_change_timestamp: None,
    };
    let hash = metadata_hash_from_stat(&stat).unwrap();
    let (lower, upper) = calculate_metadata_hash_parts(Some((123, 456)), 42);
    assert_eq!(hash.lower, lower);
    assert_eq!(hash.upper, upper);
}

#[test]
fn metadata_hash_from_stat_fails_with_overflow_for_pre_epoch_timestamps() {
    let stat = types::DescriptorStat {
        type_: types::DescriptorType::RegularFile,
        link_count: 1,
        size: 42,
        data_access_timestamp: None,
        data_modification_timestamp: Some(
            wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                seconds: -1,
                nanoseconds: 0,
            },
        ),
        status_change_timestamp: None,
    };
    let error = metadata_hash_from_stat(&stat).unwrap_err();
    assert!(matches!(
        error.downcast().unwrap(),
        types::ErrorCode::Overflow
    ));
}

#[cfg(unix)]
#[test]
async fn p3_fs_stat_at_follow_symlink_does_not_mutate_symlink_timestamps() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let target = tempdir.path().join("target");
    let link = tempdir.path().join("link");
    std::fs::write(&target, b"target").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let new = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
    set_symlink_times(
        &link,
        Some(SystemTimeSpec::from(old)),
        Some(SystemTimeSpec::from(old)),
    )
    .unwrap();
    let before = std::fs::symlink_metadata(&link)
        .unwrap()
        .modified()
        .unwrap();

    let new_timestamp = SerializableDateTime::from(new);
    apply_stat_response(
        Ok(types::DescriptorStat {
            type_: types::DescriptorType::RegularFile,
            link_count: 1,
            size: 6,
            data_access_timestamp: Some(new_timestamp.clone().into()),
            data_modification_timestamp: Some(new_timestamp.clone().into()),
            status_change_timestamp: None,
        }),
        HostResponseP3FileSystemStat {
            result: Ok(SerializableFileTimes {
                data_access_timestamp: Some(new_timestamp.clone()),
                data_modification_timestamp: Some(new_timestamp),
            }),
        },
    )
    .await
    .unwrap();

    let after = std::fs::symlink_metadata(&link)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        after, before,
        "stat-at with symlink-follow is a read-only operation and must not rewrite the symlink itself"
    );
}

#[cfg(unix)]
#[test]
async fn p3_fs_stat_at_follow_symlink_does_not_mutate_target_timestamps() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let target = tempdir.path().join("target");
    let link = tempdir.path().join("link");
    std::fs::write(&target, b"target").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let new = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
    set_times(
        &target,
        Some(SystemTimeSpec::from(old)),
        Some(SystemTimeSpec::from(old)),
    )
    .unwrap();
    let before = std::fs::metadata(&target).unwrap().modified().unwrap();

    let new_timestamp = SerializableDateTime::from(new);
    apply_stat_response(
        Ok(types::DescriptorStat {
            type_: types::DescriptorType::RegularFile,
            link_count: 1,
            size: 6,
            data_access_timestamp: Some(new_timestamp.clone().into()),
            data_modification_timestamp: Some(new_timestamp.clone().into()),
            status_change_timestamp: None,
        }),
        HostResponseP3FileSystemStat {
            result: Ok(SerializableFileTimes {
                data_access_timestamp: Some(new_timestamp.clone()),
                data_modification_timestamp: Some(new_timestamp),
            }),
        },
    )
    .await
    .unwrap();

    let after = std::fs::metadata(&target).unwrap().modified().unwrap();
    assert_eq!(
        after, before,
        "stat-at with symlink-follow is a read-only operation and must not rewrite the target"
    );
}
