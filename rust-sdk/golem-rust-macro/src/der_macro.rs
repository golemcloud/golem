// Copyright 2024 Golem Cloud
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

use proc_macro2::TokenStream;
use quote::quote;
use syn::{spanned::Spanned, Data, Error, Fields};

pub fn expand_wit(ast: &mut syn::DeriveInput) -> syn::Result<TokenStream> {
    let name = ast.ident.clone();

    let derived_name = extract_data_type_name(ast.attrs.clone(), name.clone())?;

    match &ast.data {

        // Struct derivation
        Data::Struct(s) => match &s.fields {
            Fields::Named(n) => {
                let field_and_names: syn::Result<Vec<_>> = n
                    .named
                    .iter()
                    .map(|f| {
                        extract_field_name(f.attrs.clone(), f.ident.clone().unwrap())
                            .map(|updated| (f, updated))
                    })
                    .collect();

                let from_fields = field_and_names.clone()?.into_iter().map(|t| {
                    let original = t.0.ident.clone().unwrap();
                    let updated = t.1;
                    quote!(
                        #original: value.#updated.into()
                    )
                });

                let to_fields = field_and_names?.into_iter().map(|t| {
                    let original = t.0.ident.clone().unwrap();
                    let updated = t.1;

                    quote!(
                        #updated: self.#original.into()
                    )
                });

                Ok(quote!(
                    impl From<#derived_name> for #name {
                        fn from(value: #derived_name) -> Self {
                            #name {
                                #(#from_fields),*
                            }
                        }
                    }
                    impl Into<#derived_name> for #name {
                        fn into(self) -> #derived_name {
                            #derived_name {
                                #(#to_fields),*
                            }
                        }
                    }
                ))
            }
            _ => Err(Error::new(
                ast.span(),
                "Unexpected. Please open an issue with description of your use case https://github.com/golemcloud/golem-rust/issues",
            )),
        },

        // Enum derivation
        Data::Enum(data_enum) => {
            let from_fields: syn::Result<Vec<_>> = data_enum.variants.clone().into_iter().map(|variant| {
                let variant_name = variant.ident.clone();
                let new_name = extract_field_name(variant.attrs.clone(), variant_name.clone());

                new_name.map(|n| (n, variant))
            }).collect();

            let from = from_fields?.into_iter().map(|(new_name, variant)|{
                let variant_name = variant.ident.clone();
                match variant.fields {
                    Fields::Unit => {
                        quote!(
                            #derived_name::#new_name => #name::#variant_name
                        )
                    }

                    Fields::Unnamed(un) => {
                        let fake_names = create_fake_names(un.unnamed.len());

                        let fields = quote!(
                            #(#fake_names),*
                        );

                        quote!(
                            #derived_name::#new_name(#fields) => #name::#variant_name(#fields)
                        )
                    }
                    Fields::Named(n) => {
                        let field_names = n.named.into_iter().map(|f| {
                            let l = f.ident.unwrap();

                            quote!(#l)
                        });

                        let fields = quote!(
                            #(#field_names),*
                        );

                        quote!(
                            #derived_name::#new_name { #fields } => #name::#variant_name { #fields }
                        )
                    }
                }
            });

            let into_fields: syn::Result<Vec<_>>  = data_enum.variants.clone().into_iter().map(|variant| {
                let variant_name = variant.ident.clone();
                let new_name =
                    extract_field_name(variant.attrs.clone(), variant_name.clone());
                new_name.map(|n| (n, variant))
            }).collect();
            let into = into_fields?.into_iter().map(|(new_name, variant)|{

                let variant_name = variant.ident.clone();

                match variant.fields {
                    Fields::Unit => {
                        quote!(
                            #name::#variant_name => #derived_name::#new_name
                        )
                    }
                    Fields::Unnamed(u) => {
                        let fake_names = create_fake_names(u.unnamed.len());

                        let fields = quote!(
                            #(#fake_names),*
                        );

                        quote!(
                            #name::#variant_name(#fields) => #derived_name::#new_name(#fields)
                        )
                    }
                    Fields::Named(n) => {
                        let field_names = n.named.into_iter().map(|f| {
                            let l = f.ident.unwrap();

                            quote!(#l)
                        });

                        let fields = quote!(
                            #(#field_names),*
                        );

                        quote!(
                            #name::#variant_name { #fields } => #derived_name::#new_name { #fields }
                        )
                    }
                }
            });

            Ok(quote!(
                impl From<#derived_name> for #name {
                    fn from(value: #derived_name) -> Self {
                        match value {
                            #(#from),*
                        }
                    }
                }
                impl Into<#derived_name> for #name {
                    fn into(self) -> #derived_name {
                        match self {
                            #(#into),*
                        }
                    }
                }
            ))
        }
        _ => Err(Error::new(ast.span(), "Supporting only structs for now")),
    }
}

/**
 * Parses #[wit_type_name(WitPerson)] or #[wit_type_name("WitPerson")] and returns Ok(None) in case "wit_type_name" attribute does not exists or errors out in case of weird structure like #[wit_type_name(100)]
 */
fn extract_data_type_name(
    attrs: Vec<syn::Attribute>,
    origin: syn::Ident,
) -> syn::Result<syn::Ident> {
    let extracted_name = attrs
        .into_iter()
        .find_map(|attr| match attr.meta {
            syn::Meta::List(ml) if ml.path.segments.first().unwrap().ident == "wit_type_name" => {

                Some(syn::parse2::<syn::Ident>(ml.tokens.clone())
                    .or({
                        syn::parse2::<syn::Lit>(ml.tokens.clone())
                            .map_err(|_| {
                                syn::Error::new(
                                    ml.tokens.span(),
                                    "Argument to \"wit_type_name\" must be a either a single data type #[wiwit_type_namet(WitPerson)] or a string #[wit_type_name(\"WitPerson\")]")})
                            .and_then(|l| match l {
                                syn::Lit::Str(lit) => Ok(syn::Ident::new(&lit.value(), lit.span())),
                                _ => Err(syn::Error::new(
                                                    l.span(),
                                                    "Argument to \"wwit_type_nameit\" must be a either a data type #[wit_type_name(WitPerson)] or a string #[wit_type_name(\"WitPerson\")]",
                                    ))
                            })
                    }))
            }
            _ => None,
        });

    match extracted_name {
        Some(name) => name,
        None => Ok(syn::Ident::new(
            &("Wit".to_owned() + &origin.to_string()),
            origin.span(),
        )),
    }
}

/**
 * Looks for #[rename_field("naw_field_name")] attributes in the attribute list.
 *
 * If there are none, returns original ident.
 * Errors out if there's a wrong format like #[rename_field("first", "second")] or #[rename_field(100)]
 */
fn extract_field_name(attrs: Vec<syn::Attribute>, original: syn::Ident) -> syn::Result<syn::Ident> {
    let rename_is_defined = attrs.into_iter().find_map(|attr| match attr.meta.clone() {
        syn::Meta::List(ml) if (ml.path.segments.first().unwrap().ident == "rename_field") => Some(
            syn::parse2::<syn::Lit>(ml.tokens.clone())
                .map_err(|_| {
                    syn::Error::new(
                        ml.path.span(),
                        "Argument to 'rename_field' must be a single String",
                    )
                })
                .and_then(|l| match l {
                    syn::Lit::Str(lit) => Ok(syn::Ident::new(&lit.value(), lit.span())),
                    _ => Err(syn::Error::new(
                        l.span(),
                        "Argument to 'rename_field' must be a String",
                    )),
                }),
        ),
        _ => None,
    });

    match rename_is_defined {
        Some(ident) => ident,
        None => Ok(original),
    }
}

// check if From<> and Into<> can be implemented without making out fake names for unnamed enums
fn create_fake_names(take: usize) -> Vec<proc_macro2::TokenStream> {
    vec![
        'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r',
        's', 't', 'u', 'v', 'x', 'y', 'z',
    ]
    .into_iter()
    .take(take)
    .map(|fake_name| {
        let fake_ident = syn::Ident::new(&fake_name.to_string(), proc_macro2::Span::call_site());
        quote!(#fake_ident)
    })
    .collect()
}
