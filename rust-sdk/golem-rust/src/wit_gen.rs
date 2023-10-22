
use proc_macro2::TokenStream;

pub fn generate_witfile(ast: &mut syn::DeriveInput, path: String) -> syn::Result<TokenStream>{

    // TODO explore how to generate WIT file from annotated Struct

    Ok(TokenStream::new())
}