// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::metadata::{MetadataEnvelope, Role};
use golem_common::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NumericBound, NumericRestrictions, PathDirection,
    PathKind, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec, SchemaType, SecretSpec,
    TextRestrictions, UrlRestrictions,
};
use proc_macro2::TokenStream;
use quote::quote;

#[derive(Default)]
pub(crate) struct SchemaGraphRegistry {
    graphs: Vec<SchemaGraph>,
}

impl SchemaGraphRegistry {
    pub(crate) fn intern(&mut self, graph: SchemaGraph) -> usize {
        if let Some(index) = self.graphs.iter().position(|known| known == &graph) {
            index
        } else {
            let index = self.graphs.len();
            self.graphs.push(graph);
            index
        }
    }

    pub(crate) fn definitions(&self) -> TokenStream {
        let definitions = self.graphs.iter().enumerate().map(|(index, graph)| {
            let name = graph_ident(index);
            let literal = emit_schema_graph_literal(graph);
            quote! {
                static #name: std::sync::LazyLock<golem_rust::SchemaGraph> =
                    std::sync::LazyLock::new(|| #literal);
            }
        });
        quote! { #(#definitions)* }
    }
}

pub(crate) fn graph_clone(index: usize) -> TokenStream {
    let name = graph_ident(index);
    quote! { (*#name).clone() }
}

fn graph_ident(index: usize) -> syn::Ident {
    syn::Ident::new(
        &format!("__GOLEM_SCHEMA_GRAPH_{index}"),
        proc_macro2::Span::call_site(),
    )
}

pub(crate) fn emit_schema_graph_literal(graph: &SchemaGraph) -> TokenStream {
    let defs = graph.defs.iter().map(|def| {
        let id = def.id.as_str();
        let name = option_string(def.name.as_deref());
        let body = emit_schema_type(&def.body);
        quote! {
            golem_rust::schema::graph::SchemaTypeDef {
                id: golem_rust::schema::metadata::TypeId::new(#id),
                name: #name,
                body: #body,
            }
        }
    });
    let root = emit_schema_type(&graph.root);
    quote! {
        golem_rust::schema::graph::SchemaGraph {
            defs: vec![#(#defs),*],
            root: #root,
        }
    }
}

fn emit_schema_type(typ: &SchemaType) -> TokenStream {
    use SchemaType::*;

    let metadata = emit_metadata(typ.metadata());
    match typ {
        Ref { id, .. } => {
            let id = id.as_str();
            quote! { golem_rust::schema::schema_type::SchemaType::Ref {
                id: golem_rust::schema::metadata::TypeId::new(#id),
                metadata: #metadata,
            } }
        }
        Bool { .. } => {
            quote! { golem_rust::schema::schema_type::SchemaType::Bool { metadata: #metadata } }
        }
        S8 { restrictions, .. } => numeric("S8", restrictions.as_ref(), metadata),
        S16 { restrictions, .. } => numeric("S16", restrictions.as_ref(), metadata),
        S32 { restrictions, .. } => numeric("S32", restrictions.as_ref(), metadata),
        S64 { restrictions, .. } => numeric("S64", restrictions.as_ref(), metadata),
        U8 { restrictions, .. } => numeric("U8", restrictions.as_ref(), metadata),
        U16 { restrictions, .. } => numeric("U16", restrictions.as_ref(), metadata),
        U32 { restrictions, .. } => numeric("U32", restrictions.as_ref(), metadata),
        U64 { restrictions, .. } => numeric("U64", restrictions.as_ref(), metadata),
        F32 { restrictions, .. } => numeric("F32", restrictions.as_ref(), metadata),
        F64 { restrictions, .. } => numeric("F64", restrictions.as_ref(), metadata),
        Char { .. } => {
            quote! { golem_rust::schema::schema_type::SchemaType::Char { metadata: #metadata } }
        }
        String { .. } => {
            quote! { golem_rust::schema::schema_type::SchemaType::String { metadata: #metadata } }
        }
        Record { fields, .. } => {
            let fields = fields.iter().map(|field| {
                let name = &field.name;
                let body = emit_schema_type(&field.body);
                let metadata = emit_metadata(&field.metadata);
                quote! { golem_rust::schema::schema_type::NamedFieldType {
                    name: #name.to_string(), body: #body, metadata: #metadata,
                } }
            });
            quote! { golem_rust::schema::schema_type::SchemaType::Record {
                fields: vec![#(#fields),*], metadata: #metadata,
            } }
        }
        Variant { cases, .. } => {
            let cases = cases.iter().map(|case| {
                let name = &case.name;
                let payload = option_type(case.payload.as_ref());
                let metadata = emit_metadata(&case.metadata);
                quote! { golem_rust::schema::schema_type::VariantCaseType {
                    name: #name.to_string(), payload: #payload, metadata: #metadata,
                } }
            });
            quote! { golem_rust::schema::schema_type::SchemaType::Variant {
                cases: vec![#(#cases),*], metadata: #metadata,
            } }
        }
        Enum { cases, .. } => {
            let cases = string_vec(cases);
            quote! { golem_rust::schema::schema_type::SchemaType::Enum { cases: #cases, metadata: #metadata } }
        }
        Flags { flags, .. } => {
            let flags = string_vec(flags);
            quote! { golem_rust::schema::schema_type::SchemaType::Flags { flags: #flags, metadata: #metadata } }
        }
        Tuple { elements, .. } => {
            let elements = elements.iter().map(emit_schema_type);
            quote! { golem_rust::schema::schema_type::SchemaType::Tuple {
                elements: vec![#(#elements),*], metadata: #metadata,
            } }
        }
        List { element, .. } => {
            let element = emit_schema_type(element);
            quote! { golem_rust::schema::schema_type::SchemaType::List {
                element: Box::new(#element), metadata: #metadata,
            } }
        }
        FixedList {
            element, length, ..
        } => {
            let element = emit_schema_type(element);
            quote! { golem_rust::schema::schema_type::SchemaType::FixedList {
                element: Box::new(#element), length: #length, metadata: #metadata,
            } }
        }
        Map { key, value, .. } => {
            let key = emit_schema_type(key);
            let value = emit_schema_type(value);
            quote! { golem_rust::schema::schema_type::SchemaType::Map {
                key: Box::new(#key), value: Box::new(#value), metadata: #metadata,
            } }
        }
        Option { inner, .. } => {
            let inner = emit_schema_type(inner);
            quote! { golem_rust::schema::schema_type::SchemaType::Option {
                inner: Box::new(#inner), metadata: #metadata,
            } }
        }
        Result { spec, .. } => {
            let ok = option_boxed_type(spec.ok.as_deref());
            let err = option_boxed_type(spec.err.as_deref());
            quote! { golem_rust::schema::schema_type::SchemaType::Result {
                spec: golem_rust::schema::schema_type::ResultSpec { ok: #ok, err: #err },
                metadata: #metadata,
            } }
        }
        Text { restrictions, .. } => {
            let restrictions = text_restrictions(restrictions);
            quote! { golem_rust::schema::schema_type::SchemaType::Text {
                restrictions: #restrictions, metadata: #metadata,
            } }
        }
        Binary { restrictions, .. } => {
            let restrictions = binary_restrictions(restrictions);
            quote! { golem_rust::schema::schema_type::SchemaType::Binary {
                restrictions: #restrictions, metadata: #metadata,
            } }
        }
        Path { spec, .. } => {
            let spec = path_spec(spec);
            quote! { golem_rust::schema::schema_type::SchemaType::Path { spec: #spec, metadata: #metadata } }
        }
        Url { restrictions, .. } => {
            let restrictions = url_restrictions(restrictions);
            quote! { golem_rust::schema::schema_type::SchemaType::Url {
                restrictions: #restrictions, metadata: #metadata,
            } }
        }
        Datetime { .. } => {
            quote! { golem_rust::schema::schema_type::SchemaType::Datetime { metadata: #metadata } }
        }
        Duration { .. } => {
            quote! { golem_rust::schema::schema_type::SchemaType::Duration { metadata: #metadata } }
        }
        Quantity { spec, .. } => {
            let spec = quantity_spec(spec);
            quote! { golem_rust::schema::schema_type::SchemaType::Quantity { spec: #spec, metadata: #metadata } }
        }
        Union { spec, .. } => {
            let branches = spec.branches.iter().map(|branch| {
                let tag = &branch.tag;
                let body = emit_schema_type(&branch.body);
                let discriminator = discriminator(&branch.discriminator);
                let metadata = emit_metadata(&branch.metadata);
                quote! { golem_rust::schema::schema_type::UnionBranch {
                    tag: #tag.to_string(), body: #body, discriminator: #discriminator, metadata: #metadata,
                } }
            });
            quote! { golem_rust::schema::schema_type::SchemaType::Union {
                spec: golem_rust::schema::schema_type::UnionSpec { branches: vec![#(#branches),*] },
                metadata: #metadata,
            } }
        }
        Secret { spec, .. } => {
            let spec = secret_spec(spec);
            quote! { golem_rust::schema::schema_type::SchemaType::Secret { spec: #spec, metadata: #metadata } }
        }
        QuotaToken { spec, .. } => {
            let spec = quota_token_spec(spec);
            quote! { golem_rust::schema::schema_type::SchemaType::QuotaToken { spec: #spec, metadata: #metadata } }
        }
        PermissionCard { spec, .. } => {
            let polymorphic = spec.polymorphic;
            quote! { golem_rust::schema::schema_type::SchemaType::PermissionCard {
                spec: golem_rust::schema::schema_type::PermissionCardSpec { polymorphic: #polymorphic },
                metadata: #metadata,
            } }
        }
        Future { inner, .. } => {
            let inner = option_boxed_type(inner.as_deref());
            quote! { golem_rust::schema::schema_type::SchemaType::Future { inner: #inner, metadata: #metadata } }
        }
        Stream { inner, .. } => {
            let inner = option_boxed_type(inner.as_deref());
            quote! { golem_rust::schema::schema_type::SchemaType::Stream { inner: #inner, metadata: #metadata } }
        }
    }
}

fn numeric(
    name: &str,
    restrictions: Option<&NumericRestrictions>,
    metadata: TokenStream,
) -> TokenStream {
    let variant = syn::Ident::new(name, proc_macro2::Span::call_site());
    let restrictions = option_numeric_restrictions(restrictions);
    quote! { golem_rust::schema::schema_type::SchemaType::#variant {
        restrictions: #restrictions, metadata: #metadata,
    } }
}

fn option_numeric_restrictions(value: Option<&NumericRestrictions>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| {
            let min = option_bound(value.min);
            let max = option_bound(value.max);
            let unit = option_string(value.unit.as_deref());
            quote! { Some(golem_rust::schema::schema_type::NumericRestrictions {
                min: #min, max: #max, unit: #unit,
            }) }
        },
    )
}

fn option_bound(value: Option<NumericBound>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| {
            let value = match value {
                NumericBound::Signed(value) => {
                    let value = i64_literal(value);
                    quote! { golem_rust::schema::schema_type::NumericBound::Signed(#value) }
                }
                NumericBound::Unsigned(value) => {
                    quote! { golem_rust::schema::schema_type::NumericBound::Unsigned(#value) }
                }
                NumericBound::FloatBits(value) => {
                    quote! { golem_rust::schema::schema_type::NumericBound::FloatBits(#value) }
                }
            };
            quote! { Some(#value) }
        },
    )
}

fn emit_metadata(value: &MetadataEnvelope) -> TokenStream {
    let doc = option_string(value.doc.as_deref());
    let aliases = string_vec(&value.aliases);
    let examples = string_vec(&value.examples);
    let deprecated = option_string(value.deprecated.as_deref());
    let role = value.role.as_ref().map_or_else(
        || quote! { None },
        |role| {
            let role = match role {
                Role::Multimodal => quote! { golem_rust::schema::metadata::Role::Multimodal },
                Role::UnstructuredText => {
                    quote! { golem_rust::schema::metadata::Role::UnstructuredText }
                }
                Role::UnstructuredBinary => {
                    quote! { golem_rust::schema::metadata::Role::UnstructuredBinary }
                }
                Role::Other(value) => {
                    quote! { golem_rust::schema::metadata::Role::Other(#value.to_string()) }
                }
            };
            quote! { Some(#role) }
        },
    );
    quote! { golem_rust::schema::metadata::MetadataEnvelope {
        doc: #doc, aliases: #aliases, examples: #examples, deprecated: #deprecated, role: #role,
    } }
}

fn text_restrictions(value: &TextRestrictions) -> TokenStream {
    let languages = option_string_vec(value.languages.as_deref());
    let min_length = option_copy(value.min_length);
    let max_length = option_copy(value.max_length);
    let regex = option_string(value.regex.as_deref());
    quote! { golem_rust::schema::schema_type::TextRestrictions {
        languages: #languages, min_length: #min_length, max_length: #max_length, regex: #regex,
    } }
}

fn binary_restrictions(value: &BinaryRestrictions) -> TokenStream {
    let mime_types = option_string_vec(value.mime_types.as_deref());
    let min_bytes = option_copy(value.min_bytes);
    let max_bytes = option_copy(value.max_bytes);
    quote! { golem_rust::schema::schema_type::BinaryRestrictions {
        mime_types: #mime_types, min_bytes: #min_bytes, max_bytes: #max_bytes,
    } }
}

fn path_spec(value: &PathSpec) -> TokenStream {
    let direction = match value.direction {
        PathDirection::Input => quote! { golem_rust::schema::schema_type::PathDirection::Input },
        PathDirection::Output => quote! { golem_rust::schema::schema_type::PathDirection::Output },
        PathDirection::InOut => quote! { golem_rust::schema::schema_type::PathDirection::InOut },
    };
    let kind = match value.kind {
        PathKind::File => quote! { golem_rust::schema::schema_type::PathKind::File },
        PathKind::Directory => quote! { golem_rust::schema::schema_type::PathKind::Directory },
        PathKind::Any => quote! { golem_rust::schema::schema_type::PathKind::Any },
    };
    let allowed_mime_types = option_string_vec(value.allowed_mime_types.as_deref());
    let allowed_extensions = option_string_vec(value.allowed_extensions.as_deref());
    quote! { golem_rust::schema::schema_type::PathSpec {
        direction: #direction, kind: #kind,
        allowed_mime_types: #allowed_mime_types, allowed_extensions: #allowed_extensions,
    } }
}

fn url_restrictions(value: &UrlRestrictions) -> TokenStream {
    let allowed_schemes = option_string_vec(value.allowed_schemes.as_deref());
    let allowed_hosts = option_string_vec(value.allowed_hosts.as_deref());
    quote! { golem_rust::schema::schema_type::UrlRestrictions {
        allowed_schemes: #allowed_schemes, allowed_hosts: #allowed_hosts,
    } }
}

fn quantity_spec(value: &QuantitySpec) -> TokenStream {
    let base_unit = &value.base_unit;
    let allowed_suffixes = string_vec(&value.allowed_suffixes);
    let min = option_quantity(value.min.as_ref());
    let max = option_quantity(value.max.as_ref());
    quote! { golem_rust::schema::schema_type::QuantitySpec {
        base_unit: #base_unit.to_string(), allowed_suffixes: #allowed_suffixes, min: #min, max: #max,
    } }
}

fn option_quantity(value: Option<&QuantityValue>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| {
            let mantissa = i64_literal(value.mantissa);
            let scale = value.scale;
            let unit = &value.unit;
            quote! { Some(golem_rust::schema::schema_type::QuantityValue {
                mantissa: #mantissa, scale: #scale, unit: #unit.to_string(),
            }) }
        },
    )
}

fn discriminator(value: &DiscriminatorRule) -> TokenStream {
    match value {
        DiscriminatorRule::Prefix { prefix } => quote! {
            golem_rust::schema::schema_type::DiscriminatorRule::Prefix { prefix: #prefix.to_string() }
        },
        DiscriminatorRule::Suffix { suffix } => quote! {
            golem_rust::schema::schema_type::DiscriminatorRule::Suffix { suffix: #suffix.to_string() }
        },
        DiscriminatorRule::Contains { substring } => quote! {
            golem_rust::schema::schema_type::DiscriminatorRule::Contains { substring: #substring.to_string() }
        },
        DiscriminatorRule::Regex { regex } => quote! {
            golem_rust::schema::schema_type::DiscriminatorRule::Regex { regex: #regex.to_string() }
        },
        DiscriminatorRule::FieldEquals(field) => {
            let field_name = &field.field_name;
            let literal = option_string(field.literal.as_deref());
            quote! { golem_rust::schema::schema_type::DiscriminatorRule::FieldEquals(
                golem_rust::schema::schema_type::FieldDiscriminator {
                    field_name: #field_name.to_string(), literal: #literal,
                }
            ) }
        }
        DiscriminatorRule::FieldAbsent { field_name } => quote! {
            golem_rust::schema::schema_type::DiscriminatorRule::FieldAbsent {
                field_name: #field_name.to_string(),
            }
        },
    }
}

fn secret_spec(value: &SecretSpec) -> TokenStream {
    let inner = emit_schema_type(&value.inner);
    let category = option_string(value.category.as_deref());
    quote! { golem_rust::schema::schema_type::SecretSpec {
        inner: Box::new(#inner), category: #category,
    } }
}

fn quota_token_spec(value: &QuotaTokenSpec) -> TokenStream {
    let resource_name = option_string(value.resource_name.as_deref());
    quote! { golem_rust::schema::schema_type::QuotaTokenSpec { resource_name: #resource_name } }
}

fn option_type(value: Option<&SchemaType>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| {
            let value = emit_schema_type(value);
            quote! { Some(#value) }
        },
    )
}

fn option_boxed_type(value: Option<&SchemaType>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| {
            let value = emit_schema_type(value);
            quote! { Some(Box::new(#value)) }
        },
    )
}

fn option_string(value: Option<&str>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| quote! { Some(#value.to_string()) },
    )
}

fn string_vec(values: &[String]) -> TokenStream {
    quote! { vec![#(#values.to_string()),*] }
}

fn option_string_vec(values: Option<&[String]>) -> TokenStream {
    values.map_or_else(
        || quote! { None },
        |values| {
            let values = string_vec(values);
            quote! { Some(#values) }
        },
    )
}

fn option_copy<T: quote::ToTokens>(value: Option<T>) -> TokenStream {
    value.map_or_else(|| quote! { None }, |value| quote! { Some(#value) })
}

fn i64_literal(value: i64) -> TokenStream {
    if value == i64::MIN {
        quote! { i64::MIN }
    } else {
        quote! { #value }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_gen::schema_graph_test_fixture::{
        exhaustive_schema_graph, realistic_schema_graph,
    };
    use test_r::test;

    #[test]
    fn exhaustive_literal_is_deterministic_and_preserves_edges() {
        let graph = exhaustive_schema_graph();
        let literal = emit_schema_graph_literal(&graph);
        let source = literal.to_string();

        assert_eq!(source, emit_schema_graph_literal(&graph).to_string());
        syn::parse2::<syn::Expr>(literal).unwrap();
        for expected in [
            "i64 :: MIN",
            "18446744073709551615u64",
            "9223372036854775808u64",
            "mantissa : i64 :: MIN",
            "scale : - 2147483648i32",
            "length : 4294967295u32",
            "Role :: Multimodal",
            "Role :: UnstructuredText",
            "Role :: UnstructuredBinary",
            "Role :: Other",
            "DiscriminatorRule :: FieldEquals",
            "DiscriminatorRule :: FieldAbsent",
            "SchemaType :: Future",
            "SchemaType :: Stream",
            "fixture.Recursive",
        ] {
            assert!(source.contains(expected), "missing {expected}:\n{source}");
        }
        assert!(source.contains(r#"quote \" slash \\"#));
    }

    #[test]
    fn registry_deduplicates_exact_graphs_in_stable_order() {
        let realistic = realistic_schema_graph();
        let exhaustive = exhaustive_schema_graph();
        let mut registry = SchemaGraphRegistry::default();

        assert_eq!(registry.intern(realistic.clone()), 0);
        assert_eq!(registry.intern(realistic), 0);
        assert_eq!(registry.intern(exhaustive), 1);

        let definitions = registry.definitions().to_string();
        assert_eq!(definitions.matches("static").count(), 2);
        assert!(definitions.contains("__GOLEM_SCHEMA_GRAPH_0"));
        assert!(definitions.contains("__GOLEM_SCHEMA_GRAPH_1"));
        assert_eq!(
            graph_clone(0).to_string(),
            "(* __GOLEM_SCHEMA_GRAPH_0) . clone ()"
        );
    }
}
