use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields
};

pub fn derive_non_recursive(input: &DeriveInput) -> TokenStream {
    let name = &input.ident;
    let non_rec_name = format_ident!("{}NonRecursive", name);
    let arena_name = format_ident!("{}Arena", name);

    let fields = if let Data::Struct(s) = &input.data {
        if let Fields::Named(f) = &s.fields { f } else { panic!("Named fields only") }
    } else { panic!("Only structs supported") };


    let struct_fields = fields.named.iter().map(|f| {
        let f_name = &f.ident;
        let f_ty = &f.ty;
        quote! {
            pub #f_name: <#f_ty as golem_rust::agentic::ArenaMember<#arena_name>>::NonRecursive
        }
    });

    let to_arena_fields = fields.named.iter().map(|f| {
        let f_name = &f.ident;
        let f_ty = &f.ty;
        quote! { #f_name: <#f_ty as golem_rust::agentic::ArenaMember<#arena_name>>::deflate(&self.#f_name, arena) }
    });

    let from_arena_fields = fields.named.iter().map(|f| {
        let f_name = &f.ident;
        let f_ty = &f.ty;
        // This calls <F as ArenaMember<Arena>>::inflate(...)
        // If F is Option<Box<Tree>>, this will recursively call Tree::from_arena(idx, arena)
        quote! {
        #f_name: <#f_ty as golem_rust::agentic::ArenaMember<#arena_name>>::inflate(node.#f_name.clone(), arena)
    }
    });


    let q = quote! {
        #[derive(Debug)]
        pub struct #arena_name { pub nodes: Vec<#non_rec_name> }

        impl #arena_name {
            pub fn new() -> Self {
                Self { nodes: Vec::new() }
            }

            pub fn add(&mut self, node: #non_rec_name) -> usize {
              let idx = self.nodes.len();
              self.nodes.push(node);
              idx
            }
        }

        #[derive(Clone, Debug)] // Indices are usually clonable
        pub struct #non_rec_name { #(#struct_fields,)* }

        impl #name {
            pub fn to_arena(&self, arena: &mut #arena_name) -> usize {
                // Push a placeholder or handle recursion
                // (Simplified for brevity: in real impl, handle depth-first)
                let flattened = #non_rec_name { #(#to_arena_fields,)* };
                arena.add(flattened)
            }
            pub fn from_arena(idx: usize, arena: &#arena_name) -> Self {

              let node = &arena.nodes[idx];

              Self {
                #(#from_arena_fields,)*
              }
            }
        }

        // Essential: Link the original type to the ArenaMember trait
        impl golem_rust::agentic::ArenaMember<#arena_name> for #name {
            type NonRecursive = usize;
            fn deflate(&self, arena: &mut #arena_name) -> Self::NonRecursive {
                self.to_arena(arena)
            }
            fn inflate(idx: Self::NonRecursive, arena: &#arena_name) -> Self {
                Self::from_arena(idx, arena)
            }
        }
    };

    TokenStream::from(q)
}
