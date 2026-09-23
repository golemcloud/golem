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

use super::helpers::{default_name_for, schema_crate_path};
use crate::parse::{ItemAttrs, RichSpec, parse_item_attrs, parse_type_attrs};
use proc_macro2::{TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, GenericParam, Member, Type};

pub fn expand(input: &DeriveInput, encode: bool) -> syn::Result<TokenStream> {
    let schema = schema_crate_path();
    let wire = quote!(#schema::schema::wit::wire);
    let direct = quote!(#schema::schema::wit::direct);
    let attrs = parse_type_attrs(&input.attrs)?;
    let ident = &input.ident;
    let mut generics = input.generics.clone();
    let trait_name = if encode {
        quote!(#direct::IntoWire)
    } else {
        quote!(#direct::FromWire)
    };
    let mut encoded_types = TokenStream::new();
    let field_groups = match &input.data {
        Data::Struct(data) => vec![(&data.fields, !attrs.transparent)],
        Data::Enum(data) => data.variants.iter().map(|v| (&v.fields, false)).collect(),
        Data::Union(_) => Vec::new(),
    };
    for (fields, is_struct) in field_groups {
        for field in fields {
            let field_attrs = parse_item_attrs(&field.attrs)?;
            let ty = &field.ty;
            if is_struct
                && matches!(fields, Fields::Named(_))
                && (field_attrs.skip || field_attrs.default_with.is_some())
            {
                if !encode && field_attrs.default_with.is_none() {
                    generics
                        .make_where_clause()
                        .predicates
                        .push(syn::parse_quote!(#ty: ::core::default::Default));
                }
            } else {
                encoded_types.extend(quote!(#ty));
            }
        }
    }
    for param in &mut generics.params {
        if let GenericParam::Type(param) = param
            && contains_ident(encoded_types.clone(), &param.ident)
        {
            param.bounds.push(syn::parse2(trait_name.clone())?);
        }
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let (preflight, prepare, body) = match &input.data {
        Data::Struct(data) => {
            let single = attrs.transparent;
            if single
                && !matches!(&data.fields, Fields::Unnamed(fields) if fields.unnamed.len() == 1)
            {
                return Err(syn::Error::new_spanned(
                    input,
                    "transparent wire conversion requires a single tuple field",
                ));
            }
            fields(
                &data.fields,
                quote!(Self),
                single,
                true,
                encode,
                &wire,
                &direct,
            )?
        }
        Data::Enum(data) => {
            let all_unit = !attrs.union
                && !data.variants.is_empty()
                && data
                    .variants
                    .iter()
                    .all(|variant| matches!(variant.fields, Fields::Unit));
            let mut preflight_arms = Vec::new();
            let mut prepare_arms = Vec::new();
            let mut arms = Vec::new();
            for (case, variant) in data.variants.iter().enumerate() {
                let variant_ident = &variant.ident;
                let constructor = quote!(Self::#variant_ident);
                let single =
                    matches!(&variant.fields, Fields::Unnamed(fields) if fields.unnamed.len() == 1);
                if attrs.union && !single {
                    return Err(syn::Error::new_spanned(
                        variant,
                        "wire union branches require one tuple field",
                    ));
                }
                let (preflight, prepare, value) = fields(
                    &variant.fields,
                    constructor.clone(),
                    single,
                    false,
                    encode,
                    &wire,
                    &direct,
                )?;
                let pattern = pattern(&variant.fields, constructor);
                let case = case as u32;
                let variant_attrs = parse_item_attrs(&variant.attrs)?;
                let tag = variant_attrs
                    .rename
                    .unwrap_or_else(|| default_name_for(variant_ident, attrs.rename_all));
                if encode {
                    preflight_arms.push(quote!(#pattern => { #preflight }));
                    prepare_arms.push(quote!(#pattern => { #prepare }));
                    let node = if all_unit {
                        quote!(#wire::SchemaValueNode::EnumValue(#case))
                    } else if attrs.union {
                        quote!(#wire::SchemaValueNode::UnionValue(#wire::UnionValuePayload { tag: #tag.to_string(), body: { #value }? }))
                    } else {
                        let payload = if matches!(variant.fields, Fields::Unit) {
                            quote!(::core::option::Option::None)
                        } else {
                            quote!(::core::option::Option::Some({ #value }?))
                        };
                        quote!(#wire::SchemaValueNode::VariantValue(#wire::VariantValuePayload { case: #case, payload: #payload }))
                    };
                    arms.push(quote!(#pattern => {
                        let node = #node;
                        ::core::result::Result::Ok(writer.push(node))
                    }));
                } else {
                    let key = if attrs.union {
                        quote!(#tag)
                    } else {
                        quote!(#case)
                    };
                    let decoded = if matches!(variant.fields, Fields::Unit) {
                        if all_unit {
                            quote!(::core::result::Result::Ok(Self::#variant_ident))
                        } else {
                            quote! {
                                if payload.is_some() { return ::core::result::Result::Err(#direct::WireError::Shape("absent variant payload")); }
                                ::core::result::Result::Ok(Self::#variant_ident)
                            }
                        }
                    } else if attrs.union {
                        value
                    } else {
                        quote! {
                            let index = payload.ok_or(#direct::WireError::Shape("variant payload"))?;
                            #value
                        }
                    };
                    arms.push(quote!(#key => { #decoded }));
                }
            }
            if encode {
                let subject = if data.variants.is_empty() {
                    quote!(*self)
                } else {
                    quote!(self)
                };
                (
                    quote!(match #subject { #(#preflight_arms),* }),
                    quote!(match #subject { #(#prepare_arms),* }),
                    quote!(match #subject { #(#arms),* }),
                )
            } else {
                let (extract, key) = if all_unit {
                    (
                        quote! {
                            let #wire::SchemaValueNode::EnumValue(case) = reader.take(index)? else {
                                return ::core::result::Result::Err(#direct::WireError::Shape("enum"));
                            };
                        },
                        quote!(case),
                    )
                } else if attrs.union {
                    (
                        quote! {
                            let #wire::SchemaValueNode::UnionValue(value) = reader.take(index)? else {
                                return ::core::result::Result::Err(#direct::WireError::Shape("union"));
                            };
                            let index = value.body;
                        },
                        quote!(value.tag.as_str()),
                    )
                } else {
                    (
                        quote! {
                            let #wire::SchemaValueNode::VariantValue(value) = reader.take(index)? else {
                                return ::core::result::Result::Err(#direct::WireError::Shape("variant"));
                            };
                            let payload = value.payload;
                        },
                        quote!(value.case),
                    )
                };
                (
                    quote!(),
                    quote!(),
                    quote! {
                        #extract
                        match #key { #(#arms,)* _ => ::core::result::Result::Err(#direct::WireError::Shape("variant case")) }
                    },
                )
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                input,
                "Rust unions cannot be converted to wire values",
            ));
        }
    };

    let result_payload = if attrs.transparent {
        let Data::Struct(data) = &input.data else {
            return Err(syn::Error::new_spanned(
                input,
                "transparent wire conversion requires a tuple struct",
            ));
        };
        let ty = &data.fields.iter().next().unwrap().ty;
        if encode {
            quote! {
                fn write_result_payload(&self, writer: &mut #direct::WireWriter) -> ::core::result::Result<::core::option::Option<#wire::ValueNodeIndex>, #direct::WireError> {
                    <#ty as #direct::IntoWire>::write_result_payload(&self.0, writer)
                }
            }
        } else {
            quote! {
                fn read_result_payload(reader: &mut #direct::WireReader, index: ::core::option::Option<#wire::ValueNodeIndex>) -> ::core::result::Result<Self, #direct::WireError> {
                    <#ty as #direct::FromWire>::read_result_payload(reader, index).map(Self)
                }
            }
        }
    } else {
        quote!()
    };
    let methods = if encode {
        quote! {
            fn preflight(&self, resources: &mut #direct::WirePreflight) -> ::core::result::Result<(), #direct::WireError> {
                #preflight
            }
            async fn prepare_wire(&self) -> ::core::result::Result<(), #direct::WireError> {
                #prepare
            }
            fn write_wire(&self, writer: &mut #direct::WireWriter) -> ::core::result::Result<#wire::ValueNodeIndex, #direct::WireError> {
                #body
            }
        }
    } else {
        quote! {
            fn read_wire(reader: &mut #direct::WireReader, index: #wire::ValueNodeIndex) -> ::core::result::Result<Self, #direct::WireError> {
                #body
            }
        }
    };
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #trait_name for #ident #ty_generics #where_clause { #methods #result_payload }
    })
}

fn contains_ident(tokens: TokenStream, ident: &syn::Ident) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(candidate) => candidate == *ident,
        TokenTree::Group(group) => contains_ident(group.stream(), ident),
        _ => false,
    })
}

fn pattern(fields: &Fields, constructor: TokenStream) -> TokenStream {
    let names = fields
        .iter()
        .enumerate()
        .map(|(i, _)| format_ident!("__field_{i}"))
        .collect::<Vec<_>>();
    match fields {
        Fields::Named(fields) => {
            let members = fields
                .named
                .iter()
                .map(|field| field.ident.as_ref().unwrap());
            quote!(#constructor { #(#members: #names),* })
        }
        Fields::Unnamed(_) => quote!(#constructor(#(#names),*)),
        Fields::Unit => constructor,
    }
}

fn fields(
    fields: &Fields,
    constructor: TokenStream,
    single: bool,
    is_struct: bool,
    encode: bool,
    wire: &TokenStream,
    direct: &TokenStream,
) -> syn::Result<(TokenStream, TokenStream, TokenStream)> {
    let mut preflight = Vec::new();
    let mut prepare = Vec::new();
    let mut values = Vec::new();
    let mut decoded = Vec::new();
    let mut encoded_count = 0usize;
    for (i, field) in fields.iter().enumerate() {
        let attrs = if is_struct && single {
            ItemAttrs::default()
        } else {
            parse_item_attrs(&field.attrs)?
        };
        let ty = &field.ty;
        let binding = format_ident!("__field_{i}");
        let member = field
            .ident
            .clone()
            .map(Member::Named)
            .unwrap_or_else(|| Member::Unnamed(i.into()));
        let skipped = is_struct
            && matches!(fields, Fields::Named(_))
            && (attrs.skip || attrs.default_with.is_some());
        if skipped {
            let default = if let Some(path) = attrs.default_with {
                let path: syn::Path = syn::parse_str(&path)?;
                quote!(#path())
            } else {
                quote!(::core::default::Default::default())
            };
            decoded.push(quote!(#member: #default));
            continue;
        }
        let access = if is_struct {
            quote!(&self.#member)
        } else {
            quote!(#binding)
        };
        let pos = encoded_count;
        encoded_count += 1;
        if encode {
            if attrs.rich.is_none() || matches!(attrs.rich, Some(RichSpec::QuotaToken(_))) {
                preflight.push(quote!(<#ty as #direct::IntoWire>::preflight(#access, resources)?;));
                prepare.push(quote!(<#ty as #direct::IntoWire>::prepare_wire(#access).await?;));
            }
            values.push(write_field(ty, &attrs, access, wire, direct));
        } else {
            let index = if single {
                quote!(index)
            } else {
                quote!(indices[#pos])
            };
            let value = read_field(ty, &attrs, index, wire, direct);
            decoded.push(if matches!(fields, Fields::Named(_)) {
                quote!(#member: #value)
            } else {
                value
            });
        }
    }
    let preflight = quote! { #(#preflight)* ::core::result::Result::Ok(()) };
    let prepare = quote! { #(#prepare)* ::core::result::Result::Ok(()) };
    let node = if matches!(fields, Fields::Unnamed(_)) {
        quote!(TupleValue)
    } else {
        quote!(RecordValue)
    };
    let body = if encode {
        if single {
            let value = &values[0];
            quote!(#value)
        } else {
            quote! {
                let indices = ::std::vec![#(#values?),*];
                ::core::result::Result::Ok::<_, #direct::WireError>(writer.push(#wire::SchemaValueNode::#node(indices)))
            }
        }
    } else {
        let extract = if single {
            quote!()
        } else {
            quote! {
                let #wire::SchemaValueNode::#node(indices) = reader.take(index)? else {
                    return ::core::result::Result::Err(#direct::WireError::Shape(stringify!(#node)));
                };
                if indices.len() != #encoded_count { return ::core::result::Result::Err(#direct::WireError::Shape("field count")); }
            }
        };
        let result = match fields {
            Fields::Named(_) => quote!(#constructor { #(#decoded),* }),
            Fields::Unnamed(_) => quote!(#constructor(#(#decoded),*)),
            Fields::Unit => constructor,
        };
        quote! { #extract ::core::result::Result::Ok(#result) }
    };
    Ok((preflight, prepare, body))
}

fn write_field(
    ty: &Type,
    attrs: &ItemAttrs,
    value: TokenStream,
    wire: &TokenStream,
    direct: &TokenStream,
) -> TokenStream {
    let node = match &attrs.rich {
        Some(RichSpec::Text(spec)) => {
            let language = option_string(spec.language.as_deref());
            quote!(#wire::SchemaValueNode::TextValue(#wire::TextValuePayload { text: (#value).clone(), language: #language }))
        }
        Some(RichSpec::Binary(spec)) => {
            let mime = option_string(spec.mime_type.as_deref());
            quote!(#wire::SchemaValueNode::BinaryValue(#wire::BinaryValuePayload { bytes: (#value).clone(), mime_type: #mime }))
        }
        Some(RichSpec::Path(_)) => quote!(#wire::SchemaValueNode::PathValue((#value).clone())),
        Some(RichSpec::Url(_)) => quote!(#wire::SchemaValueNode::UrlValue((#value).clone())),
        Some(RichSpec::Quantity(_)) => {
            quote!(#wire::SchemaValueNode::QuantityValueNode(#wire::QuantityValue {
                mantissa: (#value).mantissa, scale: (#value).scale, unit: (#value).unit.clone(),
            }))
        }
        _ => return quote!(<#ty as #direct::IntoWire>::write_wire(#value, writer)),
    };
    quote!(::core::result::Result::Ok::<_, #direct::WireError>(writer.push(#node)))
}

fn read_field(
    ty: &Type,
    attrs: &ItemAttrs,
    index: TokenStream,
    wire: &TokenStream,
    direct: &TokenStream,
) -> TokenStream {
    let (variant, value) = match &attrs.rich {
        Some(RichSpec::Text(_)) => (quote!(TextValue), quote!(value.text)),
        Some(RichSpec::Binary(_)) => (quote!(BinaryValue), quote!(value.bytes)),
        Some(RichSpec::Path(_)) => (quote!(PathValue), quote!(value)),
        Some(RichSpec::Url(_)) => (quote!(UrlValue), quote!(value)),
        Some(RichSpec::Quantity(_)) => (
            quote!(QuantityValueNode),
            quote!(#ty { mantissa: value.mantissa, scale: value.scale, unit: value.unit }),
        ),
        _ => return quote!(<#ty as #direct::FromWire>::read_wire(reader, #index)?),
    };
    quote! {
        match reader.take(#index)? {
            #wire::SchemaValueNode::#variant(value) => #value,
            _ => return ::core::result::Result::Err(#direct::WireError::Shape(stringify!(#variant))),
        }
    }
}

fn option_string(value: Option<&str>) -> TokenStream {
    match value {
        Some(value) => quote!(::core::option::Option::Some(#value.to_string())),
        None => quote!(::core::option::Option::None),
    }
}
