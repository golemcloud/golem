// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

#![allow(clippy::large_enum_variant)]

pub use command::*;
pub use dependency_manager::*;
pub use invoke::*;
pub use raw::*;
pub use repl_bootstrap_error::*;
pub use repl_printer::*;
pub use rib_context::*;
pub use rib_execution_error::*;
pub use rib_repl::*;

mod command;
mod compiler;
mod dependency_manager;
mod eval;
mod invoke;
mod raw;
mod repl_bootstrap_error;
mod repl_printer;
mod repl_state;
mod rib_context;
mod rib_edit;
mod rib_execution_error;
mod rib_repl;
mod value_generator;

#[cfg(test)]
test_r::enable!();
