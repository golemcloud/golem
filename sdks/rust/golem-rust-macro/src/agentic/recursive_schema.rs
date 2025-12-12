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

use proc_macro2::Ident;
use quote::quote;
use syn::spanned::Spanned;
use syn::{DeriveInput, Fields, Type};

pub fn is_recursive(ast: &DeriveInput) -> bool {
    let self_ident = &ast.ident;

    match &ast.data {
        syn::Data::Struct(ds) => ds
            .fields
            .iter()
            .any(|f| type_contains_self(&f.ty, self_ident)),
        syn::Data::Enum(de) => de.variants.iter().any(|v| {
            v.fields
                .iter()
                .any(|f| type_contains_self(&f.ty, self_ident))
        }),
        syn::Data::Union(_) => false,
    }
}
pub fn generate_to_generic(
    ast: &DeriveInput,
    self_ident: &Ident,
    crate_ident: &Ident,
) -> proc_macro2::TokenStream {
    match &ast.data {
        syn::Data::Struct(ds) => {
            let field_graphs = ds.fields.iter().enumerate().map(|(i, f)| {
                let var_ident = f
                    .ident
                    .clone()
                    .unwrap_or_else(|| syn::Ident::new(&format!("field{}", i), f.span()));
                let field_name_str = var_ident.to_string();

                quote! {
                    let child_index = #crate_ident::agentic::ToGenericData::to_generic(&self.#var_ident, graph);
                    field_indices.push((#field_name_str.to_string(), child_index));
                }
            });

            quote! {
                impl #crate_ident::agentic::ToGenericData for #self_ident {
                    fn to_generic(&self, graph: &mut #crate_ident::agentic::GenericData) -> usize {
                        let mut field_indices = Vec::new();
                        #(#field_graphs)*

                        let index = graph.nodes.len();
                        graph.nodes.push(
                            #crate_ident::agentic::GraphNode::Struct(
                                #crate_ident::agentic::StructNode {
                                    fields: field_indices
                                }
                            )
                        );
                        index
                    }
                }
            }
        }

        syn::Data::Enum(de) => {
            let variant_matches = de.variants.iter().map(|v| {
                let vname = &v.ident;
                let vname_str = vname.to_string();

                match &v.fields {
                    Fields::Unit => quote! {
                        Self::#vname => {
                            let index = graph.nodes.len();
                            graph.nodes.push(
                                #crate_ident::agentic::GraphNode::Enum(
                                    #crate_ident::agentic::EnumNode {
                                        variant: #vname_str.to_string(),
                                        payload: None,
                                    }
                                )
                            );
                            index
                        }
                    },

                    Fields::Unnamed(fields) => {
                        // TODO; resivit this may be
                        let n = fields.unnamed.len();
                        let bindings: Vec<_> = (0..n)
                            .map(|i| syn::Ident::new(&format!("f{}", i), v.span()))
                            .collect();

                        if n == 1 {

                            let b = &bindings[0];
                            quote! {
                                Self::#vname(ref #b) => {
                                    let child_idx = #crate_ident::agentic::ToGenericData::to_generic(&#b, graph);
                                    let index = graph.nodes.len();
                                    graph.nodes.push(
                                        #crate_ident::agentic::GraphNode::Enum(
                                            #crate_ident::agentic::EnumNode {
                                                variant: #vname_str.to_string(),
                                                payload: Some(child_idx),
                                            }
                                        )
                                    );
                                    index
                                }
                            }
                        } else {

                            let pushes: Vec<proc_macro2::TokenStream> = bindings.iter().map(|b| {
                                quote! {
                                    children.push(#crate_ident::agentic::ToGenericData::to_generic(&#b, graph));
                                }
                            }).collect();

                            quote! {
                                Self::#vname( #(#bindings),* ) => {

                                    let mut children: Vec<usize> = Vec::new();
                                    #(#pushes)*


                                    let seq_idx = graph.nodes.len();
                                    graph.nodes.push(
                                        #crate_ident::agentic::GraphNode::Seq(
                                            #crate_ident::agentic::SeqNode { elements: children }
                                        )
                                    );


                                    let index = graph.nodes.len();
                                    graph.nodes.push(
                                        #crate_ident::agentic::GraphNode::Enum(
                                            #crate_ident::agentic::EnumNode {
                                                variant: #vname_str.to_string(),
                                                payload: Some(seq_idx),
                                            }
                                        )
                                    );
                                    index
                                }
                            }
                        }
                    }


                    Fields::Named(fields_named) => {
                        let bindings: Vec<_> =
                            fields_named.named.iter().map(|f| f.ident.clone().unwrap()).collect();

                        let to_generic_children = bindings.iter().map(|b| {
                            let name = b.to_string();

                            quote! {
                                fields.push( (#name.to_string(), #crate_ident::agentic::ToGenericData::to_generic(&#b, graph)) );
                            }
                        });

                        quote! {
                            Self::#vname { #(#bindings),* } => {

                                let mut fields: Vec<(String, usize)> = Vec::new();
                                #(#to_generic_children)*

                                let struct_index = graph.nodes.len();
                                graph.nodes.push(
                                    #crate_ident::agentic::GraphNode::Struct(
                                        #crate_ident::agentic::StructNode { fields }
                                    )
                                );

                                let index = graph.nodes.len();
                                graph.nodes.push(
                                    #crate_ident::agentic::GraphNode::Enum(
                                        #crate_ident::agentic::EnumNode {
                                            variant: #vname_str.to_string(),
                                            payload: Some(struct_index),
                                        }
                                    )
                                );
                                index
                            }
                        }
                    }
                }
            });

            quote! {
                impl #crate_ident::agentic::ToGenericData for #self_ident {
                    fn to_generic(&self, graph: &mut #crate_ident::agentic::GenericData) -> usize {
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

pub fn generate_from_generic(
    ast: &DeriveInput,
    self_ident: &Ident,
    crate_ident: &Ident,
) -> proc_macro2::TokenStream {
    match &ast.data {
        syn::Data::Struct(ds) => {
            let field_assigns = ds.fields.iter().enumerate().map(|(i, f)| {
                let var_ident = f.ident.clone().unwrap_or_else(|| syn::Ident::new(&format!("field{}", i), f.span()));
                quote! { #var_ident: #crate_ident::agentic::FromGenericData::from_generic(graph, indices[#i])? }
            });

            quote! {
                impl #crate_ident::agentic::FromGenericData for #self_ident {
                    fn from_generic(graph: &#crate_ident::agentic::GenericData, index: usize) -> Result<Self, String> {
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
                            quote! { #crate_ident::agentic::FromGenericData::from_generic(graph, payload_index)? }
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
                            quote! { #fname: #crate_ident::agentic::FromGenericData::from_generic(graph, payload_indices[#i])? }
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
                impl #crate_ident::agentic::FromGenericData for #self_ident {
                    fn from_generic(graph: &#crate_ident::agentic::GenericData, index: usize) -> Result<Self, String> {
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
