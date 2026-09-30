// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::wit::{encode_graph, wire};
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
                static #name: std::sync::LazyLock<golem_rust::schema::wit::wire::SchemaGraph> =
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

pub(crate) fn emit_schema_graph_literal(graph: &SchemaGraph) -> TokenStream {
    emit_wire_graph(&encode_graph(graph).expect("validated schema graph must be encodable"))
}

fn graph_ident(index: usize) -> syn::Ident {
    syn::Ident::new(
        &format!("__GOLEM_SCHEMA_GRAPH_{index}"),
        proc_macro2::Span::call_site(),
    )
}

fn emit_wire_graph(graph: &wire::SchemaGraph) -> TokenStream {
    let nodes = graph.type_nodes.iter().map(emit_node);
    let defs = graph.defs.iter().map(|def| {
        let id = &def.id;
        let name = option_string(def.name.as_deref());
        let body = def.body;
        quote! { golem_rust::schema::wit::wire::SchemaTypeDef {
            id: #id.to_string(), name: #name, body: #body,
        } }
    });
    let root = graph.root;
    quote! { golem_rust::schema::wit::wire::SchemaGraph {
        type_nodes: vec![#(#nodes),*], defs: vec![#(#defs),*], root: #root,
    } }
}

fn emit_node(node: &wire::SchemaTypeNode) -> TokenStream {
    let body = emit_body(&node.body);
    let metadata = emit_metadata(&node.metadata);
    quote! { golem_rust::schema::wit::wire::SchemaTypeNode {
        body: #body, metadata: #metadata,
    } }
}

fn emit_body(body: &wire::SchemaTypeBody) -> TokenStream {
    use wire::SchemaTypeBody::*;
    let path = quote! { golem_rust::schema::wit::wire::SchemaTypeBody };
    match body {
        RefType(value) => quote! { #path::RefType(#value) },
        BoolType => quote! { #path::BoolType },
        S8Type(value) => numeric_body("S8Type", value),
        S16Type(value) => numeric_body("S16Type", value),
        S32Type(value) => numeric_body("S32Type", value),
        S64Type(value) => numeric_body("S64Type", value),
        U8Type(value) => numeric_body("U8Type", value),
        U16Type(value) => numeric_body("U16Type", value),
        U32Type(value) => numeric_body("U32Type", value),
        U64Type(value) => numeric_body("U64Type", value),
        F32Type(value) => numeric_body("F32Type", value),
        F64Type(value) => numeric_body("F64Type", value),
        CharType => quote! { #path::CharType },
        StringType => quote! { #path::StringType },
        RecordType(fields) => {
            let fields = fields.iter().map(emit_field);
            quote! { #path::RecordType(vec![#(#fields),*]) }
        }
        VariantType(cases) => {
            let cases = cases.iter().map(emit_case);
            quote! { #path::VariantType(vec![#(#cases),*]) }
        }
        EnumType(values) => {
            let values = string_vec(values);
            quote! { #path::EnumType(#values) }
        }
        FlagsType(values) => {
            let values = string_vec(values);
            quote! { #path::FlagsType(#values) }
        }
        TupleType(values) => quote! { #path::TupleType(vec![#(#values),*]) },
        ListType(value) => quote! { #path::ListType(#value) },
        FixedListType(value) => {
            let element = value.element;
            let length = value.length;
            quote! { #path::FixedListType(golem_rust::schema::wit::wire::FixedListSpec {
                element: #element, length: #length,
            }) }
        }
        MapType(value) => {
            let key = value.key;
            let val = value.value;
            quote! { #path::MapType(golem_rust::schema::wit::wire::MapSpec { key: #key, value: #val }) }
        }
        OptionType(value) => quote! { #path::OptionType(#value) },
        ResultType(value) => {
            let ok = option_copy(value.ok);
            let err = option_copy(value.err);
            quote! { #path::ResultType(golem_rust::schema::wit::wire::ResultSpec { ok: #ok, err: #err }) }
        }
        TextType(value) => {
            let value = text_restrictions(value);
            quote! { #path::TextType(#value) }
        }
        BinaryType(value) => {
            let value = binary_restrictions(value);
            quote! { #path::BinaryType(#value) }
        }
        PathType(value) => {
            let value = path_spec(value);
            quote! { #path::PathType(#value) }
        }
        UrlType(value) => {
            let value = url_restrictions(value);
            quote! { #path::UrlType(#value) }
        }
        DatetimeType => quote! { #path::DatetimeType },
        DurationType => quote! { #path::DurationType },
        QuantityType(value) => {
            let value = quantity_spec(value);
            quote! { #path::QuantityType(#value) }
        }
        UnionType(value) => {
            let branches = value.branches.iter().map(emit_branch);
            quote! { #path::UnionType(golem_rust::schema::wit::wire::UnionSpec { branches: vec![#(#branches),*] }) }
        }
        SecretType(value) => {
            let inner = value.inner;
            let category = option_string(value.category.as_deref());
            quote! { #path::SecretType(golem_rust::schema::wit::wire::SecretSpec { inner: #inner, category: #category }) }
        }
        QuotaTokenType(value) => {
            let resource_name = option_string(value.resource_name.as_deref());
            quote! { #path::QuotaTokenType(golem_rust::schema::wit::wire::QuotaTokenSpec { resource_name: #resource_name }) }
        }
        PermissionCardType(value) => {
            let polymorphic = value.polymorphic;
            quote! { #path::PermissionCardType(golem_rust::schema::wit::wire::PermissionCardSpec { polymorphic: #polymorphic }) }
        }
        FutureType(value) => {
            let value = option_copy(*value);
            quote! { #path::FutureType(#value) }
        }
        StreamType(value) => {
            let value = option_copy(*value);
            quote! { #path::StreamType(#value) }
        }
    }
}

fn emit_field(value: &wire::NamedFieldType) -> TokenStream {
    let name = &value.name;
    let body = value.body;
    let metadata = emit_metadata(&value.metadata);
    quote! { golem_rust::schema::wit::wire::NamedFieldType {
        name: #name.to_string(), body: #body, metadata: #metadata,
    } }
}

fn emit_case(value: &wire::VariantCaseType) -> TokenStream {
    let name = &value.name;
    let payload = option_copy(value.payload);
    let metadata = emit_metadata(&value.metadata);
    quote! { golem_rust::schema::wit::wire::VariantCaseType {
        name: #name.to_string(), payload: #payload, metadata: #metadata,
    } }
}

fn emit_branch(value: &wire::UnionBranch) -> TokenStream {
    let tag = &value.tag;
    let body = value.body;
    let discriminator = emit_discriminator(&value.discriminator);
    let metadata = emit_metadata(&value.metadata);
    quote! { golem_rust::schema::wit::wire::UnionBranch {
        tag: #tag.to_string(), body: #body, discriminator: #discriminator, metadata: #metadata,
    } }
}

fn numeric_body(name: &str, value: &Option<wire::NumericRestrictions>) -> TokenStream {
    let variant = syn::Ident::new(name, proc_macro2::Span::call_site());
    let value = value.as_ref().map_or_else(
        || quote! { None },
        |value| {
            let value = numeric_restrictions(value);
            quote! { Some(#value) }
        },
    );
    quote! { golem_rust::schema::wit::wire::SchemaTypeBody::#variant(#value) }
}

fn numeric_restrictions(value: &wire::NumericRestrictions) -> TokenStream {
    let min = option_bound(value.min.as_ref());
    let max = option_bound(value.max.as_ref());
    let unit = option_string(value.unit.as_deref());
    quote! { golem_rust::schema::wit::wire::NumericRestrictions { min: #min, max: #max, unit: #unit } }
}

fn option_bound(value: Option<&wire::NumericBound>) -> TokenStream {
    value.map_or_else(
        || quote! { None },
        |value| {
            let value = match value {
                wire::NumericBound::Signed(value) => {
                    let value = i64_literal(*value);
                    quote! { golem_rust::schema::wit::wire::NumericBound::Signed(#value) }
                }
                wire::NumericBound::Unsigned(value) => {
                    quote! { golem_rust::schema::wit::wire::NumericBound::Unsigned(#value) }
                }
                wire::NumericBound::FloatBits(value) => {
                    quote! { golem_rust::schema::wit::wire::NumericBound::FloatBits(#value) }
                }
            };
            quote! { Some(#value) }
        },
    )
}

fn emit_metadata(value: &wire::MetadataEnvelope) -> TokenStream {
    let doc = option_string(value.doc.as_deref());
    let aliases = string_vec(&value.aliases);
    let examples = string_vec(&value.examples);
    let deprecated = option_string(value.deprecated.as_deref());
    let role = value.role.as_ref().map_or_else(
        || quote! { None },
        |role| {
            let role = match role {
                wire::Role::Multimodal => {
                    quote! { golem_rust::schema::wit::wire::Role::Multimodal }
                }
                wire::Role::UnstructuredText => {
                    quote! { golem_rust::schema::wit::wire::Role::UnstructuredText }
                }
                wire::Role::UnstructuredBinary => {
                    quote! { golem_rust::schema::wit::wire::Role::UnstructuredBinary }
                }
                wire::Role::Other(value) => {
                    quote! { golem_rust::schema::wit::wire::Role::Other(#value.to_string()) }
                }
            };
            quote! { Some(#role) }
        },
    );
    quote! { golem_rust::schema::wit::wire::MetadataEnvelope {
        doc: #doc, aliases: #aliases, examples: #examples, deprecated: #deprecated, role: #role,
    } }
}

fn text_restrictions(value: &wire::TextRestrictions) -> TokenStream {
    let languages = option_string_vec(value.languages.as_deref());
    let min_length = option_copy(value.min_length);
    let max_length = option_copy(value.max_length);
    let regex = option_string(value.regex.as_deref());
    quote! { golem_rust::schema::wit::wire::TextRestrictions {
        languages: #languages, min_length: #min_length, max_length: #max_length, regex: #regex,
    } }
}

fn binary_restrictions(value: &wire::BinaryRestrictions) -> TokenStream {
    let mime_types = option_string_vec(value.mime_types.as_deref());
    let min_bytes = option_copy(value.min_bytes);
    let max_bytes = option_copy(value.max_bytes);
    quote! { golem_rust::schema::wit::wire::BinaryRestrictions {
        mime_types: #mime_types, min_bytes: #min_bytes, max_bytes: #max_bytes,
    } }
}

fn path_spec(value: &wire::PathSpec) -> TokenStream {
    let direction = match value.direction {
        wire::PathDirection::Input => {
            quote! { golem_rust::schema::wit::wire::PathDirection::Input }
        }
        wire::PathDirection::Output => {
            quote! { golem_rust::schema::wit::wire::PathDirection::Output }
        }
        wire::PathDirection::InOut => {
            quote! { golem_rust::schema::wit::wire::PathDirection::InOut }
        }
    };
    let kind = match value.kind {
        wire::PathKind::File => quote! { golem_rust::schema::wit::wire::PathKind::File },
        wire::PathKind::Directory => quote! { golem_rust::schema::wit::wire::PathKind::Directory },
        wire::PathKind::Any => quote! { golem_rust::schema::wit::wire::PathKind::Any },
    };
    let allowed_mime_types = option_string_vec(value.allowed_mime_types.as_deref());
    let allowed_extensions = option_string_vec(value.allowed_extensions.as_deref());
    quote! { golem_rust::schema::wit::wire::PathSpec { direction: #direction, kind: #kind, allowed_mime_types: #allowed_mime_types, allowed_extensions: #allowed_extensions } }
}

fn url_restrictions(value: &wire::UrlRestrictions) -> TokenStream {
    let allowed_schemes = option_string_vec(value.allowed_schemes.as_deref());
    let allowed_hosts = option_string_vec(value.allowed_hosts.as_deref());
    quote! { golem_rust::schema::wit::wire::UrlRestrictions { allowed_schemes: #allowed_schemes, allowed_hosts: #allowed_hosts } }
}

fn quantity_spec(value: &wire::QuantitySpec) -> TokenStream {
    let base_unit = &value.base_unit;
    let allowed_suffixes = string_vec(&value.allowed_suffixes);
    let min = option_quantity(value.min.as_ref());
    let max = option_quantity(value.max.as_ref());
    quote! { golem_rust::schema::wit::wire::QuantitySpec { base_unit: #base_unit.to_string(), allowed_suffixes: #allowed_suffixes, min: #min, max: #max } }
}

fn option_quantity(value: Option<&wire::QuantityValue>) -> TokenStream {
    value.map_or_else(|| quote! { None }, |value| { let mantissa = i64_literal(value.mantissa); let scale = value.scale; let unit = &value.unit;
        quote! { Some(golem_rust::schema::wit::wire::QuantityValue { mantissa: #mantissa, scale: #scale, unit: #unit.to_string() }) }
    })
}

fn emit_discriminator(value: &wire::DiscriminatorRule) -> TokenStream {
    let path = quote! { golem_rust::schema::wit::wire::DiscriminatorRule };
    match value {
        wire::DiscriminatorRule::Prefix(value) => quote! { #path::Prefix(#value.to_string()) },
        wire::DiscriminatorRule::Suffix(value) => quote! { #path::Suffix(#value.to_string()) },
        wire::DiscriminatorRule::Contains(value) => quote! { #path::Contains(#value.to_string()) },
        wire::DiscriminatorRule::Regex(value) => quote! { #path::Regex(#value.to_string()) },
        wire::DiscriminatorRule::FieldEquals(value) => {
            let field_name = &value.field_name;
            let literal = option_string(value.literal.as_deref());
            quote! { #path::FieldEquals(golem_rust::schema::wit::wire::FieldDiscriminator { field_name: #field_name.to_string(), literal: #literal }) }
        }
        wire::DiscriminatorRule::FieldAbsent(value) => {
            quote! { #path::FieldAbsent(#value.to_string()) }
        }
    }
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
    fn exhaustive_wire_literal_is_deterministic_and_preserves_edges() {
        let graph = encode_graph(&exhaustive_schema_graph()).unwrap();
        let literal = emit_wire_graph(&graph);
        let source = literal.to_string();
        assert_eq!(source, emit_wire_graph(&graph).to_string());
        syn::parse2::<syn::Expr>(literal).unwrap();
        for expected in [
            "SchemaGraph",
            "type_nodes",
            "RefType",
            "NumericRestrictions",
            "Role :: Multimodal",
            "FieldEquals",
            "FutureType",
            "StreamType",
            "fixture.Recursive",
        ] {
            assert!(source.contains(expected), "missing {expected}:\n{source}");
        }
        assert!(!source.contains("schema :: graph :: SchemaGraph"));
        assert!(!source.contains("encode_graph"));
    }

    #[test]
    fn registry_emits_flat_wire_graphs_and_deduplicates_in_stable_order() {
        let realistic = realistic_schema_graph();
        let exhaustive = exhaustive_schema_graph();
        let mut registry = SchemaGraphRegistry::default();
        assert_eq!(registry.intern(realistic.clone()), 0);
        assert_eq!(registry.intern(realistic), 0);
        assert_eq!(registry.intern(exhaustive), 1);
        let definitions = registry.definitions().to_string();
        assert_eq!(definitions.matches("static").count(), 2);
        assert!(definitions.contains("wire :: SchemaGraph"));
        assert!(!definitions.contains("schema :: graph :: SchemaGraph"));
        assert_eq!(
            graph_clone(0).to_string(),
            "(* __GOLEM_SCHEMA_GRAPH_0) . clone ()"
        );
    }
}
