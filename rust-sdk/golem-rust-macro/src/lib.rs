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

use proc_macro::TokenStream;

use crate::transaction::golem_operation_impl;

mod transaction;
mod value;

#[proc_macro_derive(IntoValue, attributes(flatten_value, unit_case))]
pub fn derive_into_value(input: TokenStream) -> TokenStream {
    value::derive_into_value(input)
}

#[proc_macro_derive(FromValueAndType, attributes(flatten_value, unit_case))]
pub fn derive_from_value_and_type(input: TokenStream) -> TokenStream {
    value::derive_from_value_and_type(input)
}

/// Defines a function as an `Operation` that can be used in transactions
#[proc_macro_attribute]
pub fn golem_operation(attr: TokenStream, item: TokenStream) -> TokenStream {
    golem_operation_impl(attr, item)
}
