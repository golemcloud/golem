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

use proc_macro2::Ident;
use syn::{Data, DeriveInput, Type};

pub fn is_recursive(ast: &DeriveInput) -> bool {
    let self_ident = &ast.ident;

    match &ast.data {
        Data::Struct(ds) => ds
            .fields
            .iter()
            .any(|f| type_contains_self(&f.ty, self_ident)),

        Data::Enum(de) => de.variants.iter().any(|v| {
            v.fields
                .iter()
                .any(|f| type_contains_self(&f.ty, self_ident))
        }),

        Data::Union(_) => false,
    }
}

fn type_contains_self(ty: &Type, self_ident: &Ident) -> bool {
    match ty {
        Type::Path(type_path) => {
            let is_direct = type_path
                .path
                .segments
                .last()
                .map(|seg| seg.ident == *self_ident)
                .unwrap_or(false);

            let has_generic_self = type_path
                .path
                .segments
                .iter()
                .any(|seg| match &seg.arguments {
                    syn::PathArguments::AngleBracketed(ab) => ab.args.iter().any(|arg| match arg {
                        syn::GenericArgument::Type(t) => type_contains_self(t, self_ident),
                        _ => false,
                    }),
                    _ => false,
                });

            is_direct || has_generic_self
        }
        Type::Reference(r) => type_contains_self(&r.elem, self_ident),
        Type::Ptr(p) => type_contains_self(&p.elem, self_ident),
        Type::Slice(s) => type_contains_self(&s.elem, self_ident),
        Type::Array(a) => type_contains_self(&a.elem, self_ident),
        Type::Paren(p) => type_contains_self(&p.elem, self_ident),
        Type::Tuple(t) => t.elems.iter().any(|e| type_contains_self(e, self_ident)),
        Type::Group(g) => type_contains_self(&g.elem, self_ident),
        _ => false,
    }
}
