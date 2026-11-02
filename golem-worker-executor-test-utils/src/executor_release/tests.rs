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
use std::sync::Arc;
use test_r::test;

#[test]
#[test_r::timeout("10s")]
async fn release_waits_for_cleanup_that_outlives_the_service_graph() {
    let parent = tempfile::tempdir().unwrap();
    let scratch = parent.path().join(".scratch");
    std::fs::create_dir(&scratch).unwrap();
    let lock_path = parent.path().join("root-lock");
    let held_lock = std::fs::File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .unwrap();
    held_lock.lock().unwrap();
    let replacement_lock = std::fs::File::options()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    let shutdown = Shutdown::new();
    let loops = InvocationLoops::new(shutdown.token());
    let graph = Arc::new(());
    let released = Arc::downgrade(&graph);
    let (allow, allowed) = tokio::sync::oneshot::channel();
    let old_scratch = scratch.clone();
    shutdown.spawn(async move {
        allowed.await.unwrap();
        tokio::fs::remove_dir_all(old_scratch).await.unwrap();
        drop(held_lock);
    });
    drop(graph);
    assert!(released.upgrade().is_none());
    shutdown.cancel();
    let waiting = wait_for_release(&shutdown, &loops, released);
    tokio::pin!(waiting);
    assert!(futures::poll!(&mut waiting).is_pending());
    assert!(scratch.is_dir());
    assert!(
        replacement_lock.try_lock().is_err(),
        "old cleanup still owns its storage lock"
    );
    allow.send(()).unwrap();
    waiting.await.unwrap();
    assert!(!scratch.exists());
    replacement_lock.try_lock().unwrap();
    std::fs::create_dir(&scratch).unwrap();
    tokio::task::yield_now().await;
    assert!(
        scratch.is_dir(),
        "no old cleanup can remove a replacement's directory"
    );
}
