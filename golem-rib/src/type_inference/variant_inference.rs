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

pub fn infer_variants(expr: &mut Expr, component_dependency: &ComponentDependencies) {
    let variants = internal::get_variants_info(expr, component_dependency);

    internal::convert_identifiers_to_no_arg_variant_calls(expr, &variants);

    // Initially every call type is dynamic-parsed function name
    internal::convert_function_calls_to_variant_calls(expr, &variants);
}

mod internal {
    use crate::call_type::CallType;
    use crate::{ComponentDependencies, Expr, InferredType};
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
        component_dependency: &ComponentDependencies,
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
                        let type_variants_opt = component_dependency
                            .function_dictionary()
                            .iter()
                            .find_map(|x| {
                                let result = x.get_variant_info(variable_id.name().as_str());

                                if result.is_empty() {
                                    None
                                } else {
                                    Some(result)
                                }
                            });

                        if let Some(type_variants) = type_variants_opt {
                            no_arg_variants.push(variable_id.name());

                            let inferred_types = type_variants
                                .iter()
                                .map(InferredType::from_type_variant)
                                .collect::<Vec<_>>();

                            let new_inferred_type = if inferred_types.len() == 1 {
                                inferred_types[0].clone()
                            } else {
                                InferredType::all_of(inferred_types)
                            };

                            *inferred_type = inferred_type.merge(new_inferred_type);
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
                        .find_map(|x| {
                            let type_variants =
                                x.get_variant_info(function_name.to_string().as_str());
                            if type_variants.is_empty() {
                                None
                            } else {
                                Some(type_variants)
                            }
                        });

                    if let Some(result) = result {
                        variant_with_args.push(function_name.to_string());

                        let inferred_types = result
                            .iter()
                            .map(InferredType::from_type_variant)
                            .collect::<Vec<_>>();

                        let new_inferred_type = if inferred_types.len() == 1 {
                            inferred_types[0].clone()
                        } else {
                            InferredType::all_of(inferred_types)
                        };

                        *inferred_type = inferred_type.merge(new_inferred_type);
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
