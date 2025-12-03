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

use syn::{ReturnType, Type};

pub struct FunctionOutputInfo {
    pub future_or_immediate: Asyncness,
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
            future_or_immediate: function_kind,
            is_unit,
        }
    }
}

pub enum Asyncness {
    Future,
    Immediate,
}

pub fn is_constructor_method(sig: &syn::Signature) -> bool {
    match &sig.output {
        ReturnType::Type(_, ty) => match &**ty {
            Type::Path(tp) => tp.path.segments.last().unwrap().ident == "Self",
            _ => false,
        },
        _ => false,
    }
}

pub fn is_static_method(sig: &syn::Signature) -> bool {
    !sig.receiver().is_some()
}

pub fn get_asyncness(sig: &syn::Signature) -> Asyncness {
    if sig.asyncness.is_some() {
        Asyncness::Future
    } else {
        Asyncness::Immediate
    }
}

pub fn has_async_trait_attribute(impl_block: &syn::ItemImpl) -> bool {
    impl_block.attrs.iter().any(is_async_trait_attr)
}

pub fn is_async_trait_attr(attr: &syn::Attribute) -> bool {
    let path = attr.path();

    path.is_ident("async_trait") || path.is_ident("async_trait::async_trait")
}
