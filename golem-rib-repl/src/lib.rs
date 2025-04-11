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

pub use dependency_manager::*;
pub use invoke::*;
pub use repl_printer::*;
pub use rib_repl::*;

mod compiler;
mod dependency_manager;
mod invoke;
mod repl_printer;
mod repl_state;
mod rib_edit;
mod rib_repl;
mod value_generator;

#[cfg(test)]
test_r::enable!();
