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
#[cfg(unix)]
use file_creation_mask::mask_without_owner_write;
#[cfg(target_os = "linux")]
use file_creation_mask::{probed_file_creation_mask, status_umask};
#[cfg(target_os = "linux")]
use file_creation_mask_for_test::{thread_file_creation_mask, with_private_file_creation_mask};
use test_r::test;

fn initial_files_service() -> Arc<InitialAgentFilesService> {
    Arc::new(InitialAgentFilesService::new(Arc::new(
        golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
    )))
}

#[cfg(unix)]
#[test]
fn the_file_mode_creation_mask_loses_only_the_owner_write_bit() {
    [
        (0o022, 0o022),
        (0o077, 0o077),
        (0o222, 0o022),
        (0o277, 0o077),
    ]
    .into_iter()
    .for_each(|(mask, expected)| {
        assert_eq!(
            mask_without_owner_write(mask),
            expected,
            "the mask {mask:o} must become {expected:o}"
        );
    });
}

#[cfg(target_os = "linux")]
#[test]
fn the_file_mode_creation_mask_comes_from_the_umask_line_of_a_status_text() {
    assert_eq!(
        status_umask("Name:\tworker-executor\nUmask:\t0277\nState:\tR (running)\n"),
        Some(0o277)
    );
    assert_eq!(
        status_umask("Name:\tworker-executor\nState:\tR (running)\n"),
        None
    );
}

/// The test makes its temporary root under the mask that the process has. Then it binds the
/// filesystems on a thread with the mask 0o227, which must stay as it is.
#[cfg(target_os = "linux")]
#[test]
fn agent_filesystems_binding_leaves_the_file_mode_creation_mask_as_it_is() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        mode: FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        ..FilesystemStorageConfig::default()
    };

    let (bound, mask) = with_private_file_creation_mask(0o227, move || {
        let bound = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(AgentFilesystems::new(&settings, initial_files_service()));
        (bound, thread_file_creation_mask())
    });

    bound.unwrap();
    assert_eq!(
        mask, 0o227,
        "binding must leave the mask 227 as it is, and the mask is {mask:o}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_file_mode_creation_mask_probe_masks_group_and_other_and_gives_the_mask_before_the_call() {
    let (probed, after) = with_private_file_creation_mask(0o227, || {
        (probed_file_creation_mask(), thread_file_creation_mask())
    });

    assert_eq!(
        probed, 0o227,
        "the call must give the mask 227 from before the call, and it gave {probed:o}"
    );
    assert_eq!(
        after, 0o077,
        "the call must set the mask 77, and the mask is {after:o}"
    );
}

/// The executor reads the mask of its thread and clears only the owner write bit of it.
#[cfg(target_os = "linux")]
#[test]
fn keeping_the_owner_write_permission_clears_only_the_owner_write_bit_of_the_mask() {
    let mask = with_private_file_creation_mask(0o277, || {
        keep_owner_write_permission();
        thread_file_creation_mask()
    });
    assert_eq!(
        mask, 0o077,
        "the mask 277 must become 77, and the mask is {mask:o}"
    );
}

struct BindingSpaceObservationGuard(Option<FilesystemSpace>);

impl Drop for BindingSpaceObservationGuard {
    fn drop(&mut self) {
        BINDING_SPACE_OBSERVATION.set(self.0);
    }
}

async fn with_binding_space_observation<T>(
    space: FilesystemSpace,
    binding: impl Future<Output = T>,
) -> T {
    let previous = BINDING_SPACE_OBSERVATION.replace(Some(space));
    let _guard = BindingSpaceObservationGuard(previous);
    binding.await
}

#[test]
fn object_limit_policy_resolves_floor_proportional_value_and_ceiling() {
    let policy = FilesystemObjectLimitPolicyConfig::new(32_768, 100, 50_000).unwrap();

    assert_eq!(policy.resolve(1).unwrap().filesystem_objects, 100);
    assert_eq!(
        policy
            .resolve(u64::try_from(BYTES_PER_GIB).unwrap())
            .unwrap()
            .filesystem_objects,
        32_768
    );
    assert_eq!(
        policy
            .resolve(u64::try_from(BYTES_PER_GIB * 10).unwrap())
            .unwrap()
            .filesystem_objects,
        50_000
    );
}

#[test]
fn object_limit_policy_still_rejects_zero_allocated_bytes() {
    let error = FilesystemObjectLimitPolicyConfig::default()
        .resolve(0)
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("resolve nonzero agent filesystem storage limit")
    );
}

#[test]
fn storage_limits_are_unlimited_from_the_sentinel_and_finite_below_it() {
    let policy = FilesystemObjectLimitPolicyConfig::default();
    let sentinel = AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE;

    [AgentAccounting::ProjectQuotas, AgentAccounting::Development]
        .into_iter()
        .for_each(|accounting| {
            assert_eq!(
                resolve_storage_limits(&policy, sentinel, accounting).unwrap(),
                ResolvedStorageLimits::Unlimited
            );
            assert_eq!(
                resolve_storage_limits(&policy, u64::MAX, accounting).unwrap(),
                ResolvedStorageLimits::Unlimited
            );
            match resolve_storage_limits(&policy, sentinel - 1, accounting).unwrap() {
                ResolvedStorageLimits::Finite(limits) => {
                    assert_eq!(limits.allocated_bytes, sentinel - 1)
                }
                ResolvedStorageLimits::Unlimited => {
                    panic!("an allocation one byte below the sentinel must be finite")
                }
            }
        });
}

#[test]
fn storage_limits_refuse_an_allocation_of_zero_bytes() {
    [AgentAccounting::ProjectQuotas, AgentAccounting::Development]
        .into_iter()
        .for_each(|accounting| {
            let error = resolve_storage_limits(
                &FilesystemObjectLimitPolicyConfig::default(),
                0,
                accounting,
            )
            .unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains("resolve nonzero agent filesystem storage limit"),
                "{error}"
            );
        });
}

#[test]
fn unaccounted_storage_cannot_accept_a_finite_registry_limit() {
    let policy = FilesystemObjectLimitPolicyConfig::default();
    let sentinel = AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE;
    for allocated_bytes in [0, 1024, sentinel - 1] {
        let error = resolve_storage_limits(&policy, allocated_bytes, AgentAccounting::Unaccounted)
            .expect_err("a finite Registry safety limit requires a quota-capable backend");
        assert!(
            error.to_string().contains("finite storage limit"),
            "{error}"
        );
    }
    for allocated_bytes in [sentinel, u64::MAX] {
        assert_eq!(
            resolve_storage_limits(&policy, allocated_bytes, AgentAccounting::Unaccounted).unwrap(),
            ResolvedStorageLimits::Unlimited
        );
    }
}

#[test]
fn storage_without_per_agent_accounting_accepts_only_unlimited_policy() {
    let policy = FilesystemObjectLimitPolicyConfig::default();
    let sentinel = AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE;

    [sentinel, u64::MAX]
        .into_iter()
        .for_each(|allocated_bytes| {
            assert_eq!(
                resolve_storage_limits(&policy, allocated_bytes, AgentAccounting::Unaccounted)
                    .unwrap(),
                ResolvedStorageLimits::Unlimited,
                "{allocated_bytes}"
            );
        });
}

#[test]
async fn registry_disk_sentinel_resolves_as_unlimited() {
    let settings = FilesystemStorageConfig::default();
    let observed_total_bytes = settings.pressure.target_available_bytes();
    let filesystems = with_binding_space_observation(
        FilesystemSpace::Observed {
            total_bytes: observed_total_bytes,
            available_bytes: observed_total_bytes,
            total_filesystem_objects: u64::MAX,
            available_filesystem_objects: u64::MAX,
        },
        AgentFilesystems::new(&settings, initial_files_service()),
    )
    .await
    .unwrap();

    assert_eq!(filesystems.agent_accounting(), AgentAccounting::Development);
    assert!(matches!(
        filesystems
            .resolved_limits(AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE - 1)
            .unwrap(),
        ResolvedStorageLimits::Finite(_)
    ));
    assert_eq!(
        filesystems
            .resolved_limits(AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE)
            .unwrap(),
        ResolvedStorageLimits::Unlimited
    );
    assert_eq!(
        filesystems.resolved_limits(u64::MAX).unwrap(),
        ResolvedStorageLimits::Unlimited
    );
}

#[test]
async fn agent_filesystems_binding_rejects_pressure_target_above_observed_capacity() {
    let settings = FilesystemStorageConfig::default();
    let observed_total_bytes = settings.pressure.target_available_bytes() - 1;

    let result = with_binding_space_observation(
        FilesystemSpace::Observed {
            total_bytes: observed_total_bytes,
            available_bytes: observed_total_bytes,
            total_filesystem_objects: u64::MAX,
            available_filesystem_objects: u64::MAX,
        },
        AgentFilesystems::new(&settings, initial_files_service()),
    )
    .await;

    let error = result
        .err()
        .expect("binding must reject a pressure target above observed capacity");
    assert!(
        error
            .to_string()
            .contains("fit filesystem pressure byte target within the volume capacity")
    );
}

#[test]
async fn agent_filesystems_binding_accepts_pressure_target_equal_to_observed_capacity() {
    let settings = FilesystemStorageConfig::default();
    let observed_total_bytes = settings.pressure.target_available_bytes();

    let result = with_binding_space_observation(
        FilesystemSpace::Observed {
            total_bytes: observed_total_bytes,
            available_bytes: observed_total_bytes,
            total_filesystem_objects: u64::MAX,
            available_filesystem_objects: u64::MAX,
        },
        AgentFilesystems::new(&settings, initial_files_service()),
    )
    .await;

    if let Err(error) = result {
        panic!("binding rejected a pressure target equal to observed capacity: {error}");
    }
}

#[test]
async fn agent_filesystems_make_the_scratch_and_initial_files_directories_at_binding() {
    let root = tempfile::tempdir().unwrap();
    [".scratch", ".initial-files"].into_iter().for_each(|name| {
        std::fs::create_dir_all(root.path().join(name).join("left-by-an-earlier-process")).unwrap();
    });
    let settings = FilesystemStorageConfig {
        mode: FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        ..FilesystemStorageConfig::default()
    };

    let _filesystems = AgentFilesystems::new(&settings, initial_files_service())
        .await
        .unwrap();

    [".scratch", ".initial-files"].into_iter().for_each(|name| {
        assert!(
            std::fs::read_dir(root.path().join(name))
                .unwrap()
                .next()
                .is_none(),
            "binding must remove what an earlier process left in {name}"
        );
    });
}

/// The loader owns `.initial-files`: the directory stays while the loader lives, also after the
/// service is dropped, and goes away with the loader.
#[test]
async fn agent_filesystems_give_the_initial_files_directory_to_their_one_file_loader() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        mode: FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        ..FilesystemStorageConfig::default()
    };
    let initial_files = root.path().join(".initial-files");

    let filesystems = AgentFilesystems::new(&settings, initial_files_service())
        .await
        .unwrap();
    let loader = filesystems.file_loader();
    assert!(Arc::ptr_eq(&loader, &filesystems.file_loader()));
    drop(filesystems);

    assert!(
        initial_files.is_dir(),
        "the directory must live while its loader lives"
    );
    drop(loader);
    assert!(
        !initial_files.exists(),
        "the directory must go away with its loader"
    );
}

#[test]
async fn a_failed_binding_discards_both_host_directories() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        mode: FilesystemStorageMode::Directory {
            root: root.path().into(),
        },
        ..FilesystemStorageConfig::default()
    };
    let observed_total_bytes = settings.pressure.target_available_bytes() - 1;

    let result = with_binding_space_observation(
        FilesystemSpace::Observed {
            total_bytes: observed_total_bytes,
            available_bytes: observed_total_bytes,
            total_filesystem_objects: u64::MAX,
            available_filesystem_objects: u64::MAX,
        },
        AgentFilesystems::new(&settings, initial_files_service()),
    )
    .await;

    assert!(result.is_err(), "binding must fail");
    assert!(
        std::fs::read_dir(root.path()).unwrap().next().is_none(),
        "a failed binding must leave no host directory"
    );
}

#[test]
async fn agent_filesystems_binding_rejects_object_pressure_target_above_observed_capacity() {
    let settings = FilesystemStorageConfig::default();
    let observed_total_objects = settings.pressure.target_available_filesystem_objects() - 1;

    let result = with_binding_space_observation(
        FilesystemSpace::Observed {
            total_bytes: u64::MAX,
            available_bytes: u64::MAX,
            total_filesystem_objects: observed_total_objects,
            available_filesystem_objects: observed_total_objects,
        },
        AgentFilesystems::new(&settings, initial_files_service()),
    )
    .await;

    let error = result
        .err()
        .expect("binding must reject an object pressure target above observed capacity");
    assert!(
        error
            .to_string()
            .contains("fit filesystem pressure object target within the volume capacity")
    );
}
