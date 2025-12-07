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

use crate::value;
use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Type, Fields};
use syn::spanned::Spanned;

pub fn derive_schema(input: TokenStream, golem_rust_crate_ident: &Ident) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let self_ident = &ast.ident;

    let is_recursive = match &ast.data {
        syn::Data::Struct(ds) => ds.fields.iter().any(|f| type_contains_self(&f.ty, self_ident)),
        syn::Data::Enum(de) => de.variants.iter().any(|v| v.fields.iter().any(|f| type_contains_self(&f.ty, self_ident))),
        syn::Data::Union(_) => false,
    };

    if is_recursive {
        let to_graph_impl = generate_to_graph(&ast, self_ident, golem_rust_crate_ident);
        let from_graph_impl = generate_from_graph(&ast, self_ident, golem_rust_crate_ident);

        let value_impl = quote! {
            impl #golem_rust_crate_ident::value_and_type::IntoValue for #self_ident {
                fn add_to_builder<B: #golem_rust_crate_ident::value_and_type::NodeBuilder>(self, builder: B) -> B::Result {
                    use #golem_rust_crate_ident::agentic::ToGraph;

                    let mut graph = #golem_rust_crate_ident::agentic::Graph { nodes: vec![], root: 0 };
                    let result_index = self.to_graph(&mut graph);
                    graph.root = result_index;
                    graph.add_to_builder(builder)
                }

                fn add_to_type_builder<B: #golem_rust_crate_ident::value_and_type::TypeNodeBuilder>(builder: B) -> B::Result {
                    #golem_rust_crate_ident::agentic::Graph::add_to_type_builder(builder)
                }
            }

            impl #golem_rust_crate_ident::value_and_type::FromValueAndType for #self_ident {
                fn from_extractor<'a, 'b>(
                    extractor: &'a impl #golem_rust_crate_ident::value_and_type::WitValueExtractor<'a, 'b>,
                ) -> Result<Self, String> {
                    use #golem_rust_crate_ident::agentic::FromGraph;

                    let graph = #golem_rust_crate_ident::agentic::Graph::from_extractor(extractor)?;
                    Self::from_graph(&graph, graph.root)
                }
            }
        };

        quote! {
            #to_graph_impl
            #from_graph_impl
            #value_impl
        }
            .into()
    } else {
        let into_value_tokens: proc_macro2::TokenStream =
            value::derive_into_value(&ast, golem_rust_crate_ident).into();
        let from_value_tokens: proc_macro2::TokenStream =
            value::derive_from_value_and_type(&ast, golem_rust_crate_ident).into();

        quote! {
            #into_value_tokens
            #from_value_tokens
        }
            .into()
    }
}

fn generate_to_graph(ast: &DeriveInput, self_ident: &Ident, crate_ident: &Ident) -> proc_macro2::TokenStream {
    match &ast.data {
        syn::Data::Struct(ds) => {
            let field_graphs = ds.fields.iter().enumerate().map(|(i, f)| {
                let var_ident = f.ident.clone().unwrap_or_else(|| syn::Ident::new(&format!("field{}", i), f.span()));
                let field_name_str = var_ident.to_string();
                quote! {
                    let #var_ident = #crate_ident::agentic::ToGraph::to_graph(&self.#var_ident, graph);
                    field_indices.push((#field_name_str.to_string(), #var_ident));
                }
            });

            quote! {
                impl #crate_ident::agentic::ToGraph for #self_ident {
                    fn to_graph(&self, graph: &mut #crate_ident::agentic::Graph) -> usize {
                        let mut field_indices = Vec::new();
                        #(#field_graphs)*
                        let index = graph.nodes.len();
                        graph.nodes.push(#crate_ident::agentic::GraphNode::Struct(
                            #crate_ident::agentic::StructNode { fields: field_indices }
                        ));
                        index
                    }
                }
            }
        }
        syn::Data::Enum(de) => {
            let variant_matches = de.variants.iter().map(|v| {
                let vname = &v.ident;
                match &v.fields {
                    Fields::Unit => quote! {
                        Self::#vname => {
                            let payload_index = None;
                            graph.nodes.push(#crate_ident::agentic::GraphNode::Enum(
                                #crate_ident::agentic::EnumNode {
                                    variant: stringify!(#vname).to_string(),
                                    payload: payload_index,
                                }
                            ));

                            graph.nodes.len() - 1
                        }
                    },
                    Fields::Unnamed(fields_unnamed) => {
                        let bindings: Vec<syn::Ident> = (0..fields_unnamed.unnamed.len())
                            .map(|i| syn::Ident::new(&format!("f{}", i), v.span()))
                            .collect();

                        let to_graph_calls: Vec<_> = bindings.iter()
                            .map(|b| quote! { #crate_ident::agentic::ToGraph::to_graph(#b, graph) })
                            .collect();

                        let payload_expr = if bindings.len() == 1 {
                            quote! { Some(#(#to_graph_calls),*) }
                        } else {
                            quote! {
                                let mut elements = Vec::new();
                                #(elements.push(#to_graph_calls);)*
                                Some(elements[0])
                            }
                        };

                        quote! {
                            Self::#vname(#(#bindings),*) => {
                                let payload_index = #payload_expr;
                                graph.nodes.push(#crate_ident::agentic::GraphNode::Enum(
                                    #crate_ident::agentic::EnumNode {
                                        variant: stringify!(#vname).to_string(),
                                        payload: payload_index,
                                    }
                                ));

                                graph.nodes.len() - 1
                            }
                        }
                    },
                    Fields::Named(fields_named) => {
                        let bindings: Vec<_> = fields_named.named.iter().map(|f| f.ident.clone().unwrap()).collect();
                        let to_graph_calls = bindings.iter().map(|b| quote! { #crate_ident::agentic::ToGraph::to_graph(#b, graph) });
                        quote! {
                            Self::#vname { #(#bindings),* } => {
                                let mut elements = Vec::new();
                                #(elements.push(#to_graph_calls);)*
                                let payload_index = Some(elements[0]);
                                graph.nodes.push(#crate_ident::agentic::GraphNode::Enum(
                                    #crate_ident::agentic::EnumNode {
                                        variant: stringify!(#vname).to_string(),
                                        payload: payload_index,
                                    }
                                ));

                                graph.nodes.len() - 1
                            }
                        }
                    },
                }
            });

            quote! {
                impl #crate_ident::agentic::ToGraph for #self_ident {
                    fn to_graph(&self, graph: &mut #crate_ident::agentic::Graph) -> usize {
                        match self {
                            #(#variant_matches),*
                        }
                    }
                }
            }
        }
        _ => quote! {},
    }
}

fn generate_from_graph(ast: &DeriveInput, self_ident: &Ident, crate_ident: &Ident) -> proc_macro2::TokenStream {
    match &ast.data {
        syn::Data::Struct(ds) => {
            let field_assigns = ds.fields.iter().enumerate().map(|(i, f)| {
                let var_ident = f.ident.clone().unwrap_or_else(|| syn::Ident::new(&format!("field{}", i), f.span()));
                quote! { #var_ident: #crate_ident::agentic::FromGraph::from_graph(graph, indices[#i])? }
            });

            quote! {
                impl #crate_ident::agentic::FromGraph for #self_ident {
                    fn from_graph(graph: &#crate_ident::agentic::Graph, index: usize) -> Result<Self, String> {
                        match &graph.nodes[index] {
                            #crate_ident::agentic::GraphNode::Struct(struct_node) => {
                                let indices: Vec<usize> = struct_node.fields.iter().map(|(_, idx)| *idx).collect();
                                Ok(Self { #(#field_assigns),* })
                            }
                            _ => Err(format!("Expected Struct node at index {}", index))
                        }
                    }
                }
            }
        }
        syn::Data::Enum(de) => {
            let variant_matches = de.variants.iter().map(|v| {
                let vname = &v.ident;
                match &v.fields {
                    Fields::Unit => quote! {
                        stringify!(#vname) => Ok(Self::#vname),
                    },
                    Fields::Unnamed(fields_unnamed) => {
                        let n = fields_unnamed.unnamed.len();
                        let unwrap_call = if n == 1 {
                            quote! { #crate_ident::agentic::FromGraph::from_graph(graph, payload_index)? }
                        } else {
                            quote! { unimplemented!("Multiple tuple fields not implemented yet") }
                        };
                        quote! {
                            stringify!(#vname) => Ok(Self::#vname(#unwrap_call)),
                        }
                    },
                    Fields::Named(fields_named) => {
                        let field_assigns = fields_named.named.iter().enumerate().map(|(i, f)| {
                            let fname = f.ident.clone().unwrap();
                            quote! { #fname: #crate_ident::agentic::FromGraph::from_graph(graph, payload_indices[#i])? }
                        });
                        quote! {
                            stringify!(#vname) => {
                                let payload_indices = vec![payload_index];
                                Ok(Self::#vname { #(#field_assigns),* })
                            }
                        }
                    },
                }
            });

            quote! {
                impl #crate_ident::agentic::FromGraph for #self_ident {
                    fn from_graph(graph: &#crate_ident::agentic::Graph, index: usize) -> Result<Self, String> {
                        match &graph.nodes[index] {
                            #crate_ident::agentic::GraphNode::Enum(enum_node) => {
                                let payload_index = enum_node.payload.ok_or("Missing payload")?;
                                match enum_node.variant.as_str() {
                                    #(#variant_matches)*
                                    other => Err(format!("Unknown variant: {}", other))
                                }
                            }
                            _ => Err(format!("Expected Enum node at index {}", index))
                        }
                    }
                }
            }
        }
        _ => quote! {},
    }
}

fn type_contains_self(ty: &Type, self_ident: &Ident) -> bool {
    match ty {
        Type::Path(type_path) => {
            let is_direct = type_path
                .path
                .segments
                .last()
                .map(|seg| seg.ident == *self_ident)
                .unwrap_or(false);

            let has_generic_self = type_path
                .path
                .segments
                .iter()
                .any(|seg| match &seg.arguments {
                    syn::PathArguments::AngleBracketed(ab) => ab.args.iter().any(|arg| match arg {
                        syn::GenericArgument::Type(t) => type_contains_self(t, self_ident),
                        _ => false,
                    }),
                    _ => false,
                });

            is_direct || has_generic_self
        }
        Type::Reference(r) => type_contains_self(&r.elem, self_ident),
        Type::Ptr(p) => type_contains_self(&p.elem, self_ident),
        Type::Slice(s) => type_contains_self(&s.elem, self_ident),
        Type::Array(a) => type_contains_self(&a.elem, self_ident),
        Type::Paren(p) => type_contains_self(&p.elem, self_ident),
        Type::Tuple(t) => t.elems.iter().any(|e| type_contains_self(e, self_ident)),
        Type::Group(g) => type_contains_self(&g.elem, self_ident),
        _ => false,
    }
}


