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

use super::Worker;
use crate::workerctx::WorkerCtx;
use tokio::sync::oneshot;

/// Controls only the native TCP readiness phase after completed durable poll replay.
/// Dropping a control releases its gate so failed assertions cannot strand a Store.
pub struct P2ConnectReplayControlForTest {
    pub entered: oneshot::Receiver<()>,
    pub subscribe: oneshot::Sender<()>,
    pub waiting: oneshot::Receiver<()>,
    pub ready: oneshot::Sender<()>,
    pub selected: oneshot::Receiver<bool>,
    pub finish: oneshot::Sender<()>,
}

pub(crate) struct P2ConnectReplayGate {
    ready_probe: bool,
    pub entered: oneshot::Sender<()>,
    pub subscribe: oneshot::Receiver<()>,
    pub waiting: oneshot::Sender<()>,
    pub ready: oneshot::Receiver<()>,
    pub selected: oneshot::Sender<bool>,
    pub finish: oneshot::Receiver<()>,
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub fn pause_p2_connect_replay_for_test(
        &self,
        ready_probe: bool,
    ) -> P2ConnectReplayControlForTest {
        let (entered_tx, entered) = oneshot::channel();
        let (subscribe, subscribe_rx) = oneshot::channel();
        let (waiting_tx, waiting) = oneshot::channel();
        let (ready, ready_rx) = oneshot::channel();
        let (selected_tx, selected) = oneshot::channel();
        let (finish, finish_rx) = oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .p2_connect_replay_gate
                .replace(P2ConnectReplayGate {
                    ready_probe,
                    entered: entered_tx,
                    subscribe: subscribe_rx,
                    waiting: waiting_tx,
                    ready: ready_rx,
                    selected: selected_tx,
                    finish: finish_rx,
                })
                .is_none()
        );
        P2ConnectReplayControlForTest {
            entered,
            subscribe,
            waiting,
            ready,
            selected,
            finish,
        }
    }

    pub(crate) fn take_p2_connect_replay_gate_for_test(
        &self,
        ready_probe: bool,
    ) -> Option<P2ConnectReplayGate> {
        let mut progress = self.stop_progress.lock().unwrap();
        if progress
            .p2_connect_replay_gate
            .as_ref()
            .is_some_and(|gate| gate.ready_probe == ready_probe)
        {
            progress.p2_connect_replay_gate.take()
        } else {
            None
        }
    }
}
