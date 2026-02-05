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

use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use syn::{parse_macro_input, Attribute, Data, DeriveInput, Lit};

pub fn derive_allowed_mime_types(
    input: TokenStream,
    golem_rust_crate_ident: &Ident,
) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let name = &ast.ident;

    let Data::Enum(data_enum) = &ast.data else {
        return syn::Error::new_spanned(
            &ast.ident,
            "AllowedMimeTypes can only be derived for enums",
        )
        .to_compile_error()
        .into();
    };

    let mut variant_idents = Vec::new();
    let mut mime_types = Vec::new();

    for variant in &data_enum.variants {
        let v_ident = &variant.ident;
        variant_idents.push(v_ident);

        let mut mime_type = v_ident.to_string().to_lowercase();

        for attr in &variant.attrs {
            if let Some(override_mime) = parse_mime_type_attr(attr) {
                mime_type = override_mime;
                break;
            }
        }

        mime_types.push(mime_type);
    }

    let mime_strs: Vec<_> = mime_types.iter().map(|s| s.as_str()).collect();

    let expanded = quote! {
        impl #golem_rust_crate_ident::agentic::AllowedMimeTypes for #name {
            fn all() -> &'static [&'static str] {
                &[#(#mime_strs),*]
            }

            fn from_string(mime_type: &str) -> Option<Self> {
                match mime_type {
                    #(
                        #mime_strs => Some(Self::#variant_idents),
                    )*
                    _ => None,
                }
            }

            fn to_string(&self) -> String {
                match self {
                    #(
                        Self::#variant_idents => #mime_strs.to_string(),
                    )*
                }
            }
        }
    };

    expanded.into()
}
fn parse_mime_type_attr(attr: &Attribute) -> Option<String> {
    if attr.path().is_ident("mime_type") {
        if let Ok(Lit::Str(lit_str)) = attr.parse_args::<Lit>() {
            return Some(lit_str.value());
        }
    }
    None
}
