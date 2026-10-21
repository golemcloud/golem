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
use crate::worker::{InterruptEstablishment, UnloadReason, UnloadRequest};
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use test_r::test;

fn interrupt(kind: InterruptKind) -> PendingWorkerInterrupt {
    PendingWorkerInterrupt {
        kind,
        reacquire_permits: false,
        unload_request: UnloadRequest::ordinary(UnloadReason::Interrupt),
        establishment: InterruptEstablishment::ready(),
    }
}

#[test]
async fn observed_terminal_claims_only_the_exact_published_request_once() {
    for kind in [
        InterruptKind::Interrupt(Timestamp::from(100_u64)),
        InterruptKind::Suspend(Timestamp::from(100_u64)),
        InterruptKind::ShardLost,
    ] {
        let interrupts = Interrupts::default();
        let terminal = interrupts.terminal();
        assert!(interrupts.lock().await.queue(interrupt(kind)));
        let different = InterruptKind::Suspend(Timestamp::from(101_u64));
        assert!(!interrupts.claim_observed_terminal(different).await);
        assert!(*terminal.borrow());
        assert!(interrupts.claim_observed_terminal(kind).await);
        assert!(!*terminal.borrow());
        assert!(interrupts.has_interrupt().await);
        assert!(!interrupts.claim_observed_terminal(kind).await);
        assert!(interrupts.take().await.is_none());
    }
}

#[test]
async fn observed_nonterminal_does_not_consume_the_pending_restart() {
    let interrupts = Interrupts::default();
    assert!(
        interrupts
            .lock()
            .await
            .queue(interrupt(InterruptKind::Restart))
    );
    assert!(
        !interrupts
            .claim_observed_terminal(InterruptKind::Restart)
            .await
    );
    assert_eq!(
        interrupts.take().await.unwrap().kind,
        InterruptKind::Restart
    );
}

#[test]
async fn each_change_publishes_whether_a_terminal_request_waits() {
    let interrupts = Interrupts::default();
    let terminal = interrupts.terminal();

    let queued_restart = interrupts
        .try_lock()
        .unwrap()
        .queue(interrupt(InterruptKind::Restart));
    let restart = (queued_restart, interrupts.terminal_pending());
    let taken_restart = interrupts.take().await.is_some();
    let queued_suspend = interrupts
        .try_lock()
        .unwrap()
        .queue(interrupt(InterruptKind::Suspend(Timestamp::now_utc())));
    let suspend = (queued_suspend, *terminal.borrow());
    let claimed_request = interrupts.lock().await.claim_pending_terminal().is_some();
    let claimed = (
        claimed_request,
        interrupts.terminal_pending(),
        interrupts.has_interrupt().await,
    );
    let queued_interrupt = interrupts
        .try_lock()
        .unwrap()
        .queue(interrupt(InterruptKind::Interrupt(Timestamp::now_utc())));
    let queued_again = (queued_interrupt, interrupts.terminal_pending());
    let taken = (
        interrupts.take().await.is_some(),
        interrupts.terminal_pending(),
    );
    interrupts.reset_terminal_for_new_generation().await;
    let reset = (
        interrupts.has_interrupt().await,
        interrupts.terminal_pending(),
    );

    assert_eq!(restart, (true, false));
    assert!(taken_restart);
    assert_eq!(suspend, (true, true));
    assert_eq!(claimed, (true, false, true));
    assert_eq!(queued_again, (true, true));
    assert_eq!(taken, (true, false));
    assert_eq!(reset, (false, false));
}

#[test]
async fn a_receiver_wakes_only_when_whether_a_terminal_request_waits_changes() {
    let interrupts = Interrupts::default();
    let terminal = interrupts.terminal();

    interrupts
        .try_lock()
        .unwrap()
        .queue(interrupt(InterruptKind::Restart));
    let after_restart = terminal.has_changed().unwrap();
    interrupts.take().await;
    interrupts
        .try_lock()
        .unwrap()
        .queue(interrupt(InterruptKind::Suspend(Timestamp::now_utc())));
    let after_suspend = terminal.has_changed().unwrap();

    assert_eq!((after_restart, after_suspend), (false, true));
}

#[test]
async fn a_queue_while_the_lock_is_held_gives_none_and_changes_nothing() {
    let interrupts = Interrupts::default();
    let held = interrupts.lock().await;

    let queued = interrupts
        .try_lock()
        .map(|mut state| state.queue(interrupt(InterruptKind::Interrupt(Timestamp::now_utc()))));
    drop(held);

    assert_eq!(queued, None);
    assert!(!interrupts.terminal_pending());
    assert!(!interrupts.has_interrupt().await);
}

#[test]
async fn a_rejected_request_keeps_the_terminal_projection() {
    let interrupts = Interrupts::default();
    let kind = InterruptKind::Suspend(Timestamp::now_utc());
    assert!(interrupts.lock().await.queue(interrupt(kind)));
    let terminal = interrupts.terminal();
    assert!(*terminal.borrow());
    assert!(
        !interrupts
            .lock()
            .await
            .queue(interrupt(InterruptKind::Restart))
    );
    assert!(*terminal.borrow());
    assert!(!terminal.has_changed().unwrap());
}
