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

use crate::bindings::exports::wasi::logging::logging::Level;
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::bindings::wasi::logging::logging::log;
use std::mem::transmute;

impl crate::bindings::exports::wasi::logging::logging::Guest for crate::Component {
    fn log(level: Level, context: String, message: String) {
        observe_function_call("logging::handler", "log");
        let level = unsafe { transmute(level) };
        log(level, &context, &message)
    }
}
