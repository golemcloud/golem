use crate::type_inference::variant_resolution::internal::get_variants_info;
use crate::{Expr, FunctionTypeRegistry};

pub fn infer_variants(expr: &mut Expr, function_type_registry: &FunctionTypeRegistry) {
    let variants = get_variants_info(expr, function_type_registry);

    internal::convert_identifiers_to_no_arg_variant_calls(expr, &variants);
    internal::convert_function_calls_to_variant_calls(expr, &variants);
}

mod internal {
    use crate::{Expr, FunctionTypeRegistry, InvocationName, RegistryKey, RegistryValue};
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
                Expr::Call(InvocationName::Function(parsed_function_name), args, inferred_type) => {
                    if variants.contains(&parsed_function_name.to_string()) {
                        *expr = Expr::Call(
                            InvocationName::VariantConstructor(parsed_function_name.to_string()),
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
                            InvocationName::VariantConstructor(variable_id.name()),
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
                        inferred_type.update(analysed_type.clone().into());
                    }
                }

                Expr::Call(
                    InvocationName::Function(parsed_function_name),
                    exprs,
                    inferred_type,
                ) => {
                    let key = RegistryKey::VariantName(parsed_function_name.to_string());
                    if let Some(RegistryValue::Function { return_types, .. }) =
                        function_type_registry.types.get(&key)
                    {
                        variant_with_args.push(parsed_function_name.to_string());

                        // TODO; return type is only 1 in reality for variants - we can make this typed
                        if let Some(variant_type) = return_types.first() {
                            inferred_type.update(variant_type.clone().into());
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
