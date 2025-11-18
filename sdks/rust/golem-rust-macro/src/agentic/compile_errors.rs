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

pub fn compile_error(item_trait: &ItemTrait, msg: &str) -> proc_macro2::TokenStream {
    syn::Error::new_spanned(item_trait, msg).to_compile_error()
}
