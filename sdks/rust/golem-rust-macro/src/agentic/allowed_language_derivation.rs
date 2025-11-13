use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput};

pub fn derive_allowed_languages(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let name = &ast.ident;

    let Data::Enum(data_enum) = &ast.data else {
        return syn::Error::new_spanned(
            &ast.ident,
            "AllowedLanguages can only be derived for enums",
        )
        .to_compile_error()
        .into();
    };

    let variants: Vec<_> = data_enum.variants.iter().map(|v| &v.ident).collect();
    let codes: Vec<String> = variants
        .iter()
        .map(|v| v.to_string().to_lowercase())
        .collect();
    let code_strs: Vec<_> = codes.iter().map(|s| s.as_str()).collect::<Vec<_>>();

    let expanded = quote! {
        impl golem_rust::agentic::AllowedLanguages for #name {
            fn all() -> &'static [&'static str] {
                &[#(#code_strs),*]
            }

            fn from_language_code(code: &str) -> Option<Self> {
                match code {
                    #(
                        #code_strs => Some(Self::#variants),
                    )*
                    _ => None,
                }
            }

            fn to_language_code(&self) -> &'static str {
                match self {
                    #(
                        Self::#variants => #code_strs,
                    )*
                }
            }
        }
    };

    expanded.into()
}
