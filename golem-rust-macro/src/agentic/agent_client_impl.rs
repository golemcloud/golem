// Copyright 2024-2026 Golem Cloud
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

use heck::ToUpperCamelCase;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;
use syn::parse::{Parse, ParseStream};
use syn::{FnArg, Ident, ItemTrait, LitStr, Pat, ReturnType, Token, TraitItem, Type};

struct AgentClientArgs {
    type_name: Option<LitStr>,
}

impl Parse for AgentClientArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self { type_name: None });
        }

        let name: Ident = input.parse()?;
        if name != "type_name" {
            return Err(syn::Error::new(
                name.span(),
                "expected `type_name = \"...\"`",
            ));
        }
        input.parse::<Token![=]>()?;
        let type_name = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("unexpected agent_client attribute argument"));
        }
        Ok(Self {
            type_name: Some(type_name),
        })
    }
}

pub fn agent_client_impl(attr: TokenStream, item: TokenStream, golem_rust: &Ident) -> TokenStream {
    match expand(attr, item, golem_rust) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(
    attr: TokenStream,
    item: TokenStream,
    golem_rust: &Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    let args = syn::parse::<AgentClientArgs>(attr)?;
    let item_trait = syn::parse::<ItemTrait>(item)?;
    if !item_trait.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_trait.generics,
            "agent_client traits cannot be generic",
        ));
    }

    let trait_ident = &item_trait.ident;
    let client_ident = format_ident!("{}Client", trait_ident);
    let visibility = &item_trait.vis;
    let remote_type_name = args
        .type_name
        .unwrap_or_else(|| LitStr::new(&trait_ident.to_string(), trait_ident.span()));
    let method_names = item_trait
        .items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(method) => Some(method.sig.ident.to_string()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    for reserved in ["client_definition", "for_agent_id", "typed_client"] {
        if method_names.contains(reserved) {
            return Err(syn::Error::new_spanned(
                &item_trait.ident,
                format!("agent_client method name `{reserved}` is reserved"),
            ));
        }
    }

    let mut input_structs = Vec::new();
    let mut definition_steps = Vec::new();
    let mut client_methods = Vec::new();

    for item in &item_trait.items {
        let method = match item {
            TraitItem::Fn(method) => method,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "agent_client traits may only contain methods",
                ));
            }
        };
        if method.default.is_some()
            || method.sig.asyncness.is_some()
            || method.sig.constness.is_some()
            || method.sig.unsafety.is_some()
            || method.sig.abi.is_some()
            || !method.sig.generics.params.is_empty()
        {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "agent_client methods must be non-generic declarations of the form `fn method(&self, ...) -> ...;`",
            ));
        }

        let mut inputs = method.sig.inputs.iter();
        let receiver = inputs.next().ok_or_else(|| {
            syn::Error::new_spanned(&method.sig, "agent_client methods require `&self`")
        })?;
        if !matches!(receiver, FnArg::Receiver(receiver) if receiver.reference.is_some() && receiver.mutability.is_none())
        {
            return Err(syn::Error::new_spanned(
                receiver,
                "agent_client methods require an immutable `&self` receiver",
            ));
        }

        let mut params = Vec::new();
        for input in inputs {
            let FnArg::Typed(input) = input else {
                return Err(syn::Error::new_spanned(input, "unexpected receiver"));
            };
            let Pat::Ident(pattern) = input.pat.as_ref() else {
                return Err(syn::Error::new_spanned(
                    &input.pat,
                    "agent_client parameter patterns must be identifiers",
                ));
            };
            params.push((pattern.ident.clone(), input.ty.as_ref().clone()));
        }

        let method_ident = &method.sig.ident;
        let method_name = LitStr::new(&method_ident.to_string(), method_ident.span());
        let input_ident = format_ident!(
            "__{}{}Input",
            trait_ident,
            method_ident.to_string().to_upper_camel_case()
        );
        let field_idents = params.iter().map(|(ident, _)| ident).collect::<Vec<_>>();
        let field_types = params.iter().map(|(_, ty)| ty).collect::<Vec<_>>();
        input_structs.push(quote! {
            #[doc(hidden)]
            #[derive(#golem_rust::IntoSchema)]
            struct #input_ident {
                #(#field_idents: #field_types),*
            }
        });

        let (output_type, is_unit) = match &method.sig.output {
            ReturnType::Default => (syn::parse_quote!(()), true),
            ReturnType::Type(_, ty) => {
                let is_unit = matches!(ty.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty());
                (ty.as_ref().clone(), is_unit)
            }
        };
        if is_unit {
            definition_steps.push(quote! {
                let builder = builder.unit_method::<#input_ident>(#method_name)?;
            });
        } else {
            definition_steps.push(quote! {
                let builder = builder.method::<#input_ident, #output_type>(#method_name)?;
            });
        }

        let trigger_ident = format_ident!("trigger_{}", method_ident);
        let pending_ident = format_ident!("pending_{}", method_ident);
        let schedule_ident = format_ident!("schedule_{}", method_ident);
        for generated in [&trigger_ident, &pending_ident, &schedule_ident] {
            if method_names.contains(&generated.to_string()) {
                return Err(syn::Error::new_spanned(
                    method_ident,
                    format!("generated method `{generated}` conflicts with an interface method"),
                ));
            }
        }
        let params = params
            .iter()
            .map(|(ident, ty)| quote!(#ident: #ty))
            .collect::<Vec<_>>();
        let input = quote!(#input_ident { #(#field_idents),* });
        let awaited_value = if is_unit {
            quote! {
                if invocation.value.is_some() {
                    return Err(#golem_rust::GolemReflectError::InvalidType(
                        format!("method `{}` returned a value instead of unit", #method_name),
                    ));
                }
                #golem_rust::Invocation { metadata: invocation.metadata, value: () }
            }
        } else {
            quote! {
                let value = invocation.value.ok_or_else(|| #golem_rust::GolemReflectError::InvalidType(
                    format!("method `{}` returned unit instead of a value", #method_name),
                ))?;
                #golem_rust::Invocation { metadata: invocation.metadata, value }
            }
        };
        client_methods.push(quote! {
            pub async fn #method_ident(&self, #(#params),*)
                -> Result<#golem_rust::Invocation<#output_type>, #golem_rust::GolemReflectError>
            {
                let input = #input;
                let invocation = self.inner
                    .method::<#input_ident, #output_type>(#method_name)?
                    .invoke(&input)
                    .await?;
                Ok({ #awaited_value })
            }

            pub fn #trigger_ident(&self, #(#params),*)
                -> Result<#golem_rust::InvocationMetadata, #golem_rust::GolemReflectError>
            {
                let input = #input;
                self.inner.method::<#input_ident, #output_type>(#method_name)?.trigger(&input)
            }

            pub fn #pending_ident(&self, #(#params),*)
                -> Result<#golem_rust::TypedPendingInvocation<#output_type>, #golem_rust::GolemReflectError>
            {
                let input = #input;
                self.inner.method::<#input_ident, #output_type>(#method_name)?.pending(&input)
            }

            pub fn #schedule_ident(&self, at: #golem_rust::ScheduledTime, #(#params),*)
                -> Result<#golem_rust::ScheduledInvocation, #golem_rust::GolemReflectError>
            {
                let input = #input;
                self.inner.method::<#input_ident, #output_type>(#method_name)?.schedule(at, &input)
            }
        });
    }

    Ok(quote! {
        #item_trait

        #(#input_structs)*

        #[derive(Clone)]
        #visibility struct #client_ident {
            inner: #golem_rust::TypedAgentClient,
        }

        impl #client_ident {
            pub fn client_definition()
                -> Result<#golem_rust::AgentClientDefinition, #golem_rust::GolemReflectError>
            {
                let builder = #golem_rust::AgentClientDefinition::builder()
                    .type_name(#remote_type_name);
                #(#definition_steps)*
                Ok(builder.build())
            }

            pub fn for_agent_id(agent_id: &#golem_rust::AgentId)
                -> Result<Self, #golem_rust::GolemReflectError>
            {
                Ok(Self { inner: Self::client_definition()?.bind(agent_id)? })
            }

            pub fn typed_client(&self) -> &#golem_rust::TypedAgentClient {
                &self.inner
            }

            #(#client_methods)*
        }
    })
}
