// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

mod from;
mod into;

use proc_macro::TokenStream;
use proc_macro2::Ident;
use syn::{Attribute, LitStr, Type, Variant};
use syn::parse::{Parse, ParseStream};

#[proc_macro_derive(IntoValue, attributes(wit_transparent, unit_case, wit_field))]
pub fn derive_into_value(input: TokenStream) -> TokenStream {
    into::derive_into_value(input)
}

#[proc_macro_derive(FromValue)]
pub fn derive_from_value(input: TokenStream) -> TokenStream {
    from::derive_from_value(input)
}


#[derive(Default)]
struct WitField {
    skip: bool,
    rename: Option<LitStr>,
    convert: Option<Type>,
    convert_vec: Option<Type>,
    convert_option: Option<Type>,
}

fn parse_wit_field_attribute(attr: &Attribute) -> WitField {
    attr.parse_args_with(WitField::parse)
        .expect("failed to parse wit_field attribute")
}

impl Parse for WitField {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut skip = false;
        let mut rename = None;
        let mut convert = None;
        let mut convert_vec = None;
        let mut convert_option = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if ident == "skip" {
                skip = true;
            } else if ident == "rename" {
                input.parse::<syn::Token![=]>()?;
                rename = Some(input.parse()?);
            } else if ident == "convert" {
                input.parse::<syn::Token![=]>()?;
                convert = Some(input.parse()?);
            } else if ident == "convert_vec" {
                input.parse::<syn::Token![=]>()?;
                convert_vec = Some(input.parse()?);
            } else if ident == "convert_option" {
                input.parse::<syn::Token![=]>()?;
                convert_option = Some(input.parse()?);
            } else {
                return Err(syn::Error::new(ident.span(), "unexpected attribute"));
            }
        }

        Ok(WitField {
            skip,
            rename,
            convert,
            convert_vec,
            convert_option,
        })
    }
}

fn is_unit_case(variant: &Variant) -> bool {
    variant.fields.is_empty()
        || variant
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("unit_case"))
}
