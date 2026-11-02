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
use test_r::test;

#[test]
fn current_thread_runtime_uses_the_blocking_pool_for_both_modes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let caller = std::thread::current().id();
        for mode in [BlockingTask::InPlace, BlockingTask::Spawn] {
            let worker = mode.run(|| std::thread::current().id()).await.unwrap();
            assert_ne!(worker, caller);
        }
    });
}

#[test]
fn multi_thread_runtime_respects_the_execution_mode() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let caller = std::thread::current().id();
        let in_place = BlockingTask::InPlace
            .run(|| std::thread::current().id())
            .await
            .unwrap();
        let spawned = BlockingTask::Spawn
            .run(|| std::thread::current().id())
            .await
            .unwrap();
        assert_eq!(in_place, caller);
        assert_ne!(spawned, caller);
    });
}

#[test]
fn panics_are_caught_in_both_modes() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        for mode in [BlockingTask::InPlace, BlockingTask::Spawn] {
            assert!(matches!(
                mode.run(|| panic!("scripted task panic")).await,
                Err(BlockingTaskError::Panicked)
            ));
        }
    });
}
