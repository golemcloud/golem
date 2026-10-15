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

use super::{MonthlyProposalForTest, Worker};
use crate::workerctx::WorkerCtx;
use golem_common::model::IdempotencyKey;
use golem_common::model::oplog::OplogIndex;
use golem_service_base::error::worker_executor::InterruptKind;
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P2HttpPreSubscriptionOperationForTest {
    ResendResponseReady,
    PrefixSkipReady,
    BlockingSkip,
}

impl P2HttpPreSubscriptionOperationForTest {
    pub fn label(self) -> &'static str {
        match self {
            Self::ResendResponseReady => "http_body_resend_response_ready",
            Self::PrefixSkipReady => "http_body_prefix_skip_ready",
            Self::BlockingSkip => "http_body_blocking_skip",
        }
    }
}

#[derive(Debug)]
pub struct P2HttpPreSubscriptionEnteredForTest {
    pub invocation_key: IdempotencyKey,
    pub operation: P2HttpPreSubscriptionOperationForTest,
    pub stream_rep: u32,
    pub start_index: OplogIndex,
    pub runtime: MonthlyProposalForTest,
}

#[derive(Debug, PartialEq, Eq)]
pub enum P2HttpPreSubscriptionSelectionForTest {
    Native,
    Interrupt(InterruptKind),
}

pub struct P2HttpPreSubscriptionControlForTest {
    pub entered: oneshot::Receiver<P2HttpPreSubscriptionEnteredForTest>,
    pub selected: oneshot::Receiver<P2HttpPreSubscriptionSelectionForTest>,
    release: Option<oneshot::Sender<()>>,
}

impl P2HttpPreSubscriptionControlForTest {
    pub fn release(&mut self) {
        self.release.take();
    }
}

pub(crate) struct P2HttpPreSubscriptionGate {
    key: IdempotencyKey,
    operation: P2HttpPreSubscriptionOperationForTest,
    entered: oneshot::Sender<P2HttpPreSubscriptionEnteredForTest>,
    release: oneshot::Receiver<()>,
    selected: oneshot::Sender<P2HttpPreSubscriptionSelectionForTest>,
}

pub(crate) struct P2HttpPreSubscriptionSelection {
    selected: oneshot::Sender<P2HttpPreSubscriptionSelectionForTest>,
}

impl P2HttpPreSubscriptionSelection {
    pub fn report(self, selection: P2HttpPreSubscriptionSelectionForTest) {
        let _ = self.selected.send(selection);
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub fn pause_next_p2_http_pre_subscription_for_test(
        &self,
        key: IdempotencyKey,
        operation: P2HttpPreSubscriptionOperationForTest,
    ) -> P2HttpPreSubscriptionControlForTest {
        let (entered_tx, entered) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let (selected_tx, selected) = oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .p2_http_pre_subscription_gate
                .replace(P2HttpPreSubscriptionGate {
                    key,
                    operation,
                    entered: entered_tx,
                    release: release_rx,
                    selected: selected_tx,
                })
                .is_none()
        );
        P2HttpPreSubscriptionControlForTest {
            entered,
            selected,
            release: Some(release),
        }
    }

    pub(crate) async fn wait_p2_http_pre_subscription_for_test(
        &self,
        key: Option<&IdempotencyKey>,
        operation: P2HttpPreSubscriptionOperationForTest,
        stream_rep: u32,
        start_index: OplogIndex,
    ) -> Option<P2HttpPreSubscriptionSelection> {
        let gate = {
            let mut progress = self.stop_progress.lock().unwrap();
            let matches = progress
                .p2_http_pre_subscription_gate
                .as_ref()
                .is_some_and(|gate| Some(&gate.key) == key && gate.operation == operation);
            matches.then(|| progress.p2_http_pre_subscription_gate.take().unwrap())
        };
        let gate = gate?;
        let runtime = self.current_monthly_proposal_for_test();
        let _ = gate.entered.send(P2HttpPreSubscriptionEnteredForTest {
            invocation_key: gate.key,
            operation,
            stream_rep,
            start_index,
            runtime,
        });
        let _ = gate.release.await;
        Some(P2HttpPreSubscriptionSelection {
            selected: gate.selected,
        })
    }
}
