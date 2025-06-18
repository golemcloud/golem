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

pub(crate) use exhaustive_pattern_match::*;
pub(crate) use unresolved_types::*;

pub use path::*;
mod exhaustive_pattern_match;
mod invalid_function_args;
mod invalid_function_calls;
mod invalid_worker_name;
mod missing_fields;
mod path;
mod unresolved_types;

use crate::rib_type_error::RibTypeErrorInternal;
use crate::type_checker::exhaustive_pattern_match::check_exhaustive_pattern_match;
use crate::type_checker::invalid_function_args::check_invalid_function_args;
use crate::type_checker::invalid_function_calls::check_invalid_function_calls;
use crate::type_checker::invalid_worker_name::check_invalid_worker_name;
use crate::{ComponentDependencies, Expr};

pub fn type_check(
    expr: &mut Expr,
    component_dependency: &ComponentDependencies,
) -> Result<(), RibTypeErrorInternal> {
    check_invalid_function_args(expr, component_dependency)?;
    check_unresolved_types(expr)?;
    check_invalid_worker_name(expr)?;
    check_exhaustive_pattern_match(expr, component_dependency)?;
    check_invalid_function_calls(expr)?;
    Ok(())
}
