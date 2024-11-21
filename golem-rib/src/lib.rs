// Copyright 2024 Golem Cloud
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

#[cfg(feature = "full")]
pub use compiler::*;

#[cfg(feature = "full")]
pub use expr::*;

#[cfg(feature = "full")]
pub use function_name::*;

#[cfg(feature = "full")]
pub use inferred_type::*;

#[cfg(feature = "full")]
pub use interpreter::*;

#[cfg(feature = "full")]
pub use parser::type_name::TypeName;

#[cfg(feature = "full")]
pub use text::*;

#[cfg(feature = "full")]
pub use type_inference::*;

#[cfg(feature = "full")]
pub use type_registry::*;

#[cfg(feature = "full")]
pub use variable_id::*;

#[cfg(feature = "full")]
mod call_type;
#[cfg(feature = "full")]
mod compiler;
#[cfg(feature = "full")]
mod expr;
#[cfg(feature = "full")]
mod function_name;
#[cfg(feature = "full")]
mod inferred_type;
#[cfg(feature = "full")]
mod interpreter;
#[cfg(feature = "full")]
mod parser;
#[cfg(feature = "full")]
mod text;
#[cfg(feature = "full")]
mod type_checker;
#[cfg(feature = "full")]
mod type_inference;
#[cfg(feature = "full")]
mod type_refinement;
#[cfg(feature = "full")]
mod type_registry;
#[cfg(feature = "full")]
mod variable_id;

#[cfg(test)]
test_r::enable!();
