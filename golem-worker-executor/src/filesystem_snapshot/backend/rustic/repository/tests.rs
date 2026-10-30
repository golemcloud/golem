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

use super::{RepositoryKey, backup_options, open_existing, open_or_create, repository_options};
use crate::filesystem_snapshot::backend::rustic::pruning::prune_options;
use crate::filesystem_snapshot::backend::rustic::store_policy::PruneSettings;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    LIMIT, LONG_DEADLINE, NEVER, REPOSITORY_LIMIT, SHORT_WRITE_DEADLINE, backend_of, eventually,
    key, listed_names, name, one_file_tree, policy, polled_until, repository_fixture_tree,
    repository_key, restored_listing, seconds, store, store_with_deadline, stored_paths,
};
use crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking;
use crate::filesystem_snapshot::test_support::fixture::listing;
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{FilesystemSnapshotStore, SaveError, Unlimited};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio_util::task::AbortOnDropHandle;

#[test]
#[timeout("60s")]
async fn the_first_save_creates_the_repository_with_no_key_file_and_later_saves_open_it() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let store_with_deadline = store_with_deadline(storage.clone(), STORAGE_CALL_DEADLINE);
    let tree = repository_fixture_tree();

    store_with_deadline
        .save(
            &scope,
            &name("p-first"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let config_after_the_first = stored_paths(&storage, &scope)
        .await
        .into_iter()
        .filter(|path| path == "config")
        .count();
    store_with_deadline
        .save(
            &scope,
            &name("p-second"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let paths = stored_paths(&storage, &scope).await;

    assert_eq!(
        (
            config_after_the_first,
            paths.iter().filter(|path| *path == "config").count(),
            paths
                .iter()
                .filter(|path| path.starts_with("keys/"))
                .count(),
            paths
                .iter()
                .filter(|path| path.starts_with("snapshots/"))
                .count(),
        ),
        (1, 1, 0, 2)
    );
}

#[test]
#[timeout("60s")]
async fn a_create_whose_config_write_finds_the_config_of_another_writer_opens_that_repository() {
    // The gate holds the config write of the losing create after both of its checks found no
    // config. The winning create writes the config meanwhile, so the held write finds it.
    let shared = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let losing = ScriptedBlobStorage::new(shared.clone(), |op_label, path| {
        if op_label == "write" && path == Path::new("config") {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let backend =
        |storage: Arc<dyn BlobStorage>| backend_of(storage, &scope, STORAGE_CALL_DEADLINE);
    let loser = backend(losing.clone());
    let lost = tokio::task::spawn_blocking(move || {
        open_or_create(loser, &repository_key()).map(|repository| repository.config().id)
    });
    let held = polled_until(Duration::from_secs(10), || {
        losing
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "write" && path == "config")
    })
    .await;

    let winner = backend(shared.clone());
    let won =
        run_blocking(move || Ok(open_or_create(winner, &repository_key())?.config().id)).await;
    losing.open_gate();
    let lost = tokio::time::timeout(REPOSITORY_LIMIT, lost).await;

    let won = won.unwrap();
    let lost = lost
        .ok()
        .and_then(Result::ok)
        .map(|opened| opened.map_err(|error| error.to_string()));
    assert_eq!((held, lost), (true, Some(Ok(won))));
}

#[test]
fn a_repository_uses_no_rustic_cache() {
    assert!(repository_options().no_cache);
}

#[test]
fn the_key_is_the_aes_key_then_k_then_r() {
    let repository_key = RepositoryKey::new(std::array::from_fn(|index| index as u8)).master_key();

    assert_eq!(
        (
            repository_key.encrypt,
            repository_key.mac.k,
            repository_key.mac.r
        ),
        (
            (0..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
            (48..64).collect::<Vec<u8>>()
        )
    );
}

#[test]
fn the_off_side_of_each_save_and_prune_setting_goes_into_its_rustic_option() {
    let backup = backup_options(None);
    let prune = prune_options(&PruneSettings {
        fast_repack: false,
        keep_delete: Duration::from_secs(23 * 3600),
    })
    .unwrap();

    assert_eq!(
        (
            backup.threads,
            prune.fast_repack,
            seconds(prune.keep_delete)
        ),
        (None, false, 82_800.0)
    );
}

#[test]
fn each_setting_goes_into_its_rustic_option() {
    let backup = backup_options(NonZeroUsize::new(2));
    let prune = prune_options(&PruneSettings {
        fast_repack: true,
        keep_delete: Duration::ZERO,
    })
    .unwrap();

    assert_eq!(
        (
            backup.threads,
            backup.parent_opts.group_by.is_some(),
            backup.as_path,
            prune.fast_repack,
            seconds(prune.keep_delete),
            format!("{:?}", (prune.max_unused, prune.max_repack)),
        ),
        (
            NonZeroUsize::new(2),
            true,
            Some(PathBuf::from("/")),
            true,
            0.0,
            format!(
                "{:?}",
                (
                    rustic_core::LimitOption::Percentage(5),
                    rustic_core::LimitOption::Percentage(10)
                )
            ),
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_new_repository_uses_the_rabin_chunker_zstd_level_3_and_extra_verify() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("settings");

    store
        .save(
            &scope,
            &name("p-settings"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let backend = backend_of(storage, &scope, LONG_DEADLINE);
    let config = tokio::task::spawn_blocking(move || {
        let repository = open_existing(backend, &key()).unwrap().unwrap();
        let config = repository.config();
        (
            config.chunker,
            config.chunk_size,
            config.chunk_min_size,
            config.chunk_max_size,
            config.compression,
            config.extra_verify,
        )
    })
    .await
    .unwrap();

    assert_eq!(
        config,
        (
            Some(rustic_core::repofile::Chunker::Rabin),
            None,
            None,
            None,
            Some(3),
            Some(true)
        )
    );
}

#[test]
#[timeout("60s")]
async fn two_stores_that_create_one_repository_at_the_same_time_both_save() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path == Path::new("config") {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        });
    let policy = policy(LONG_DEADLINE, NEVER, Duration::ZERO);
    let (first, second) = (
        store(storage.clone(), policy.clone()),
        store(storage.clone(), policy),
    );
    let scope = new_scope();
    let (first_tree, second_tree) = (one_file_tree("first"), one_file_tree("second"));
    let config_writes = || {
        storage
            .calls()
            .iter()
            .filter(|(op_label, path)| *op_label == "write" && path == "config")
            .count()
    };

    let (first_name, second_name) = (name("p-first"), name("p-second"));
    let (first_saved, second_saved, both_waited) = tokio::join!(
        first.save(
            &scope,
            &first_name,
            first_tree.path(),
            None,
            never_cancelled(),
            &Unlimited
        ),
        second.save(
            &scope,
            &second_name,
            second_tree.path(),
            None,
            never_cancelled(),
            &Unlimited
        ),
        async {
            let both = eventually(|| config_writes() == 2).await;
            storage.open_gate();
            both
        }
    );
    let mut names = listed_names(&first, &scope).await;
    names.sort();

    assert_eq!(
        (
            both_waited,
            first_saved.map(|_| ()).map_err(|error| error.to_string()),
            second_saved.map(|_| ()).map_err(|error| error.to_string()),
            names,
            restored_listing(&second, &scope, &name("p-first"))
                .await
                .unwrap(),
            restored_listing(&first, &scope, &name("p-second"))
                .await
                .unwrap(),
        ),
        (
            true,
            Ok(()),
            Ok(()),
            vec!["p-first".to_string(), "p-second".to_string()],
            listing(first_tree.path()),
            listing(second_tree.path()),
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_save_that_loses_the_creation_of_the_repository_after_its_first_config_check_saves_into_the_winner()
 {
    // The gate holds the second check of the config by the losing save, which the init of rustic
    // makes before its config write. The winning save creates the repository meanwhile, so that
    // check finds the config.
    let shared = Arc::new(InMemoryBlobStorage::new());
    let checks = Arc::new(AtomicUsize::new(0));
    let losing = ScriptedBlobStorage::new(shared.clone(), {
        let checks = checks.clone();
        move |op_label, path| {
            if op_label == "stat"
                && path == Path::new("config")
                && checks.fetch_add(1, Ordering::SeqCst) == 1
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let winner = store(shared.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let loser = store(losing.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let (won_tree, lost_tree) = (one_file_tree("winner"), one_file_tree("loser"));
    let losing_save = AbortOnDropHandle::new(tokio::spawn({
        let (loser, scope, path) = (loser.clone(), scope.clone(), lost_tree.path().to_path_buf());
        async move {
            loser
                .save(
                    &scope,
                    &name("p-loser"),
                    &path,
                    None,
                    never_cancelled(),
                    &Unlimited,
                )
                .await
        }
    }));
    let held = eventually(|| checks.load(Ordering::SeqCst) >= 2).await;

    let won = winner
        .save(
            &scope,
            &name("p-winner"),
            won_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;
    losing.open_gate();
    let lost = tokio::time::timeout(LIMIT, losing_save).await;

    assert!(won.is_ok(), "{won:?}");
    assert!(matches!(lost, Ok(Ok(Ok(_)))), "{lost:?}");
    assert_eq!(
        (
            held,
            restored_listing(&winner, &scope, &name("p-loser"))
                .await
                .ok(),
            restored_listing(&loser, &scope, &name("p-winner"))
                .await
                .ok(),
        ),
        (
            true,
            Some(listing(lost_tree.path())),
            Some(listing(won_tree.path())),
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_config_write_that_fails_gives_a_storage_error_with_that_failure() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
            if op_label == "write" && path == Path::new("config") {
                Script::Refuse
            } else {
                Script::Pass
            }
        });
    let store = store(storage, policy(SHORT_WRITE_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("never saved");

    let saved = store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await;

    assert!(
        matches!(
            &saved,
            Err(SaveError::Failed(failed))
                if format!("{:#}", failed.cause()).contains("the storage refused the call")
        ),
        "{saved:?}"
    );
}

mod time_zone {
    //! The image of the executor must give rustic a time zone. rustic asks for the time zone of the
    //! system for each timestamp that a save writes, and it logs a warning when it finds none.

    use test_r::test;

    const DOCKERFILE: &str = include_str!("../../../../../docker/Dockerfile");

    #[test]
    fn the_executor_image_sets_a_posix_utc_time_zone() {
        let final_stage = DOCKERFILE
            .rsplit_once("\nFROM ")
            .map(|(_, stage)| stage)
            .unwrap_or_default();

        assert!(
            final_stage.lines().any(|line| line.trim() == "ENV TZ=UTC0"),
            "the final stage of the executor image does not set `ENV TZ=UTC0`:\n{final_stage}"
        );
    }
}
