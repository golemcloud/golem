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

use crate::{Expr, FunctionTypeRegistry};

pub fn infer_enums(expr: &mut Expr, function_type_registry: &FunctionTypeRegistry) {
    let eum_info = internal::get_enum_info(expr, function_type_registry);

    internal::convert_identifiers_to_enum_function_calls(expr, &eum_info);
}

mod internal {
    use crate::call_type::CallType;
    use crate::{Expr, FunctionTypeRegistry, RegistryKey, RegistryValue};
    use golem_wasm_ast::analysis::AnalysedType;
    use std::collections::VecDeque;

    pub(crate) fn convert_identifiers_to_enum_function_calls(
        expr: &mut Expr,
        enum_info: &EnumInfo,
    ) {
        let enum_cases = enum_info.clone();

        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    if enum_cases.cases.contains(&variable_id.name()) {
                        *expr = Expr::Call(
                            CallType::EnumConstructor(variable_id.name()),
                            vec![],
                            inferred_type.clone(),
                        );
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }
    }

    pub(crate) fn get_enum_info(
        expr: &mut Expr,
        function_type_registry: &FunctionTypeRegistry,
    ) -> EnumInfo {
        let mut enum_cases = vec![];
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    // Retrieve the possible no-arg variant from the registry
                    let key = RegistryKey::FunctionName(variable_id.name().clone());
                    if let Some(RegistryValue::Value(AnalysedType::Enum(typed_enum))) =
                        function_type_registry.types.get(&key)
                    {
                        enum_cases.push(variable_id.name());
                        *inferred_type = inferred_type
                            .merge(AnalysedType::Enum(typed_enum.clone()).clone().into());
                    }
                }

                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }

        EnumInfo { cases: enum_cases }
    }

    #[derive(Debug, Clone)]
    pub(crate) struct EnumInfo {
        cases: Vec<String>,
    }
}
