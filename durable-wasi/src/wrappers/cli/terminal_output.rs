// Copyright 2024-2025 Golem Cloud
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

pub struct TerminalOutput {
    pub terminal_output: crate::bindings::wasi::cli::terminal_output::TerminalOutput,
}

impl crate::bindings::exports::wasi::cli::terminal_output::GuestTerminalOutput for TerminalOutput {
    // fn drop(&mut self, rep: Resource<TerminalOutput>) -> anyhow::Result<()> {
    //     self.observe_function_call("cli::terminal_output::terminal_output", "drop");
    //     HostTerminalOutput::drop(&mut self.as_wasi_view(), rep)
    // }
}

impl crate::bindings::exports::wasi::cli::terminal_output::Guest for crate::Component {
    type TerminalOutput = TerminalOutput;
}
