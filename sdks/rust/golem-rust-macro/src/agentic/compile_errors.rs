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

use proc_macro2::Span;
use syn::ItemTrait;

pub fn no_constructor_method_error(item_trait: &ItemTrait) -> proc_macro2::TokenStream {
    compile_error(item_trait, "Agent traits must have a constructor method to create instances of the agent. Please define a method with constructor parameters if any, returning `Self`.")
}

pub fn multiple_constructor_methods_error(item_trait: &ItemTrait) -> proc_macro2::TokenStream {
    compile_error(item_trait, "Agent traits can have only one constructor method. Please ensure there is only one method returning `Self`.")
}

pub fn async_trait_in_agent_definition_error(item_trait: &ItemTrait) -> proc_macro2::TokenStream {
    compile_error(item_trait, "The `#[async_trait]` attribute is not allowed on agent traits. Agent traits automatically support async methods without this attribute.")
}

pub fn generic_type_in_constructor_error(span: Span, type_name: &str) -> proc_macro2::TokenStream {
    syn::Error::new(
        span,
        format!(
            "Generic type `{}` cannot be used as an agent constructor parameter",
            type_name
        ),
    )
    .to_compile_error()
}

pub fn generic_type_in_agent_method_error(span: Span, type_name: &str) -> proc_macro2::TokenStream {
    let msg = format!(
        "Generic type `{}` cannot be used in agent method parameter",
        type_name
    );
    syn::Error::new(span, msg).to_compile_error()
}

pub fn generic_type_in_agent_return_type_error(
    span: Span,
    type_name: &str,
) -> proc_macro2::TokenStream {
    let msg = format!(
        "Generic type `{}` cannot be used in agent method return type",
        type_name
    );
    syn::Error::new(span, msg).to_compile_error()
}

pub fn compile_error(item_trait: &ItemTrait, msg: &str) -> proc_macro2::TokenStream {
    syn::Error::new_spanned(item_trait, msg).to_compile_error()
}

pub fn endpoint_on_static_method_error(span: Span) -> proc_macro2::TokenStream {
    syn::Error::new(
        span,
        "#[endpoint] attribute is not allowed on static methods. Please ensure the method takes &self or &mut self as the first parameter."
    ).to_compile_error()
}

pub fn endpoint_on_constructor_method_error(span: Span) -> proc_macro2::TokenStream {
    syn::Error::new(
        span,
        "#[endpoint] attribute is not allowed on constructor methods. Please remove the #[endpoint] attribute from this method."
    ).to_compile_error()
}

pub fn invalid_static_method_in_agent_error(span: Span, method_name: &str) -> proc_macro2::TokenStream {
    syn::Error::new(
        span,
        format!(
            "Static method `{}` is not allowed in agent traits. Only constructor methods (returning `Self` or the agent type name) are permitted as static methods. Please convert this to an instance method.",
            method_name
        ),
    )
    .to_compile_error()
}
