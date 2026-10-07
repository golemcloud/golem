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

mod app;
mod context_executor;
mod input;
mod interaction;
mod layout;
mod navigation;
mod nested_cli;
mod ops;
#[cfg(feature = "tui-preview")]
mod preview;
mod terminal;
pub(crate) mod terminal_screen;
mod visual;
mod widgets;

use app::{AgentRefreshPayload, ContextSwitchResult, OpsEnvironmentSnapshot};
use context_executor::TuiContextTaskResult;
use crossterm::event::Event;
use golem_client::model::EnvironmentWithDetails;
use nested_cli::{NestedCliEvent, NestedCliTarget};
use ops::activity::ActivityRefreshPayload;
use ops::metrics::MetricsRefreshPayload;
use ops::resources::ResourcesRefreshPayload;
use tokio::sync::oneshot;

pub use app::run;

#[cfg(feature = "tui-preview")]
#[doc(hidden)]
pub use preview::main as preview_main;

enum TuiEvent {
    Terminal(Event),
    NestedCli {
        target: NestedCliTarget,
        event: NestedCliEvent,
    },
    SpinnerTick(u64),
    ServerSpinnerTick(u64),
    AgentRefreshTick,
    AgentRefreshSpinnerTick {
        generation: u64,
    },
    AgentRefreshFinished {
        generation: u64,
        append: bool,
        result: TuiContextTaskResult<AgentRefreshPayload>,
    },
    OpsEnvironmentRefreshFinished {
        generation: u64,
        result: TuiContextTaskResult<OpsEnvironmentSnapshot>,
    },
    MetricsRefreshTick,
    MetricsRefreshFinished {
        generation: u64,
        result: TuiContextTaskResult<MetricsRefreshPayload>,
    },
    ActivityRefreshFinished {
        generation: u64,
        result: TuiContextTaskResult<ActivityRefreshPayload>,
    },
    ResourcesRefreshFinished {
        generation: u64,
        result: TuiContextTaskResult<ResourcesRefreshPayload>,
    },
    ContextSwitchFinished {
        generation: u64,
        result: TuiContextTaskResult<ContextSwitchResult>,
    },
    ContextEnvironmentListFinished {
        generation: u64,
        server_key: String,
        result: TuiContextTaskResult<Vec<EnvironmentWithDetails>>,
    },
    ContextEnvironmentListTick {
        generation: u64,
    },
    AuthPromptStarted {
        url: String,
        ready: oneshot::Sender<()>,
    },
    AuthPromptFinished,
}
