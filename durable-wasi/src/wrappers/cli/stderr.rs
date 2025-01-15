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

use crate::bindings::exports::wasi::cli::stderr::OutputStream;
use crate::bindings::golem::api::durability::observe_function_call;
use crate::bindings::wasi::cli::stderr::get_stderr;

impl crate::bindings::exports::wasi::cli::stderr::Guest for crate::Component {
    fn get_stderr() -> OutputStream {
        observe_function_call("cli::stderr", "get_stderr");
        get_stderr()
    }
}
