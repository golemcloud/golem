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
async fn cancelled_custom_begin_holds_parent_initiation_until_terminal_settles() {
    let parent_counter = Arc::new(AtomicUsize::new(0));
    let initiation = CustomChildInitiation::new(parent_counter.clone());
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (verdict_tx, verdict_rx) = oneshot::channel();
    let lifecycle = CustomBeginLifecycle::new(verdict_tx, Some(initiation), event_tx);
    let release_terminal = Arc::new(Notify::new());
    let release_terminal_task = release_terminal.clone();
    lifecycle.set_coordinator(tokio::spawn(async move {
        assert!(matches!(
            verdict_rx.await,
            Ok(CustomBeginVerdict::Cancelled)
        ));
        release_terminal_task.notified().await;
        Ok(Some(OplogIndex::from_u64(42)))
    }));

    lifecycle.settle(CustomBeginVerdict::Cancelled);
    let event = event_rx.recv().await.expect("settlement event");
    assert_eq!(parent_counter.load(Ordering::Acquire), 1);

    let DropEvent::SettleCustomInvocationBegin {
        lifecycle,
        mut terminal,
    } = event
    else {
        panic!("expected custom begin settlement event");
    };
    release_terminal.notify_one();
    let cancelled_start = terminal
        .take()
        .expect("settlement task")
        .await
        .expect("settlement task join")
        .expect("settlement task result");
    assert_eq!(cancelled_start, Some(OplogIndex::from_u64(42)));
    assert_eq!(parent_counter.load(Ordering::Acquire), 1);
    lifecycle.release_initiation();
    assert_eq!(parent_counter.load(Ordering::Acquire), 0);
}
