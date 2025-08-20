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

use crate::call_type::CallType;
use crate::type_checker::missing_fields::find_missing_fields_in_record;
use crate::{type_checker, ComponentDependencies, FunctionName};
use crate::{Expr, ExprVisitor, FunctionCallError};
use golem_wasm_ast::analysis::AnalysedType;

// While we have a dedicated generic phases (refer submodules) within type_checker module,
// we have this special phase to grab errors in the context function calls.
// This is grab as many errors as possible.
// Refer `FunctionCallTypeCheckError`.
#[allow(clippy::result_large_err)]
pub fn check_invalid_function_args(
    expr: &mut Expr,
    component_dependency: &ComponentDependencies,
) -> Result<(), FunctionCallError> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::Call {
            call_type, args, ..
        } = &expr
        {
            match call_type {
                CallType::InstanceCreation(_) => {}
                call_type => get_missing_record_keys(call_type, args, component_dependency, expr)?,
            }
        }
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
fn get_missing_record_keys(
    call_type: &CallType,
    args: &[Expr],
    component_dependency: &ComponentDependencies,
    function_call_expr: &Expr,
) -> Result<(), FunctionCallError> {
    let function_name =
        FunctionName::from_call_type(call_type).ok_or(FunctionCallError::InvalidFunctionCall {
            function_name: call_type.to_string(),
            source_span: function_call_expr.source_span(),
            message: "invalid function call type".to_string(),
        })?;

    let (_, function_type) = component_dependency
        .get_function_type(&None, &function_name)
        .map_err(|err| FunctionCallError::InvalidFunctionCall {
            function_name: call_type.to_string(),
            source_span: function_call_expr.source_span(),
            message: err.to_string(),
        })?;

    let expected_arg_types = function_type.parameter_types;

    let mut filtered_expected_types = expected_arg_types
        .iter()
        .map(|x| AnalysedType::try_from(x).unwrap())
        .collect::<Vec<_>>();

    if call_type.is_resource_method() {
        filtered_expected_types.remove(0);
    }

    for (actual_arg, expected_arg_type) in args.iter().zip(filtered_expected_types) {
        // See if there are unresolved types in function arguments,
        // if so, tie them to the details specific to the function.
        // Finding resolved types can be called from anywhere, but this is called
        // within a function-call type-check phase,
        // to grab as many details as possible
        let unresolved_type = type_checker::check_unresolved_types(actual_arg);

        if let Err(unresolved_error) = unresolved_type {
            return Err(FunctionCallError::UnResolvedTypes {
                function_name: call_type.to_string(),
                source_span: actual_arg.source_span(),
                unresolved_error,
                expected_type: expected_arg_type.clone(),
            });
        }

        // Find possible missing fields in the arguments that are records
        let missing_fields = find_missing_fields_in_record(actual_arg, &expected_arg_type);

        if !missing_fields.is_empty() {
            return Err(FunctionCallError::MissingRecordFields {
                function_name: call_type.to_string(),
                argument_source_span: actual_arg.source_span(),
                missing_fields,
            });
        }
    }

    Ok(())
}
