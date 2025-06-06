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

use crate::{ComponentDependency, Expr, FunctionTypeRegistry};

pub fn infer_variants(expr: &mut Expr, component_dependency: &ComponentDependency) {
    let variants = internal::get_variants_info(expr, component_dependency);

    internal::convert_identifiers_to_no_arg_variant_calls(expr, &variants);

    // Initially every call type is dynamic-parsed function name
    internal::convert_function_calls_to_variant_calls(expr, &variants);
}

mod internal {
    use crate::call_type::CallType;
    use crate::{
        ComponentDependency, Expr, FunctionTypeRegistry, InferredType, RegistryKey, RegistryValue,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use std::collections::VecDeque;

    pub(crate) fn convert_function_calls_to_variant_calls(
        expr: &mut Expr,
        variant_info: &VariantInfo,
    ) {
        let variants = variant_info.variants_with_args.clone();
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Call {
                    call_type: CallType::Function { function_name, .. },
                    args,
                    inferred_type,
                    ..
                } => {
                    if variants.contains(&function_name.to_string()) {
                        *expr = Expr::call(
                            CallType::VariantConstructor(function_name.to_string()),
                            None,
                            args.clone(),
                        )
                        .with_inferred_type(inferred_type.clone());
                    }
                }
                _ => expr.visit_expr_nodes_lazy(&mut queue),
            }
        }
    }

    pub(crate) fn convert_identifiers_to_no_arg_variant_calls(
        expr: &mut Expr,
        variant_info: &VariantInfo,
    ) {
        let variants = variant_info.no_arg_variants.clone();

        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier {
                    variable_id,
                    inferred_type,
                    ..
                } => {
                    if !variable_id.is_local() && variants.contains(&variable_id.name()) {
                        *expr = Expr::call(
                            CallType::VariantConstructor(variable_id.name()),
                            None,
                            vec![],
                        )
                        .with_inferred_type(inferred_type.clone());
                    }
                }
                _ => expr.visit_expr_nodes_lazy(&mut queue),
            }
        }
    }

    pub(crate) fn get_variants_info(
        expr: &mut Expr,
        component_dependency: &ComponentDependency,
    ) -> VariantInfo {
        let mut no_arg_variants = vec![];
        let mut variant_with_args = vec![];
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier {
                    variable_id,
                    inferred_type,
                    ..
                } => {
                    if !variable_id.is_local() {
                        let result = component_dependency
                            .function_dictionary()
                            .iter()
                            .find_map(|x| x.get_variant_info(variable_id.name().as_str()));

                        // Conflicts of having the same variant names across multiple components is not handled
                        if let Some(type_variant) = result {
                            no_arg_variants.push(variable_id.name());
                            *inferred_type =
                                inferred_type.merge(InferredType::from_type_variant(&type_variant));
                        }
                    }
                }

                Expr::Call {
                    call_type: CallType::Function { function_name, .. },
                    args,
                    inferred_type,
                    ..
                } => {
                    // Conflicts of having the same variant names across multiple components is not handled
                    let result = component_dependency
                        .function_dictionary()
                        .iter()
                        .find_map(|x| x.get_variant_info(function_name.to_string().as_str()));

                    if let Some(RegistryValue::Variant { variant_type, .. }) = result {
                        let variant_inferred_type = InferredType::from_type_variant(&variant_type);
                        *inferred_type = inferred_type.merge(variant_inferred_type);

                        variant_with_args.push(function_name.to_string());
                    }

                    for expr in args {
                        queue.push_back(expr);
                    }
                }

                _ => expr.visit_expr_nodes_lazy(&mut queue),
            }
        }

        VariantInfo {
            no_arg_variants,
            variants_with_args: variant_with_args,
        }
    }

    #[derive(Debug, Clone)]
    pub(crate) struct VariantInfo {
        no_arg_variants: Vec<String>,
        variants_with_args: Vec<String>,
    }
}
