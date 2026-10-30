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

use super::{StorePolicy, store_backup_options, store_restore_options};
use crate::filesystem_snapshot::ChangeDetection;
use crate::filesystem_snapshot::backend::rustic::pruning::eligibility::Percent;
use crate::filesystem_snapshot::backend::rustic::test_support::config;
use pretty_assertions::assert_eq;
use rustic_core::repofile::SnapshotId;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;
use test_r::test;

#[test]
fn the_policy_takes_the_configured_values_and_the_options_are_strict() {
    let policy = StorePolicy::from_config(&config());
    let backup = store_backup_options(&policy, None);
    let restore = store_restore_options(&policy);

    assert_eq!(
        (
            policy.deadline,
            policy.save_threads.map(NonZeroUsize::get),
            policy.restore_reader_threads.get(),
            policy.prune.keep_delete,
            policy.prune.fast_repack,
            policy.prune_threshold,
        ),
        (
            Duration::from_secs(30),
            Some(3),
            4,
            Duration::from_secs(15 * 60),
            true,
            Percent(10),
        )
    );
    assert_eq!(
        (
            backup.fail_on_read_error,
            backup.threads.map(NonZeroUsize::get),
            backup.as_path.as_deref().map(Path::to_path_buf),
            restore.fail_on_metadata_error,
            restore.no_ownership,
            restore.reader_threads.map(NonZeroUsize::get),
        ),
        (
            true,
            Some(3),
            Some(Path::new("/").to_path_buf()),
            true,
            true,
            Some(4),
        )
    );
}

#[test]
fn a_size_and_mtime_save_names_its_parent_and_ignores_the_change_time_and_a_full_save_forces_a_read()
 {
    let policy = StorePolicy::from_config(&config());
    let parent = SnapshotId::default();
    let modes = [
        Some((parent, ChangeDetection::SizeMtime)),
        Some((parent, ChangeDetection::Full)),
        None,
    ]
    .map(|mode| {
        let options = store_backup_options(&policy, mode).parent_opts;
        (
            options.parents,
            options.ignore_ctime,
            options.ignore_inode,
            options.force,
        )
    });

    assert_eq!(
        modes,
        [
            (vec![parent.to_hex().to_string()], true, false, false),
            (Vec::new(), false, false, true),
            (Vec::new(), false, false, true),
        ]
    );
}
