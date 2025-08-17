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

use heck::ToPascalCase;
use proc_macro::TokenStream;
use proc_macro2::Ident;

use quote::quote;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, parse_quote, FnArg, ItemFn, Meta, PatType, ReturnType, Type};

pub fn golem_operation_impl(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args with Punctuated::<Meta, syn::Token![,]>::parse_terminated);

    let mut compensation = None;
    for arg in args {
        if let Meta::NameValue(name_value) = arg {
            let name = name_value.path.get_ident().unwrap().to_string();
            let value = name_value.value;

            if name == "compensation" {
                compensation = Some(value);
            }
        }
    }

    let ast: ItemFn = syn::parse(item).unwrap();
    let mut fnsig = ast.sig.clone();

    let (succ, err) = match fnsig.output {
        ReturnType::Type(_, ref typ) => result_type(typ),
        _ => panic!("Expected function to have a return type of Result<_, _>"),
    }
    .expect("Expected function to have a return type of Result<_, _>");

    let inputs: Vec<FnArg> = fnsig.inputs.iter().cloned().collect();
    let mut input_names = Vec::new();
    let mut input_types = Vec::new();
    for input in inputs.iter() {
        match input {
            FnArg::Typed(PatType { pat, ty, .. }) => {
                input_names.push(pat.clone());
                input_types.push(ty.clone());
            }
            FnArg::Receiver(_) => panic!("Expected function to have no self argument"),
        }
    }
    let input_pattern: proc_macro2::TokenStream = quote! {
        (#(#input_names),*): (#(#input_types),*)
    };
    let input_args: Vec<proc_macro2::TokenStream> =
        input_names.iter().map(|name| quote! { #name }).collect();

    let compensate = match &compensation {
        Some(_) => quote! { golem_rust::call_compensation_function },
        None => quote! {},
    };

    let compensation_pattern = quote! { #input_pattern, op_result: #succ };
    let compensation_args = input_args.clone();

    let operation = quote! { operation };

    fnsig.inputs.insert(
        0,
        parse_quote! {
            self
        },
    );

    match fnsig.output {
        ReturnType::Type(_, ref mut typ) => {
            *typ = parse_quote! { Result<#succ, #err> };
        }
        _ => panic!("Expected function to have a return type of Result<_, _>"),
    };

    let fnname = fnsig.ident.clone();
    let traitname = Ident::new(&fnname.to_string().to_pascal_case(), fnsig.ident.span());

    let result = quote! {
        #ast

        trait #traitname {
            #fnsig;
        }

        impl<T: golem_rust::Transaction<#err>> #traitname for &mut T {
            #fnsig {
                self.execute(
                    golem_rust::#operation(
                        |#input_pattern| {
                            #fnname(#(#input_args), *)
                        },
                        |#compensation_pattern| {
                            #compensate(#compensation, (op_result,), (#(#compensation_args), *,)).map_err(|err| err.0)
                        }
                    ),
                    (#(#input_args), *)
                )
            }
        }
    };

    result.into()
}

fn result_type(ty: &Type) -> Option<(Type, Type)> {
    match ty {
        Type::Group(group) => result_type(&group.elem),
        Type::Paren(paren) => result_type(&paren.elem),
        Type::Path(type_path) => {
            if type_path.qself.is_none() {
                let idents = type_path
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                if idents == vec!["Result"]
                    || idents == vec!["std", "result", "Result"]
                    || idents == vec!["core", "result", "Result"]
                {
                    let last_segment = type_path.path.segments.last().unwrap();
                    let syn::PathArguments::AngleBracketed(generics) = &last_segment.arguments
                    else {
                        return None;
                    };
                    if generics.args.len() != 2 {
                        return None;
                    }
                    let syn::GenericArgument::Type(success_type) = &generics.args[0] else {
                        return None;
                    };
                    let syn::GenericArgument::Type(err_type) = &generics.args[1] else {
                        return None;
                    };

                    Some((success_type.clone(), err_type.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}
