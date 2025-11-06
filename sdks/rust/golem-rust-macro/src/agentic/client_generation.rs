use crate::agentic::helpers::{
    get_input_param_type, get_output_param_type, InputParamType, OutputParamType,
};
use heck::ToKebabCase;
use quote::{format_ident, quote};
use syn::ItemTrait;

pub fn get_remote_client(
    item_trait: &ItemTrait,
    constructor_param_type: &InputParamType,
    constructor_param_defs: Vec<proc_macro2::TokenStream>,
    constructor_param_idents: Vec<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let remote_trait_name = format_ident!("{}Client", item_trait.ident);

    let type_name = item_trait.ident.to_string();
    let method_impls = get_remote_method_impls(item_trait, type_name.to_string());

    match constructor_param_type {
        InputParamType::Tuple => {
            let remote_client = quote! {
                pub struct #remote_trait_name {
                    agent_id: golem_rust::wasm_rpc::AgentId,
                }

                impl #remote_trait_name {
                    pub fn get(#(#constructor_param_defs), *) -> #remote_trait_name {
                        let agent_type =
                           golem_rust::golem_agentic::golem::agent::host::get_agent_type(#type_name).expect("Internal Error: Agent type not registered");

                         let element_values = vec![#(golem_rust::agentic::Schema::to_element_value(#constructor_param_idents).expect("Failed to convert constructor parameter to ElementValue")),*];

                         let data_value  = golem_rust::golem_agentic::golem::agent::common::DataValue::Tuple(element_values);

                         let agent_id_string =
                           golem_rust::golem_agentic::golem::agent::host::make_agent_id(#type_name, &data_value).expect("Internal Error: Failed to make agent id");

                         let agent_id = golem_rust::wasm_rpc::AgentId { agent_id: agent_id_string, component_id: agent_type.implemented_by.clone() };

                        #remote_trait_name { agent_id: agent_id }

                    }

                    pub fn get_id(&self) -> String {
                        self.agent_id.agent_id.clone()
                    }

                    #method_impls


                }

            };

            remote_client
        }
        InputParamType::Multimodal => {
            // TODO; Once multimodal representation is decided,
            // We can almost copy paste the above tokens expect for picking only 1 constructor parameter, and enumerate and get DataValue::Multimodal
            quote! {}
        }
    }
}

fn get_remote_method_impls(tr: &ItemTrait, agent_type_name: String) -> proc_macro2::TokenStream {
    let method_impls = tr.items.iter().filter_map(|item| {
        if let syn::TraitItem::Fn(method) = item {
            if let syn::ReturnType::Type(_, ty) = &method.sig.output {
                if let syn::Type::Path(type_path) = &**ty {
                    if type_path.path.segments.last().unwrap().ident == "Self" {
                        return None;
                    }
                }
            }

            let method_name = &method.sig.ident;

            let agent_type_name_kebab = agent_type_name.to_kebab_case();

            let method_name_str_kebab = method_name.to_string().to_kebab_case();

            // To form remote method name, we convert this back to kebab-case similar to TS
            let remote_method_name = format!(
                "{}.{{{}}}",
                agent_type_name_kebab,
                method_name_str_kebab
            );

            let remote_method_name_token = {
                quote! {
                   #remote_method_name
                }
            };

            let inputs: Vec<_> = method.sig.inputs.iter().collect();

            let input_idents: Vec<_> = method.sig
                .inputs
                .iter()
                .filter_map(|arg| {
                    if let syn::FnArg::Typed(pat_type) = arg {
                        if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                            Some(pat_ident.ident.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
            })
            .collect();


            let return_type = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => quote! { #ty },
                syn::ReturnType::Default => quote! { () },
            };

            // Depending on the input parameter type and output parameter type generate different implementations
            let input_parameter_type = get_input_param_type(&method.sig);

            let output_parameter_type = get_output_param_type(&method.sig);

            match (input_parameter_type, output_parameter_type) {
                (InputParamType::Tuple, OutputParamType::Tuple) =>
                    Some(quote!{
                        pub fn #method_name(#(#inputs),*) -> #return_type {
                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> =
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed")),*];

                          let wasm_rpc = golem_rust::wasm_rpc::WasmRpc::new(&self.agent_id);

                          // Change to async later
                          let rpc_result: Result<golem_rust::wasm_rpc::WitValue, golem_rust::wasm_rpc::RpcError> = wasm_rpc.invoke_and_await(
                              #remote_method_name_token,
                              &wit_values
                          );

                            let result = rpc_result.expect(format!("rpc call to {} failed", #remote_method_name_token).as_str());

                          let value = golem_rust::wasm_rpc::Value::from(result);

                          let value_unwrapped = match value {
                            golem_rust::wasm_rpc::Value::Tuple(values) => values[0].clone(), // Not sure how to go about this unwrapping and clone
                            v => v,
                          };

                          let wit_value = golem_rust::wasm_rpc::WitValue::from(value_unwrapped);

                          let element_value = golem_rust::golem_agentic::golem::agent::common::ElementValue::ComponentModel(wit_value);
                          let element_schema = <#return_type as golem_rust::agentic::Schema>::get_type();

                          <#return_type as golem_rust::agentic::Schema>::from_element_value(element_value, element_schema).expect("Failed to deserialize rpc result to return type")

                        }
                     }),
                  (InputParamType::Tuple, OutputParamType::Multimodal) => {
                    Some(quote!{
                        pub fn #method_name(#(#inputs),*) -> #return_type {

                          let wit_values: Vec<golem_rust::wasm_rpc::WitValue> =
                            vec![#(golem_rust::agentic::Schema::to_wit_value(#input_idents).expect("Failed to serialize")),*];

                          let wasm_rpc = golem_rust::wasm_rpc::WasmRpc::new(&self.agent_id);

                          // Change to async later
                          let rpc_result: Result<golem_rust::wasm_rpc::WitValue, golem_rust::wasm_rpc::RpcError> = wasm_rpc.invoke_and_await(
                              #remote_method_name_token,
                              &wit_values
                          );

                          let result = rpc_result.expect(format!("rpc call to {} failed", #remote_method_name_token).as_str());

                          let value = golem_rust::wasm_rpc::Value::from(result);

                          let value_unwrapped = match value {
                            golem_rust::wasm_rpc::Value::Tuple(values) => values[0].clone(), // Not sure how to go about this unwrapping and clone
                            v => v,
                          };

                          let wit_value = golem_rust::wasm_rpc::WitValue::from(value_unwrapped);

                          // TODO;
                          // If its multimodal, we cannot use Schema instance directly, we need to enumerate the values from multimodal
                          // and apply them separately.
                          todo!("Multimodal output parameter handling not implemented yet");
                        }
                     })
                  },
                _ => {
                    // TODO;
                    todo!("Remote method generation for multimodal input/output parameter types not implemented yet");
                }

            }
        } else {
            None
        }

    }).collect::<Vec<_>>();

    quote! {
        #(#method_impls)*
    }
}
