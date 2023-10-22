use syn::{*, spanned::Spanned};
use quote::quote;

// TODO probably delete this attribute macro 
pub fn expand_wit(attributes: &mut DeriveAttributes, ast: &mut syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {

    let name = &ast.ident;
    let derived_name = &attributes.name;

    match &ast.data {
        Data::Struct(s) => {
            match &s.fields {
                Fields::Named(n) => {

                    let from_fields = n.named.iter().map(|f| {
                        let field_name = f.ident.clone().unwrap();

                        let updated = attributes
                            .mappers
                            .clone()
                            .into_iter()
                            .find(|f| {
                                if f.0 == field_name {
                                    true
                                } else {
                                    return false;
                                }
                            })
                            .map(|t| t.1)
                            .or(Some(field_name.clone()))
                            .unwrap();

                        quote!(
                            #field_name: value.#updated.into()
                        )
                    });

                    let to_fields = n.named.iter().map(|f| {
                        let field_name = f.ident.clone().unwrap();

                        let updated = attributes
                            .mappers
                            .clone()
                            .into_iter()
                            .find(|f| {
                                if f.0 == field_name {
                                    true
                                } else {
                                    return false;
                                }
                            })
                            .map(|t| t.1)
                            .or(Some(field_name.clone()))
                            .unwrap();

                        quote!(
                            #updated: self.#field_name.into()
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
                _ => Err(Error::new(ast.span(), "supporting only structs with named fields"))
            }
        }
        Data::Enum(data_enum) => {

            let from_fields = data_enum.variants.clone()
                .into_iter()
                .map(|variant| {

                    let variant_name = variant.ident;

                    match variant.fields {
                        Fields::Unit => {
                            quote!(
                                // TODO support name change?
                                #derived_name::#variant_name => #name::#variant_name
                            )
                        },
                        Fields::Unnamed(u) => {

                            let fake_names = vec!['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'x', 'y', 'z']
                                .into_iter()
                                .take(u.unnamed.len())
                                .map(|fake_name| {
                                    let fake_ident = Ident::new(&fake_name.to_string(), proc_macro2::Span::call_site());
                                    quote!(#fake_ident)
                                });

                            let fields = quote!(
                                #(#fake_names),*
                            );

                            quote!(
                                #derived_name::#variant_name(#fields) => #name::#variant_name(#fields)
                            )
                        },
                        Fields::Named(n) => {
                            
                            let field_names = n.named
                                .into_iter()
                                .map(|f| {
                                    let l = f.ident.unwrap();

                                    quote!(#l)
                                });

                            let fields = quote!(
                                #(#field_names),*
                            );

                            quote!(
                                #derived_name::#variant_name { #fields } => #name::#variant_name { #fields }
                            )
                        }
                    }
                });

                let into_fields = data_enum.variants.clone()
                    .into_iter()
                    .map(|variant| {

                        let variant_name = variant.ident;

                        match variant.fields {
                            Fields::Unit => {
                                quote!(
                                    // TODO support name change?
                                    #name::#variant_name => #derived_name::#variant_name
                                )
                            },
                            Fields::Unnamed(u) => {

                                let fake_names = vec!['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'x', 'y', 'z']
                                    .into_iter()
                                    .take(u.unnamed.len())
                                    .map(|fake_name| {
                                        let fake_ident = Ident::new(&fake_name.to_string(), proc_macro2::Span::call_site());
                                        quote!(#fake_ident)
                                    });

                                let fields = quote!(
                                    #(#fake_names),*
                                );

                                quote!(
                                    #name::#variant_name(#fields) => #derived_name::#variant_name(#fields)
                                )
                            },
                            Fields::Named(n) => {
                                
                                let field_names = n.named
                                    .into_iter()
                                    .map(|f| {
                                        let l = f.ident.unwrap();

                                        quote!(#l)
                                    });

                                let fields = quote!(
                                    #(#field_names),*
                                );

                                quote!(
                                    #name::#variant_name { #fields } => #derived_name::#variant_name { #fields }
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
        _ => Err(Error::new(ast.span(), "Macro is supporting only structs or enums."))
    } 
}

pub struct DeriveAttributes {
    pub name: Ident,
    pub mappers: Vec<(Ident, Ident)>,
}


impl syn::parse::Parse for DeriveAttributes {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let mut name = input.parse::<syn::Ident>()?;

        let mut mappers: Vec<(Ident, Ident)> = Vec::new();

        while input.peek(Token![:]) == true {
            input.parse::<Token![:]>()?;
            input.parse::<Token![:]>()?;

            // TODO find better solution, right now the full path is ignored
            name = input.parse::<syn::Ident>()?;
        }

        while !input.is_empty() {
            input.parse::<Token![,]>()?;

            let from = input.parse::<syn::Ident>()?;

            input.parse::<Token![=]>()?;
            input.parse::<Token![>]>()?;

            let to = input.parse::<syn::Ident>()?;

            mappers.push((from, to));
        }

        Ok(DeriveAttributes {
            name: name,
            mappers: mappers,
        })
    }
}