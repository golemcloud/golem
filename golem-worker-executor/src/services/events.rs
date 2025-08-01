// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_common::model::{IdempotencyKey, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm_rpc::ValueAndType;
use tokio::sync::broadcast::error::RecvError;

pub struct Events {
    sender: tokio::sync::broadcast::Sender<Event>,
    _receiver: tokio::sync::broadcast::Receiver<Event>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new(32768)
    }
}

impl Events {
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = tokio::sync::broadcast::channel(capacity);
        Self {
            sender,
            _receiver: receiver,
        }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> EventsSubscription {
        EventsSubscription {
            receiver: self.sender.subscribe(),
        }
    }
}

pub struct EventsSubscription {
    receiver: tokio::sync::broadcast::Receiver<Event>,
}

impl EventsSubscription {
    pub async fn wait_for<F, R>(&mut self, f: F) -> Result<R, RecvError>
    where
        F: Fn(&Event) -> Option<R>,
    {
        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if let Some(result) = f(&event) {
                        break Ok(result);
                    } else {
                        continue;
                    }
                }
                Err(err) => break Err(err),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    InvocationCompleted {
        worker_id: WorkerId,
        idempotency_key: IdempotencyKey,
        result: Result<Option<ValueAndType>, WorkerExecutorError>,
    },
    WorkerLoaded {
        worker_id: WorkerId,
        result: Result<(), WorkerExecutorError>,
    },
}
