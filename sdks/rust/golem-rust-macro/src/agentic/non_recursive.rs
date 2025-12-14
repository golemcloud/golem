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

    // 1. Generate fields for the NonRecursive struct
    let struct_fields = fields.named.iter().map(|f| {
        let f_name = &f.ident;
        let f_ty = &f.ty;
        let type_str = quote!(#f_ty).to_string();
        if type_str.contains(&name.to_string()) {
            quote! { pub #f_name: Option<usize> }
        } else {
            quote! { pub #f_name: #f_ty }
        }
    });

    // 2. Logic for flattening: Recursive -> Arena (to_arena)
    let to_arena_fields = fields.named.iter().map(|f| {
        let f_name = &f.ident;
        let type_str = quote!(#f.ty).to_string();
        if type_str.contains(&name.to_string()) {
            quote! { #f_name: self.#f_name.as_ref().map(|node| node.to_arena(arena)) }
        } else {
            quote! { #f_name: self.#f_name.clone() }
        }
    });

    // 3. Logic for inflating: Arena -> Recursive (from_arena)
    let from_arena_fields = fields.named.iter().map(|f| {
        let f_name = &f.ident;
        let type_str = quote!(#f.ty).to_string();
        if type_str.contains(&name.to_string()) {
            quote! { #f_name: node.#f_name.map(|idx| Box::new(#name::from_arena(idx, arena))) }
        } else {
            quote! { #f_name: node.#f_name.clone() }
        }
    });

    let expanded = quote! {
        #[derive(Debug, Clone)]
        pub struct #non_rec_name { #(#struct_fields,)* }

        #[derive(Debug, Clone)]
        pub struct #arena_name { pub nodes: Vec<#non_rec_name> }
        impl #arena_name {
            pub fn new() -> Self { Self { nodes: Vec::new() } }
            pub fn add(&mut self, node: #non_rec_name) -> usize {
                let idx = self.nodes.len();
                self.nodes.push(node);
                idx
            }
        }

        impl #name {
            pub fn to_arena(&self, arena: &mut #arena_name) -> usize {
                let non_rec = #non_rec_name { #(#to_arena_fields,)* };
                arena.add(non_rec)
            }

            pub fn from_arena(idx: usize, arena: &#arena_name) -> Self {
                let node = &arena.nodes[idx];
                Self { #(#from_arena_fields,)* }
            }
        }
    };
    TokenStream::from(expanded)
}
