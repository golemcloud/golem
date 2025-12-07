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

                    let mut graph = #golem_rust_crate_ident::agentic::Graph { nodes: vec![] };
                    let _ = self.to_graph(&mut graph);
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
                    Self::from_graph(&graph, 0)
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
                let name = f.ident.as_ref().map(|id| id.to_string()).unwrap_or(format!("field{}", i));
                let var_ident = f.ident.clone().unwrap_or_else(|| syn::Ident::new(&format!("field{}", i), f.span()));
                let field_name_str = var_ident.to_string();


                quote! {
                    let #name = #crate_ident::agentic::ToGraph::to_graph(&self.#name, graph);
                    field_indices.push((#field_name_str.to_string(), #name));
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
            let variants_graphs = de.variants.iter().map(|v| {
                let vname = &v.ident;
                let payload = match &v.fields {
                    Fields::Named(fields_named) => {
                        let assignments = fields_named.named.iter().map(|f| {
                            let fname = &f.ident;
                            quote! { #fname: #crate_ident::agentic::ToGraph::to_graph(&inner.#fname, graph) }
                        });
                        quote! {
                            let inner = match self { Self::#vname { #( #assignments ),* } => inner, _ => unreachable!() };
                        }
                    }
                    Fields::Unnamed(fields_unnamed) => {
                        let assignments = fields_unnamed.unnamed.iter().enumerate().map(|(i, _)| {
                            let idx = syn::Index::from(i);
                            quote! { #crate_ident::agentic::ToGraph::to_graph(&inner.#idx, graph) }
                        });
                        quote! {}
                    }
                    Fields::Unit => quote! {},
                };
                quote! { /* per variant logic */ }
            });

            quote! {
                impl #crate_ident::agentic::ToGraph for #self_ident {
                    fn to_graph(&self, graph: &mut #crate_ident::agentic::Graph) -> usize {
                        unimplemented!("Recursive enum ToGraph generation not implemented fully")
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
            let field_graphs = ds.fields.iter().enumerate().map(|(i, f)| {
                let name = f.ident.as_ref().map(|id| id.clone()).unwrap_or_else(|| syn::Ident::new(&format!("field{}", i), f.span()));
                let ty = &f.ty;
                quote! {
                    #name: #crate_ident::agentic::FromGraph::from_graph(graph, indices[#i])?
                }
            });

            quote! {
                impl #crate_ident::agentic::FromGraph for #self_ident {
                    fn from_graph(graph: &#crate_ident::agentic::Graph, index: usize) -> Result<Self, String> {
                        let node = &graph.nodes[index];
                        match node {
                            #crate_ident::agentic::GraphNode::Struct(struct_node) => {
                                let indices: Vec<usize> = struct_node.fields.iter().map(|(_, idx)| *idx).collect();
                                Ok(Self {
                                    #(#field_graphs),*
                                })
                            }
                            _ => Err(format!("Expected Struct node at index {}", index))
                        }
                    }
                }
            }
        }
        syn::Data::Enum(de) => {
            quote! {
                impl #crate_ident::agentic::FromGraph for #self_ident {
                    fn from_graph(graph: &#crate_ident::agentic::Graph, index: usize) -> Result<Self, String> {
                        unimplemented!("Recursive enum FromGraph generation not implemented fully")
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
