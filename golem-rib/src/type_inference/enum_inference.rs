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

use crate::{ComponentDependencies, Expr};

pub fn infer_enums(expr: &mut Expr, component_dependencies: &ComponentDependencies) {
    let eum_info = internal::get_enum_info(expr, component_dependencies);

    internal::convert_identifiers_to_enum_function_calls(expr, &eum_info);
}

mod internal {
    use crate::call_type::CallType;
    use crate::{ComponentDependencies, Expr, ExprVisitor};
    use golem_wasm_ast::analysis::AnalysedType;

    pub(crate) fn convert_identifiers_to_enum_function_calls(
        expr: &mut Expr,
        enum_info: &EnumInfo,
    ) {
        let enum_cases = enum_info.clone();

        let mut visitor = ExprVisitor::bottom_up(expr);

        while let Some(expr) = visitor.pop_back() {
            if let Expr::Identifier {
                variable_id,
                inferred_type,
                source_span,
                type_annotation,
            } = expr
            {
                if enum_cases.cases.contains(&variable_id.name()) && !variable_id.is_local() {
                    *expr = Expr::Call {
                        call_type: CallType::EnumConstructor(variable_id.name()),
                        generic_type_parameter: None,
                        args: vec![],
                        inferred_type: inferred_type.clone(),
                        source_span: source_span.clone(),
                        type_annotation: type_annotation.clone(),
                    };
                }
            }
        }
    }

    pub(crate) fn get_enum_info(
        expr: &mut Expr,
        component_dependency: &ComponentDependencies,
    ) -> EnumInfo {
        let mut enum_cases = vec![];
        let mut visitor = ExprVisitor::bottom_up(expr);

        while let Some(expr) = visitor.pop_back() {
            if let Expr::Identifier {
                variable_id,
                inferred_type,
                ..
            } = expr
            {
                // If variable is local, it takes priority over being a global enum
                if !variable_id.is_local() {
                    // Retrieve the possible no-arg variant from the registry
                    let result = component_dependency
                        .function_dictionary()
                        .iter()
                        .find_map(|x| x.get_enum_info(variable_id.name().as_str()));

                    if let Some(typed_enum) = result {
                        enum_cases.push(variable_id.name());
                        *inferred_type =
                            inferred_type.merge((&AnalysedType::Enum(typed_enum.clone())).into());
                    }
                }
            }
        }

        EnumInfo { cases: enum_cases }
    }

    #[derive(Debug, Clone)]
    pub(crate) struct EnumInfo {
        cases: Vec<String>,
    }
}
