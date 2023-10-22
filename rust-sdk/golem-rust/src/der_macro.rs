use proc_macro2::TokenStream;
use quote::quote;
use syn::{spanned::Spanned, Data, Error, Fields};

// TODO
// 1. error handling
// 2  more tests in golem-rust-example
// 3. cleanup
// 4. write readme
pub fn expand_wit(ast: &mut syn::DeriveInput) -> syn::Result<TokenStream> {

    if ast.attrs.len() > 1 {
        return Err(syn::Error::new(
            ast.span(),
            "Too many attributes provided to wit. Call #[wit(DataTypeName)] instead.",
        ));
    }

    let name = ast.ident.clone();

    let derived_name = if ast.attrs.len() < 1 {
        syn::Ident::new(&("Wit".to_owned() + &name.to_string()), name.span())
    } else {
        match &ast.attrs.first().unwrap().meta {
            syn::Meta::List(lm) => {
                let att_name: syn::Ident = syn::parse2(lm.tokens.clone()).unwrap();
                att_name
            }
            _ => {
                return Err(syn::Error::new(
                    ast.span(),
                    "Unexpected attribute structure. Call #[wit(DataTypeName)] instead.",
                ))
            }
        }
    };

    match &ast.data {
        Data::Struct(s) => {
            match &s.fields {
                Fields::Named(n) => {

                    let field_and_names = n.named.iter().map(|f| {
                        let field_name = f.ident.clone().unwrap();

                        if f.attrs.len() > 1 {
                            //TODO error handling
                            unimplemented!(
                                "Supporting only one field attribute #[wit(rename = '')]"
                            );

                            // Err(syn::Error::new(
                            //     ast.span(),
                            //     "Unexpected attribute structure. Call #[wit(DataTypeName)] instead.",
                            // ))
                        }

                        let updated = extract_name(f.attrs.clone(), field_name);

                        (f, updated)
                    });

                    let from_fields = field_and_names.clone().into_iter().map(|t| {
                        let original = t.0.ident.clone().unwrap();
                        let updated = t.1;
                        quote!(
                            #original: value.#updated.into()
                        )
                    });

                    let to_fields = field_and_names.clone().into_iter().map(|t| {
                        let original = t.0.ident.clone().unwrap();
                        let updated = t.1;

                        quote!(
                            #updated: self.#original.into()
                        )
                    });

                    let from = quote!(
                        impl From<#derived_name> for #name {
                            fn from(value: #derived_name) -> Self {
                                #name {
                                    #(#from_fields),*
                                }
                            }
                        }
                    );

                    let into = quote!(
                        impl Into<#derived_name> for #name {
                            fn into(self) -> #derived_name {
                                #derived_name {
                                    #(#to_fields),*
                                }
                            }
                        }
                    );

                    Ok(quote!(
                        #from
                        #into
                    ))
                }
                _ => Err(Error::new(
                    ast.span(),
                    "supporting only structs with named fields",
                )),
            }
        }
        Data::Enum(data_enum) => {

            let from_fields = data_enum.variants.clone().into_iter().map(|variant| {
                let variant_name = variant.ident;
                let new_name = extract_name(variant.attrs.clone(), variant_name.clone());

                match variant.fields {
                    Fields::Unit => {
                        quote!(
                            #derived_name::#new_name => #name::#variant_name
                        )
                    }
                    Fields::Unnamed(un) => {
                        let fake_names = vec![
                            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n',
                            'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'x', 'y', 'z',
                        ]
                        .into_iter()
                        .take(un.unnamed.len())
                        .map(|fake_name| {
                            let fake_ident = syn::Ident::new(
                                &fake_name.to_string(),
                                proc_macro2::Span::call_site(),
                            );
                            quote!(#fake_ident)
                        });

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

            let into_fields = data_enum.variants.clone().into_iter().map(|variant| {
                let variant_name = variant.ident;
                let new_name = extract_name(variant.attrs.clone(), variant_name.clone());

                match variant.fields {
                    Fields::Unit => {
                        quote!(
                            #name::#variant_name => #derived_name::#new_name
                        )
                    }
                    Fields::Unnamed(u) => {
                        // TODO better way to generate fake names
                        let fake_names = vec![
                            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n',
                            'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'x', 'y', 'z',
                        ]
                        .into_iter()
                        .take(u.unnamed.len())
                        .map(|fake_name| {
                            let fake_ident = syn::Ident::new(
                                &fake_name.to_string(),
                                proc_macro2::Span::call_site(),
                            );
                            quote!(#fake_ident)
                        });

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

            let from = quote!(

                impl From<#derived_name> for #name {
                    fn from(value: #derived_name) -> Self {
                        match value {
                            #(#from_fields),*
                        }
                    }
                }

            );

            let into = quote!(
                impl Into<#derived_name> for #name {
                    fn into(self) -> #derived_name {
                        match self {
                            #(#into_fields),*
                        }
                    }
                }

            );

            Ok(quote!(
                #from
                #into
            ))
        }
        _ => Err(Error::new(ast.span(), "Supporting only structs for now")),
    }
}

/**
 * Checks if there are any rename #[wit(rename = "naw_field_name")] field attributes, otherwise returns original name.
 */
fn extract_name(attrs: Vec<syn::Attribute>, original: syn::Ident) -> syn::Ident {
    if attrs.len() == 1 {
        match attrs.first().unwrap().meta.clone() {
            syn::Meta::List(ml) => ml
                .tokens
                .clone()
                .into_iter()
                .filter_map(|t| match t {
                    proc_macro2::TokenTree::Literal(lit) => {
                        let l: syn::Lit =
                            syn::parse2(proc_macro2::TokenTree::Literal(lit).into()).unwrap();

                        let new_name = match &l {
                            syn::Lit::Str(name) => name.value(),
                            _ => unimplemented!("Supporting only string for renaming fields."),
                        };

                        Some(syn::Ident::new(&new_name, l.span()))
                    }
                    _ => None,
                })
                .next()
                .unwrap(),
            _ => unimplemented!("unexpected branch"),
        }
    } else {
        original
    }
}