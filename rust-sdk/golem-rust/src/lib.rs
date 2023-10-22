mod attr_macro;
mod der_macro;
mod wit_gen;

use proc_macro::TokenStream;
use quote::quote;
use syn::*;
use attr_macro::*;

/**
 * 
    #[derive_wit(WitPerson, name => name2)]
    pub struct Person {
        pub name: String,
        
        pub age: i32
    }
 */
#[proc_macro_attribute]
pub fn derive_wit(attr: TokenStream, item: TokenStream) -> TokenStream { 

    let original_item: proc_macro2::TokenStream = item.clone().into();
    let mut input = parse_macro_input!(item as DeriveInput);
    let mut attributes = parse_macro_input!(attr as DeriveAttributes);
    
    attr_macro::expand_wit(&mut attributes, &mut input)
        .map(|generated|  {
            quote!(
                #generated
                #original_item
            )
        })
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/**
 * Usage:
 *      
        #[derive(WIT)]
        #[wit(WitPerson)]
        pub struct Person {
            
            pub name: String,

            #[wit(rename = "age2")]
            pub age: i32
        }
 */
#[proc_macro_derive(WIT, attributes(wit))]
pub fn derive(input: TokenStream) -> TokenStream {

    let mut input = parse_macro_input!(input as DeriveInput);

    der_macro::expand_wit(&mut input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/**
 * Usage:
 * 
    
    #[wit_file]
    trait AuctionApi {
        fn close_auction() -> Option<String>;
    }
 * 
 * 
 */
#[proc_macro_attribute]
pub fn wit_file(attr: TokenStream, item: TokenStream) -> TokenStream {

    let mut input = parse_macro_input!(item as DeriveInput);

    println!("aaarrrgg {:#?}", input);

    wit_gen::generate_witfile(&mut input, "/Users/jregec/Coding/golem/golem-wit-utils".to_owned())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}