// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::error::GolemError;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_wasm_rpc::Value;

pub struct Events {
    sender: tokio::sync::broadcast::Sender<Event>,
    _receiver: tokio::sync::broadcast::Receiver<Event>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

impl Events {
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::broadcast::channel(100);
        Self {
            sender,
            _receiver: receiver,
        }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    pub async fn wait_for<F, R>(&self, f: F) -> R
    where
        F: Fn(&Event) -> Option<R>,
    {
        let mut receiver = self.sender.subscribe();
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Some(result) = f(&event) {
                        break result;
                    } else {
                        continue;
                    }
                }
                Err(_) => continue,
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    InvocationCompleted {
        worker_id: WorkerId,
        idempotency_key: IdempotencyKey,
        result: Result<Vec<Value>, GolemError>,
    },
}
