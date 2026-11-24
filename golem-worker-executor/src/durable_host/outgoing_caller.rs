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

//! The caller of every call that an agent or tool makes: the agents it calls, the agents it
//! creates, schedules and forks, and the tools it calls.

use golem_common::model::caller::{Caller, Principal, ToolPrincipal};

/// How the code of one Store appears to the agents and tools that it calls.
#[derive(Clone, Debug)]
pub(crate) enum CallingCode {
    /// The agent's own Store: the agent.
    Agent(Principal),
    /// A tool body, or a middleware layer of a call to this tool: the tool.
    Tool(ToolPrincipal),
    /// The built-in shell. A command that it runs is a call of the agent or tool that calls or
    /// hosts it. `host` is the hosting agent, or `None` for a standalone tool invocation.
    Shell { host: Option<Principal> },
}

/// The caller of a call that the code of a Store makes inside an invocation with the caller
/// `invocation`: the same original caller and tokens, and the calling agent or tool as the
/// immediate caller. The shell never appears as a caller: its commands get the caller of the
/// agent or tool that called it, or, when an entry point called it on an agent, that agent as
/// the immediate caller.
pub(crate) fn outgoing_caller(invocation: &Caller, code: &CallingCode) -> Caller {
    match code {
        CallingCode::Agent(agent) => invocation.propagate(agent.clone()),
        CallingCode::Tool(tool) => invocation.propagate(Principal::Tool(tool.clone())),
        CallingCode::Shell { host } => match (&invocation.immediate, host) {
            // An agent or a tool called the shell: the command is its call.
            (Principal::Agent(_) | Principal::Tool(_), _) => invocation.clone(),
            // An entry point started the shell on an agent: the command is the agent's call.
            (_, Some(host)) => invocation.propagate(host.clone()),
            // A standalone shell hosts no agent, so the entry caller stays.
            (_, None) => invocation.clone(),
        },
    }
}

#[cfg(test)]
mod tests;
