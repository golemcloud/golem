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
mod layout;
mod nested_cli;
#[cfg(feature = "tui-preview")]
mod preview;
mod terminal;
pub(crate) mod terminal_screen;
mod visual;

use crate::context::Context;
use crate::model::environment::EnvironmentReference;
use crate::model::worker::AgentsMetadataResponseView;
use context_executor::TuiContextTaskResult;
use crossterm::event::Event;
use golem_client::model::EnvironmentWithDetails;
use nested_cli::CommandExit;
use std::sync::Arc;
use tokio::sync::oneshot;

pub use app::run;

#[cfg(feature = "tui-preview")]
#[doc(hidden)]
pub use preview::main as preview_main;

enum TuiEvent {
    Terminal(Event),
    CommandOutput(Vec<u8>),
    CommandOutputClosed(Option<String>),
    CommandExited(CommandExit),
    SpinnerTick(u64),
    ServerOutput(Vec<u8>),
    ServerOutputClosed(Option<String>),
    ServerExited(CommandExit),
    ServerSpinnerTick(u64),
    ReplOutput(Vec<u8>),
    ReplOutputClosed(Option<String>),
    ReplExited(CommandExit),
    AgentOplogOutput(Vec<u8>),
    AgentOplogOutputClosed(Option<String>),
    AgentOplogExited(CommandExit),
    AgentStreamOutput(Vec<u8>),
    AgentStreamOutputClosed(Option<String>),
    AgentStreamExited(CommandExit),
    AgentRefreshTick,
    AgentRefreshFinished {
        generation: u64,
        result: TuiContextTaskResult<AgentsMetadataResponseView>,
    },
    ContextSwitchFinished {
        generation: u64,
        result: TuiContextTaskResult<(Arc<Context>, Option<EnvironmentReference>)>,
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
