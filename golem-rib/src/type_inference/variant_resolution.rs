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

use crate::type_inference::variant_resolution::internal::get_variants_info;
use crate::{Expr, FunctionTypeRegistry};

pub fn infer_variants(expr: &mut Expr, function_type_registry: &FunctionTypeRegistry) {
    let variants = get_variants_info(expr, function_type_registry);

    internal::convert_identifiers_to_no_arg_variant_calls(expr, &variants);
    internal::convert_function_calls_to_variant_calls(expr, &variants);
}

mod internal {
    use crate::call_type::CallType;
    use crate::{Expr, FunctionTypeRegistry, RegistryKey, RegistryValue};
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
                Expr::Call(CallType::Function(parsed_function_name), args, inferred_type) => {
                    if variants.contains(&parsed_function_name.to_string()) {
                        *expr = Expr::Call(
                            CallType::VariantConstructor(parsed_function_name.to_string()),
                            args.clone(),
                            inferred_type.clone(),
                        );
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
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
                Expr::Identifier(variable_id, inferred_type) => {
                    if variants.contains(&variable_id.name()) {
                        *expr = Expr::Call(
                            CallType::VariantConstructor(variable_id.name()),
                            vec![],
                            inferred_type.clone(),
                        );
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }
    }

    pub(crate) fn get_variants_info(
        expr: &mut Expr,
        function_type_registry: &FunctionTypeRegistry,
    ) -> VariantInfo {
        let mut no_arg_variants = vec![];
        let mut variant_with_args = vec![];
        let mut queue = VecDeque::new();
        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    // Retrieve the possible no-arg variant from the registry
                    let key = RegistryKey::VariantName(variable_id.name().clone());
                    if let Some(RegistryValue::Value(analysed_type)) =
                        function_type_registry.types.get(&key)
                    {
                        no_arg_variants.push(variable_id.name());
                        *inferred_type = inferred_type.merge(analysed_type.clone().into());
                    }
                }

                Expr::Call(CallType::Function(parsed_function_name), exprs, inferred_type) => {
                    let key = RegistryKey::VariantName(parsed_function_name.to_string());
                    if let Some(RegistryValue::Function { return_types, .. }) =
                        function_type_registry.types.get(&key)
                    {
                        variant_with_args.push(parsed_function_name.to_string());

                        // TODO; return type is only 1 in reality for variants - we can make this typed
                        if let Some(variant_type) = return_types.first() {
                            *inferred_type = inferred_type.merge(variant_type.clone().into());
                        }
                    }

                    for expr in exprs {
                        queue.push_back(expr);
                    }
                }

                _ => expr.visit_children_mut_bottom_up(&mut queue),
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
