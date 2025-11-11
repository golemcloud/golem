use syn::ItemTrait;

pub fn no_constructor_method_error(item_trait: &ItemTrait) -> proc_macro2::TokenStream {
    compile_error(item_trait, "Agent traits must have a constructor method to create instances of the agent. Please define a method with constructor parameters if any, returning `Self`.")
}

pub fn multiple_constructor_methods_error(item_trait: &ItemTrait) -> proc_macro2::TokenStream {
    compile_error(item_trait, "Agent traits can have only one constructor method. Please ensure there is only one method returning `Self`.")
}

pub fn compile_error(item_trait: &ItemTrait, msg: &str) -> proc_macro2::TokenStream {
    syn::Error::new_spanned(item_trait, msg).to_compile_error()
}
