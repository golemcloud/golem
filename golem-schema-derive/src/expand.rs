// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Codegen entry points.

use crate::codegen::r#enum::{expand_enum_from_schema, expand_enum_into_schema};
use crate::codegen::r#struct::{expand_struct_from_schema, expand_struct_into_schema};
use crate::codegen::union::{expand_union_from_schema, expand_union_into_schema};
use crate::parse::parse_type_attrs;
use proc_macro2::TokenStream;
use syn::{Data, DeriveInput};

pub fn expand_into_schema(input: DeriveInput) -> syn::Result<TokenStream> {
    let type_attrs = parse_type_attrs(&input.attrs)?;
    match &input.data {
        Data::Struct(data) => expand_struct_into_schema(&input, &type_attrs, data),
        Data::Enum(data) => {
            if type_attrs.union {
                expand_union_into_schema(&input, &type_attrs, data)
            } else {
                expand_enum_into_schema(&input, &type_attrs, data)
            }
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            input.ident,
            "`union` types are not supported by `IntoSchema`",
        )),
    }
}

pub fn expand_from_schema(input: DeriveInput) -> syn::Result<TokenStream> {
    let type_attrs = parse_type_attrs(&input.attrs)?;
    match &input.data {
        Data::Struct(data) => expand_struct_from_schema(&input, &type_attrs, data),
        Data::Enum(data) => {
            if type_attrs.union {
                expand_union_from_schema(&input, &type_attrs, data)
            } else {
                expand_enum_from_schema(&input, &type_attrs, data)
            }
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            input.ident,
            "`union` types are not supported by `FromSchema`",
        )),
    }
}
