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

use heck::ToKebabCase;
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{Data, DeriveInput, Fields, Lit, LitStr, Variant};

pub fn derive_into_value(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).expect("derive input");
    let ident = &ast.ident;
    let flatten_value = ast
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("flatten_value"));
    let ident_lit = LitStr::new(&ident.to_string(), Span::call_site());

    let (add_to_builder, add_to_type_builder) = match ast.data {
        Data::Struct(data) => {
            let newtype_result = if data.fields.len() == 1 {
                let field = data.fields.iter().next().unwrap().clone();
                if field.ident.is_none() || flatten_value {
                    // single field without an identifier, we consider this a newtype
                    let field_type = field.ty;

                    let add_to_builder = match field.ident {
                        None => quote! {
                            self.0.add_to_builder(builder)
                        },
                        Some(field_name) => quote! {
                            self.#field_name.add_to_builder(builder)
                        },
                    };
                    let add_to_type_builder = quote! {
                        <#field_type as golem_rust::value_and_type::IntoValue>::add_to_type_builder(builder)
                    };

                    Some((add_to_builder, add_to_type_builder))
                } else {
                    None
                }
            } else {
                None
            };

            match newtype_result {
                Some(newtype_result) => newtype_result,
                None => record_or_tuple(&ident_lit, &data.fields),
            }
        }
        Data::Enum(data) => {
            let is_simple_enum = data
                .variants
                .iter()
                .all(|variant| variant.fields.is_empty());

            if is_simple_enum {
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;
                        quote! {
                            #ident::#case_ident => builder.enum_value(#idx)
                        }
                    })
                    .collect::<Vec<_>>();
                let case_labels = data
                    .variants
                    .iter()
                    .map(|variant| variant.ident.to_string().to_kebab_case())
                    .collect::<Vec<_>>();

                let add_to_builder = quote! {
                    match self {
                        #(#case_branches),*
                    }
                };

                let add_to_type_builder = quote! {
                    builder.r#enum(
                        Some(#ident_lit.to_string()),
                        &[#(#case_labels),*]
                    )
                };

                (add_to_builder, add_to_type_builder)
            } else {
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;

                        if variant.fields.is_empty() {
                            quote! {
                                #ident::#case_ident => {
                                    builder.variant_unit(#idx)
                                }
                            }
                        } else if has_single_anonymous_field(&variant.fields) {
                            // separate inner type
                            if is_unit_case(variant) {
                                quote! {
                                    #ident::#case_ident(inner) => {
                                       builder.variant_unit(#idx)
                                    }
                                }
                            } else {
                                quote! {
                                    #ident::#case_ident(inner) => {
                                        let builder = builder.variant(#idx);
                                        inner.add_to_builder(builder).finish()
                                    }
                                }
                            }
                        } else if has_only_named_fields(&variant.fields) {
                            // record case
                            let field_names = variant
                                .fields
                                .iter()
                                .map(|field| {
                                    let field = field.ident.as_ref().unwrap();
                                    quote! { #field }
                                })
                                .collect::<Vec<_>>();

                            let field_values = variant.fields.iter().map(|field| {
                                let field = field.ident.as_ref().unwrap();
                                quote! {
                                    let builder = #field.add_to_builder(builder.item());
                                }
                            });

                            if is_unit_case(variant) {
                                quote! {
                                    #ident::#case_ident { #(#field_names),* } => {
                                       builder.variant_unit(#idx)
                                    }
                                }
                            } else {
                                quote! {
                                    #ident::#case_ident { #(#field_names),* } => {
                                        let builder = builder.variant(#idx);
                                        let builder = builder.record();
                                        vec![#(#field_values)*];
                                        builder.finish().finish()
                                    }
                                }
                            }
                        } else {
                            // tuple case
                            let field_names = variant
                                .fields
                                .iter()
                                .enumerate()
                                .map(|(idx, _field)| {
                                    Ident::new(&format!("f{idx}"), Span::call_site())
                                })
                                .collect::<Vec<_>>();

                            let field_values = field_names.iter().map(|field| {
                                quote! {
                                    let builder = #field.add_to_builder(builder.item());
                                }
                            });

                            if is_unit_case(variant) {
                                quote! {
                                    #ident::#case_ident(#(#field_names),*) => {
                                        builder.variant_unit(#idx)
                                    }
                                }
                            } else {
                                quote! {
                                    #ident::#case_ident(#(#field_names),*) => {
                                        let builder = builder.variant(#idx);
                                        let builder = builder.tuple();
                                        vec![#(#field_values)*];
                                        builder.finish().finish()
                                    }
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                let case_defs = data.variants.iter()
                    .map(|variant| {
                        let case_name = variant.ident.to_string().to_kebab_case();
                        if is_unit_case(variant) {
                            quote! {
                                let builder = builder.unit_case(#case_name);
                            }
                        } else if has_single_anonymous_field(&variant.fields) {
                            let single_field = variant.fields.iter().next().unwrap();
                            let typ = &single_field.ty;

                            quote! {
                                let builder = <#typ as golem_rust::value_and_type::IntoValue>::add_to_type_builder(builder.case(#case_name));
                            }
                        } else {
                            let (_, inner_add_to_type_builder) = record_or_tuple(&ident_lit, &variant.fields);

                            quote! {
                                let builder = builder.case(#case_name);
                                #inner_add_to_type_builder
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                let into_value = quote! {
                    match self {
                        #(#case_branches),*
                    }
                };
                let get_type = quote! {
                    let builder = builder.variant(Some(#ident_lit.to_string()));
                    #(#case_defs)*
                    builder.finish()
                };

                (into_value, get_type)
            }
        }
        Data::Union(_data) => {
            panic!("Cannot derive IntoValue for unions")
        }
    };

    let result = quote! {
        impl golem_rust::value_and_type::IntoValue for #ident {
            fn add_to_builder<B: golem_rust::value_and_type::NodeBuilder>(self, builder: B) -> B::Result {
                #add_to_builder
            }

            fn add_to_type_builder<B: golem_rust::value_and_type::TypeNodeBuilder>(builder: B) -> B::Result {
                #add_to_type_builder
            }
        }
    };

    result.into()
}

fn record_or_tuple(
    ident_lit: &LitStr,
    fields: &Fields,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let all_fields_has_names = fields.iter().all(|field| field.ident.is_some());

    if all_fields_has_names {
        let field_values = fields
            .iter()
            .map(|field| {
                let field_name = field.ident.as_ref().unwrap();
                quote! {
                    let builder = self.#field_name.add_to_builder(builder.item());
                }
            })
            .collect::<Vec<_>>();

        let field_defs = fields
            .iter()
            .map(|field| {
                let field_name = field.ident.as_ref().unwrap().to_string().to_kebab_case();
                let field_type = &field.ty;
                quote! {
                    let builder = <#field_type as golem_rust::value_and_type::IntoValue>::add_to_type_builder(builder.field(#field_name));
                }
            })
            .collect::<Vec<_>>();

        let add_to_builder = quote! {
            let builder = builder.record();
            #(#field_values)*
            builder.finish()
        };
        let add_to_type_builder = quote! {
            let builder = builder.record(Some(#ident_lit.to_string()));
            #(#field_defs)*
            builder.finish()
        };

        (add_to_builder, add_to_type_builder)
    } else {
        let tuple_field_values = fields
            .iter()
            .map(|field| {
                let field_name = field.ident.as_ref().unwrap();
                quote! {
                    let builder = self.#field_name.add_to_builder(builder.item());
                }
            })
            .collect::<Vec<_>>();

        let tuple_field_types = fields
            .iter()
            .map(|field| {
                let field_type = &field.ty;
                quote! {
                    let builder = <#field_type as golem_rust::value_and_type::IntoValue>::add_to_type_builder(builder.item());
                }
            })
            .collect::<Vec<_>>();

        let add_to_builder = quote! {
            let builder = builder.tuple();
            #(#tuple_field_values)*
            builder.finish()
        };
        let add_to_type_builder = quote! {
            let builder = builder.tuple(Some(#ident_lit.to_string()))
            #(#tuple_field_types)*
            builder.finish()
        };

        (add_to_builder, add_to_type_builder)
    }
}

fn has_single_anonymous_field(fields: &Fields) -> bool {
    fields.len() == 1 && fields.iter().next().unwrap().ident.is_none()
}

fn has_only_named_fields(fields: &Fields) -> bool {
    fields.iter().all(|field| field.ident.is_some())
}

fn is_unit_case(variant: &Variant) -> bool {
    variant.fields.is_empty()
        || variant
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("unit_case"))
}

pub fn derive_from_value_and_type(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).expect("derive input");
    let ident = &ast.ident;
    let flatten_value = ast
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("flatten_value"));

    let extractor = match ast.data {
        Data::Struct(data) => {
            let newtype_result = if data.fields.len() == 1 {
                let field = data.fields.iter().next().unwrap().clone();
                if field.ident.is_none() || flatten_value {
                    // single field without an identifier, we consider this a newtype
                    let field_type = field.ty;

                    let extractor = match field.ident {
                        None => quote! {
                            let inner = <#field_type as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                                extractor
                            )?;
                            Ok(Self(inner))
                        },
                        Some(field_name) => quote! {
                            let #field_name = <#field_type as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                                extractor
                            )?;
                            Ok(Self { #field_name })
                        },
                    };

                    Some(extractor)
                } else {
                    None
                }
            } else {
                None
            };

            match newtype_result {
                Some(newtype_result) => newtype_result,
                None => record_or_tuple_extractor(&data.fields),
            }
        }
        Data::Enum(data) => {
            let is_simple_enum = data
                .variants
                .iter()
                .all(|variant| variant.fields.is_empty());

            if is_simple_enum {
                let case_branches = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;
                        quote! {
                            Some(#idx) => Ok(#ident::#case_ident)
                        }
                    })
                    .collect::<Vec<_>>();

                let invalid_case_error = Lit::Str(LitStr::new(
                    &format!("Invalid {}", ast.ident),
                    Span::call_site(),
                ));

                quote! {
                    match extractor.enum_value() {
                        #(#case_branches),*,
                        _ => Err(#invalid_case_error.to_string())
                    }
                }
            } else {
                let cases = data
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(idx, variant)| {
                        let case_ident = &variant.ident;
                        let idx = idx as u32;

                        if variant.fields.is_empty() {
                            quote! {
                                #idx => {
                                    Ok(#ident::#case_ident)
                                }
                            }
                        } else if has_single_anonymous_field(&variant.fields) {
                            // separate inner type
                            if is_unit_case(variant) {
                                quote! {
                                    #idx => {
                                        Ok(#ident::#case_ident)
                                    }
                                }
                            } else {
                                let single_field = variant.fields.iter().next().unwrap();
                                let typ = &single_field.ty;
                                let missing_body_error = Lit::Str(LitStr::new(
                                    &format!("Missing {case_ident} body"),
                                    Span::call_site()
                                ));

                                quote! {
                                    #idx => {
                                        Ok(#ident::#case_ident(
                                            <#typ as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                                                &inner.ok_or_else(|| #missing_body_error.to_string())?
                                            )?
                                        ))
                                    }
                                }
                            }
                        } else if has_only_named_fields(&variant.fields) {
                            // record case
                            if is_unit_case(variant) {
                                quote! {
                                    #idx => {
                                        Ok(#ident::#case_ident)
                                    }
                                }
                            } else {
                                let field_extractors = variant.fields.iter()
                                    .enumerate()
                                    .map(|(idx, field)| {
                                        let field_name = field.ident.as_ref().unwrap();
                                        let field_ty = &field.ty;
                                        let missing_field_error = Lit::Str(LitStr::new(&format!("Missing {field_name} field"), Span::call_site()));
                                        quote! {
                                            #field_name: <#field_ty as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                                                &extractor.field(#idx).ok_or_else(|| #missing_field_error.to_string())?
                                            )?
                                        }
                                    })
                                    .collect::<Vec<_>>();

                                quote! {
                                    Ok(#ident::#case_ident {
                                        #(#field_extractors),*
                                    })
                                }
                            }
                        } else {
                            // tuple case
                            if is_unit_case(variant) {
                                quote! {
                                    #idx => {
                                        Ok(#ident::#case_ident)
                                    }
                                }
                            } else {
                                let field_extractors = variant.fields.iter()
                                    .enumerate()
                                    .map(|(idx, field)| {
                                        let elem_ty = &field.ty;
                                        let missing_tuple_element_error = Lit::Str(LitStr::new(&format!("Missing tuple element #{idx}"), Span::call_site()));
                                        quote! {
                                            <#elem_ty as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                                                &extractor.tuple_element(#idx).ok_or_else(|| #missing_tuple_element_error.to_string())?
                                            )?
                                        }
                                    })
                                    .collect::<Vec<_>>();

                                quote! {
                                    Ok(#ident::#case_ident {
                                        #(#field_extractors),*
                                    })
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                let should_be_variant_error = Lit::Str(LitStr::new(
                    &format!("{} should be variant", ast.ident),
                    Span::call_site(),
                ));

                let invalid_case_format = Lit::Str(LitStr::new(
                    &format!("Invalid {} variant: {{idx}}", ast.ident),
                    Span::call_site(),
                ));

                quote! {
                    let (idx, inner) = extractor
                        .variant()
                        .ok_or_else(|| #should_be_variant_error.to_string())?;
                    match idx {
                        #(#cases),*,
                        _ => Err(format!(#invalid_case_format)),
                    }
                }
            }
        }
        Data::Union(_) => {
            panic!("Cannot derive FromValueAndType for unions")
        }
    };

    let result = quote! {
        impl golem_rust::value_and_type::FromValueAndType for #ident {
            fn from_extractor<'a, 'b>(
                extractor: &'a impl golem_rust::value_and_type::WitValueExtractor<'a, 'b>,
            ) -> Result<Self, String> {
                #extractor
            }
        }
    };

    result.into()
}

fn record_or_tuple_extractor(fields: &Fields) -> proc_macro2::TokenStream {
    let all_fields_has_names = fields.iter().all(|field| field.ident.is_some());

    if all_fields_has_names {
        let field_extractors = fields.iter()
            .enumerate()
            .map(|(idx, field)| {
                let field_name = field.ident.as_ref().unwrap();
                let field_ty = &field.ty;
                let missing_field_error = Lit::Str(LitStr::new(&format!("Missing {field_name} field"), Span::call_site()));
                quote! {
                    #field_name: <#field_ty as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                        &extractor.field(#idx).ok_or_else(|| #missing_field_error.to_string())?
                    )?
                }
            })
            .collect::<Vec<_>>();

        quote! {
            Ok(Self {
                #(#field_extractors),*
            })
        }
    } else {
        let field_extractors = fields.iter()
            .enumerate()
            .map(|(idx, field)| {
                let elem_ty = &field.ty;
                let missing_tuple_element_error = Lit::Str(LitStr::new(&format!("Missing tuple element #{idx}"), Span::call_site()));
                quote! {
                    <#elem_ty as golem_rust::value_and_type::FromValueAndType>::from_extractor(
                        &extractor.tuple_element(#idx).ok_or_else(|| #missing_tuple_element_error.to_string())?
                    )?
                }
            })
            .collect::<Vec<_>>();

        quote! {
            Ok((
                #(#field_extractors),*
            ))
        }
    }
}
