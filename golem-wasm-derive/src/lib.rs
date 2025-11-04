// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod from;
mod into;

use proc_macro::TokenStream;

#[proc_macro_derive(IntoValue, attributes(wit_transparent, unit_case, wit_field))]
pub fn derive_into_value(input: TokenStream) -> TokenStream {
    into::derive_into_value(input)
}

#[proc_macro_derive(FromValueAndType)]
pub fn derive_from_value_and_type(input: TokenStream) -> TokenStream {
    from::derive_from_value_and_type(input)
}
