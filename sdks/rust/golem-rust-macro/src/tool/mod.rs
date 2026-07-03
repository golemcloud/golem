// Copyright 2024-2026 Golem Cloud
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

//! Tool authoring macros (`#[tool_definition]`, `#[tool_implementation]`,
//! `#[derive(ToolError)]`) and the attribute IR they parse into.

pub use definition::tool_definition_impl;
pub use implementation::tool_implementation_impl;
pub use tool_error::derive_tool_error_impl;

mod arg;
mod client;
mod command;
mod constraint;
mod definition;
mod descriptor;
mod doc;
mod helpers;
mod implementation;
pub mod ir;
mod result;
mod synthesis;
mod tool_error;
