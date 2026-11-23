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

use tokio::signal::unix::{SignalKind, signal};

/// Resolves on SIGTERM or SIGINT, which is how an orchestrator or a terminal stops this process.
pub(super) async fn termination_signal() {
    let mut terminate =
        signal(SignalKind::terminate()).expect("failed to install the SIGTERM handler");
    let mut interrupt =
        signal(SignalKind::interrupt()).expect("failed to install the SIGINT handler");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}
