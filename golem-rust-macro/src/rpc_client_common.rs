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

//! Generator-neutral primitives shared by the agent remote-client generator
//! and the tool client generator.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{FnArg, ReturnType, Type};

// =====================================================================
// Signature helpers
// =====================================================================

/// Whether a method's result is produced asynchronously.
pub enum Asyncness {
    Future,
    Immediate,
}

pub fn get_asyncness(sig: &syn::Signature) -> Asyncness {
    if sig.asyncness.is_some() {
        Asyncness::Future
    } else {
        Asyncness::Immediate
    }
}

/// Output classification of a method signature: whether it is async and
/// whether its declared return type is unit.
pub struct FunctionOutputInfo {
    pub async_ness: Asyncness,
    pub is_unit: bool,
}

impl FunctionOutputInfo {
    pub fn from_signature(sig: &syn::Signature) -> FunctionOutputInfo {
        let function_kind = get_asyncness(sig);

        let is_unit = match &sig.output {
            ReturnType::Type(_, ty) => match &**ty {
                Type::Tuple(tuple) => tuple.elems.is_empty(),
                _ => false,
            },
            _ => true,
        };

        FunctionOutputInfo {
            async_ness: function_kind,
            is_unit,
        }
    }
}

/// A method is static when it has no `self` receiver.
pub fn is_static_method(sig: &syn::Signature) -> bool {
    sig.receiver().is_none()
}

/// Whether a typed parameter's type is the auto-injected `Principal`.
pub fn is_principal_param(pat_type: &syn::PatType) -> bool {
    matches!(
        &*pat_type.ty,
        Type::Path(type_path)
            if type_path.path.segments.last().map(|s| s.ident == "Principal").unwrap_or(false)
    )
}

// =====================================================================
// Generic-parameter validation
// =====================================================================

/// A use of one of the trait's generic type parameters in a position where the
/// client generator requires a concrete type.
pub struct GenericParamUse {
    pub span: Span,
    pub type_name: String,
}

/// Finds the first generic type parameter used in the method's return type.
///
/// A `Self` return type is allowed (constructors) and skipped.
pub fn find_generic_param_in_return(
    sig: &syn::Signature,
    type_params: &[String],
) -> Option<GenericParamUse> {
    if let ReturnType::Type(_, ty) = &sig.output
        && let Type::Path(path) = &**ty
    {
        let ident = &path.path.segments.last().unwrap().ident;

        if ident == "Self" {
            return None;
        }

        if type_params.contains(&ident.to_string()) {
            return Some(GenericParamUse {
                span: sig.ident.span(),
                type_name: ident.to_string(),
            });
        }
    }
    None
}

/// Finds the first generic type parameter used in a method parameter type.
pub fn find_generic_param_in_inputs(
    sig: &syn::Signature,
    type_params: &[String],
) -> Option<GenericParamUse> {
    for fn_arg in &sig.inputs {
        if let FnArg::Typed(pat_type) = fn_arg
            && let Type::Path(type_path) = &*pat_type.ty
        {
            let type_name = type_path.path.segments.last().unwrap().ident.to_string();
            if type_params.contains(&type_name) {
                return Some(GenericParamUse {
                    span: pat_type.ty.span(),
                    type_name,
                });
            }
        }
    }
    None
}

// =====================================================================
// Parameter collection
// =====================================================================

/// A method parameter reduced to the identifier and type the encoder needs.
pub struct TypedParam {
    pub ident: syn::Ident,
    /// The parameter's declared type, used to build the input schema graph for
    /// carriers that travel with their own schema. The value-only carrier
    /// infers it from the moved argument and does not read this field.
    #[allow(dead_code)]
    pub ty: syn::Type,
}

/// Collects the method's value-carrying parameters (identifier + type) in
/// declaration order, keeping only those for which `keep` returns `true`.
///
/// The receiver and any parameter whose pattern is not a plain identifier are
/// always skipped.
pub fn collect_typed_params(
    sig: &syn::Signature,
    keep: impl Fn(&syn::PatType) -> bool,
) -> Vec<TypedParam> {
    sig.inputs
        .iter()
        .filter_map(|arg| {
            let FnArg::Typed(pat_type) = arg else {
                return None;
            };
            if !keep(pat_type) {
                return None;
            }
            let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
                return None;
            };
            Some(TypedParam {
                ident: pat_ident.ident.clone(),
                ty: (*pat_type.ty).clone(),
            })
        })
        .collect()
}

/// Collects the function arguments to reproduce on the generated method's
/// signature: the receiver (when present) plus every typed argument for which
/// `keep` returns `true`, preserving the original argument tokens.
pub fn collect_kept_args(
    sig: &syn::Signature,
    keep: impl Fn(&syn::PatType) -> bool,
) -> Vec<&FnArg> {
    sig.inputs
        .iter()
        .filter(|arg| match arg {
            FnArg::Receiver(_) => true,
            FnArg::Typed(pat_type) => keep(pat_type),
        })
        .collect()
}

pub fn positional_record_schema_value(idents: &[syn::Ident], field_expect: &str) -> TokenStream {
    quote! {
        golem_rust::SchemaValue::Record {
            fields: vec![
                #(<_ as golem_rust::agentic::Schema>::to_schema_value(#idents)
                    .expect(#field_expect)),*
            ],
        }
    }
}

pub fn encode_value_only_carrier(record_expr: TokenStream) -> TokenStream {
    quote! {
        golem_rust::encode_schema_value(&#record_expr)
            .expect("Failed to encode parameters")
    }
}

pub fn memoized_graph_access(build_expr: TokenStream) -> TokenStream {
    quote! {
        {
            static __GOLEM_RPC_GRAPH_CACHE: ::std::sync::OnceLock<golem_rust::SchemaGraph> =
                ::std::sync::OnceLock::new();
            __GOLEM_RPC_GRAPH_CACHE.get_or_init(|| { #build_expr })
        }
    }
}

pub fn decode_result_value(ty: &Type, value_expr: TokenStream) -> TokenStream {
    let graph = memoized_graph_access(quote! {
        <#ty as golem_rust::agentic::Schema>::get_type()
            .get_schema_graph()
            .expect("rpc result type must have a concrete schema graph")
    });
    quote! {
        <#ty as golem_rust::agentic::Schema>::from_schema_value(
            #value_expr,
            golem_rust::agentic::StructuredSchema::Default((#graph).clone()),
        )
        .expect("Failed to deserialize rpc result to return type")
    }
}
