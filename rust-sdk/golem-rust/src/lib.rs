mod der_macro;
mod wit_gen;

use proc_macro::TokenStream;
use syn::*;

/**
 * Usage:
 *      
        #[derive(WIT)]
        #[wit(WitPerson)]
        pub struct Person {
            
            pub name: String,

            #[rename("age2")]
            pub age: i32
        }
 */
#[proc_macro_derive(WIT, attributes(wit, rename))]
pub fn derive(input: TokenStream) -> TokenStream {

    let mut input = parse_macro_input!(input as DeriveInput);

    der_macro::expand_wit(&mut input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/**
 * TODO idea is to generate wit file from annotated rust code that would describe wit interface
 */
#[proc_macro_attribute]
pub fn wit_file(attr: TokenStream, item: TokenStream) -> TokenStream {

    let mut input = parse_macro_input!(item as DeriveInput);

    wit_gen::generate_witfile(&mut input, "../target".to_owned())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}