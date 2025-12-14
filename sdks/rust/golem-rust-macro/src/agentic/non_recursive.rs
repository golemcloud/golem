use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields};

pub fn derive_non_recursive(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;
    let non_rec_name = format_ident!("{}NonRecursive", name);
    let arena_name = format_ident!("{}Arena", name);

    let trait_path = quote!(golem_rust::agentic::ArenaMember);

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => f,
            _ => panic!("NonRecursive only supports structs with named fields"),
        },
        _ => panic!("NonRecursive only supports structs"),
    };

    let field_idents: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
    let field_types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();

    let expanded = quote! {
        #[derive(Debug, Default, Clone)]
        pub struct #arena_name {
            pub nodes: Vec<#non_rec_name>
        }

        impl #arena_name {
            pub fn new() -> Self { Self::default() }

            pub fn add(&mut self, node: #non_rec_name) -> usize {
                let idx = self.nodes.len();
                self.nodes.push(node);
                idx
            }

            pub fn get(&self, index: usize) -> &#non_rec_name {
                &self.nodes[index]
            }
        }

        #[derive(Debug, Clone, golem_rust::Schema)]
        pub struct #non_rec_name {
            #(pub #field_idents: <#field_types as #trait_path<#arena_name>>::NonRecursive,)*
        }

        impl #name {

            pub fn to_arena(&self, arena: &mut #arena_name) -> usize {
                let flattened = #non_rec_name {
                    #(#field_idents: #trait_path::<#arena_name>::deflate(&self.#field_idents, arena),)*
                };
                arena.add(flattened)
            }


            pub fn from_arena(index: usize, arena: &#arena_name) -> Self {
                let node = arena.get(index);
                Self {
                    #(#field_idents: #trait_path::<#arena_name>::inflate(node.#field_idents.clone(), arena),)*
                }
            }
        }

        impl #trait_path<#arena_name> for #name {
            type NonRecursive = usize;

            fn deflate(&self, arena: &mut #arena_name) -> Self::NonRecursive {
                self.to_arena(arena)
            }

            fn inflate(index: Self::NonRecursive, arena: &#arena_name) -> Self {
                Self::from_arena(index, arena)
            }
        }
    };

    TokenStream::from(expanded)
}
