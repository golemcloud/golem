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

use crate::bindings::exports::wasi::cli::terminal_stdin::TerminalInput;
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::bindings::wasi::cli::terminal_stdin::get_terminal_stdin;

impl crate::bindings::exports::wasi::cli::terminal_stdin::Guest for crate::Component {
    fn get_terminal_stdin() -> Option<TerminalInput> {
        observe_function_call("cli::terminal_stdin", "get_terminal_stdin");
        get_terminal_stdin().map(|ti| {
            let wrapped =
                crate::wrappers::cli::terminal_input::WrappedTerminalInput { terminal_input: ti };
            TerminalInput::new(wrapped)
        })
    }
}
