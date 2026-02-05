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

pub fn derive_allowed_languages(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let name = &ast.ident;

    let Data::Enum(data_enum) = &ast.data else {
        return syn::Error::new_spanned(
            &ast.ident,
            "AllowedLanguages can only be derived for enums",
        )
        .to_compile_error()
        .into();
    };

    let mut variant_idents = Vec::new();
    let mut lang_codes = Vec::new();

    for variant in &data_enum.variants {
        let v_ident = &variant.ident;
        variant_idents.push(v_ident);

        let mut code = v_ident.to_string().to_lowercase();

        for attr in &variant.attrs {
            if let Some(override_code) = parse_lang_attr(attr) {
                code = override_code;
                break;
            }
        }

        lang_codes.push(code);
    }

    let code_strs: Vec<_> = lang_codes.iter().map(|s| s.as_str()).collect();

    let expanded = quote! {
        impl #golem_rust_crate_ident::agentic::AllowedLanguages for #name {
            fn all() -> &'static [&'static str] {
                &[#(#code_strs),*]
            }

            fn from_language_code(code: &str) -> Option<Self> {
                match code {
                    #(
                        #code_strs => Some(Self::#variant_idents),
                    )*
                    _ => None,
                }
            }

            fn to_language_code(&self) -> &'static str {
                match self {
                    #(
                        Self::#variant_idents => #code_strs,
                    )*
                }
            }
        }
    };

    expanded.into()
}
fn parse_lang_attr(attr: &Attribute) -> Option<String> {
    if attr.path().is_ident("code") {
        if let Ok(Lit::Str(lit_str)) = attr.parse_args::<Lit>() {
            return Some(lit_str.value());
        }
    }
    None
}
